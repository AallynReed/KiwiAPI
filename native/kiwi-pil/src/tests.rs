//! Replays native/port/parity/gen_pil_fixtures.py's Pillow scenes and metrics.

use serde_json::Value;

use crate::{Draw, Font, Library, Rgba, image::Ink};

fn testdata(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(name)
}

fn font_bytes(name: &str) -> Vec<u8> {
    let file = if name == "bold" { "DejaVuSans-Bold.ttf" } else { "DejaVuSans.ttf" };
    std::fs::read(testdata("fonts").join(file)).unwrap()
}

fn decode_rgba(path: &std::path::Path) -> (usize, usize, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba);
    buf.truncate(info.buffer_size());
    (info.width as usize, info.height as usize, buf)
}

fn ink(v: &Value) -> Option<Ink> {
    let a = v.as_array()?;
    Some([0, 1, 2, 3].map(|i| a[i].as_u64().unwrap() as u8))
}

fn quad(v: &Value) -> [f64; 4] {
    let a = v.as_array().unwrap();
    [0, 1, 2, 3].map(|i| a[i].as_f64().unwrap())
}

fn points(v: &Value) -> Vec<(f64, f64)> {
    v.as_array().unwrap().iter().map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap())).collect()
}

fn replay(im: &mut Rgba, ops: &[Value], regular: &[u8], bold: &[u8]) {
    let lib = Library::new().unwrap();
    let mut d = Draw::new(im);
    for o in ops {
        let width = o.get("width").and_then(Value::as_i64).unwrap_or(1) as i32;
        let fill = o.get("fill").and_then(ink);
        let outline = o.get("outline").and_then(ink);
        match o["op"].as_str().unwrap() {
            "rectangle" => d.rectangle(quad(&o["xy"]), fill, outline, width).unwrap(),
            "ellipse" => d.ellipse(quad(&o["xy"]), fill, outline, width).unwrap(),
            "polygon" => d.polygon(&points(&o["xy"]), fill.unwrap()).unwrap(),
            "line" => d.line(&points(&o["xy"]), fill.unwrap(), width, o["curve"].as_bool().unwrap_or(false)).unwrap(),
            "pieslice" => d
                .pieslice(quad(&o["xy"]), o["start"].as_f64().unwrap(), o["end"].as_f64().unwrap(), fill.unwrap())
                .unwrap(),
            "rounded_rectangle" => {
                d.rounded_rectangle(quad(&o["xy"]), o["radius"].as_f64().unwrap(), fill, outline, width).unwrap()
            }
            "text" => {
                let data = if o["font"] == "bold" { bold } else { regular };
                let mut f = Font::new(&lib, data, o["size"].as_f64().unwrap() as f32).unwrap();
                let xy = points(&Value::Array(vec![o["xy"].clone()]))[0];
                d.text(xy, o["text"].as_str().unwrap(), &mut f, fill.unwrap()).unwrap();
            }
            other => panic!("unknown op {other}"),
        }
    }
}

fn first_diff(a: &[u8], b: &[u8], width: usize) -> String {
    let i = a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(0) / 4;
    let n = a.chunks(4).zip(b.chunks(4)).filter(|(x, y)| x != y).count();
    let max = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
    format!("{n} pixels differ (max channel diff {max}), first at ({}, {})", i % width, i / width)
}

#[test]
fn scenes_match_pillow() {
    let regular = font_bytes("regular");
    let bold = font_bytes("bold");
    let scenes: Value = serde_json::from_slice(&std::fs::read(testdata("scenes.json")).unwrap()).unwrap();
    let mut failures = Vec::new();
    for s in scenes.as_array().unwrap() {
        let name = s["name"].as_str().unwrap();
        let size = &s["size"];
        let (w, h) = (size[0].as_u64().unwrap() as usize, size[1].as_u64().unwrap() as usize);
        let mut im = Rgba::new(w, h, ink(&s["bg"]).unwrap());
        replay(&mut im, s["ops"].as_array().unwrap(), &regular, &bold);
        let (ew, eh, want) = decode_rgba(&testdata("scenes").join(format!("{name}.png")));
        assert_eq!((ew, eh), (w, h));
        let got: Vec<u8> = im.data.iter().flatten().copied().collect();
        if got != want {
            failures.push(format!("{name}: {}", first_diff(&got, &want, w)));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn textbbox_matches_pillow() {
    let regular = font_bytes("regular");
    let bold = font_bytes("bold");
    let lib = Library::new().unwrap();
    let rows: Value = serde_json::from_slice(&std::fs::read(testdata("metrics.json")).unwrap()).unwrap();
    let mut bad = Vec::new();
    let mut im = Rgba::new(1, 1, [0; 4]);
    let d = Draw::new(&mut im);
    for r in rows.as_array().unwrap() {
        let data = if r["font"] == "bold" { &bold } else { &regular };
        let f = Font::new(&lib, data, r["size"].as_f64().unwrap() as f32).unwrap();
        let text = r["text"].as_str().unwrap();
        let b = d.textbbox((0.0, 0.0), text, &f).unwrap();
        let want = quad(&r["bbox"]);
        if [b.0, b.1, b.2, b.3] != want {
            bad.push(format!("{} {} {text:?}: {b:?} != {want:?}", r["font"], r["size"]));
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {} differ: {:#?}",
        bad.len(),
        rows.as_array().unwrap().len(),
        &bad[..bad.len().min(20)]
    );
}

#[test]
fn png_filters_and_chunks() {
    let mut im = Rgba::new(70, 40, [13, 17, 23, 255]);
    Draw::new(&mut im).ellipse([5.0, 5.0, 60.0, 30.0], Some([93, 208, 120, 255]), None, 1).unwrap();
    let png = crate::png::encode_rgb(&im.to_rgb(), 70, 40);
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&png[12..16], b"IHDR");
    assert_eq!(&png[png.len() - 12..], b"\0\0\0\0IEND\xaeB`\x82");
}

use sha2::{Digest, Sha256};

use super::*;

#[test]
fn matches_cpython_small() {
    // zlib.compress(b"hello hello hello"), and level 9 / raw / gzip variants.
    assert_eq!(
        compress(b"hello hello hello", -1, 15).unwrap(),
        [120, 156, 203, 72, 205, 201, 201, 87, 200, 64, 144, 0, 58, 46, 6, 125]
    );
    let gz = gzip_compress(b"hello hello hello", 6, 7).unwrap();
    assert_eq!(&gz[..10], &[0x1f, 0x8b, 8, 0, 7, 0, 0, 0, 0, 255]);
    let mut z = compressobj(0, 15, 8, 0).unwrap();
    let mut out = z.compress(b"abc").unwrap();
    out.extend(z.flush(Z_SYNC_FLUSH).unwrap());
    assert_eq!(out, [120, 1, 0, 3, 0, 252, 255, 97, 98, 99, 0, 0, 0, 255, 255]);
}

#[test]
fn zip_encode_parts_are_the_stream_in_buffers() {
    let raw: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    let parts = zip_encode(&raw, 601, 6, Z_FILTERED, 65536).unwrap();
    let mut z = compressobj(6, 15, 9, Z_FILTERED).unwrap();
    let mut whole = Vec::new();
    for r in raw.chunks(601) {
        whole.extend(z.compress(r).unwrap());
    }
    whole.extend(z.flush(Z_FINISH).unwrap());
    assert_eq!(parts.concat(), whole);
    assert!(parts[..parts.len() - 1].iter().all(|p| p.len() == 65536));
}

/// native/port/parity/zlib_corpus.py's corpus, replayed as CPython drives
/// zlib (and, for row-fed cases at levels 1-9, as Pillow's ZipEncode does).
#[test]
#[ignore = "needs KIWI_ZLIB_CORPUS"]
fn cpython_corpus() {
    let dir = std::path::PathBuf::from(std::env::var("KIWI_ZLIB_CORPUS").unwrap());
    let blob = std::fs::read(dir.join("inputs.bin")).unwrap();
    let spec: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("cases.json")).unwrap()).unwrap();
    let inputs = spec["inputs"].as_array().unwrap();
    let hex = |b: &[u8]| Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect::<String>();
    let (mut fails, mut zipenc) = (Vec::new(), 0);
    let cases = spec["cases"].as_array().unwrap();
    for c in cases {
        let c = c.as_array().unwrap();
        let int = |i: usize| c[i].as_i64().unwrap() as i32;
        let item = &inputs[int(0) as usize];
        let (off, len, row) = (item["offset"].as_u64().unwrap() as usize, item["len"].as_u64().unwrap() as usize, {
            item["row"].as_u64().unwrap() as usize
        });
        let data = &blob[off..off + len];
        let (level, strategy, mem, wbits, plan, flush) =
            (int(1), int(2), int(3), int(4), c[5].as_str().unwrap(), int(6));
        let got = if plan == "oneshot" {
            compress(data, level, wbits).unwrap()
        } else {
            let mut z = compressobj(level, wbits, mem, strategy).unwrap();
            let parts: Vec<&[u8]> = match plan {
                "whole" => vec![data],
                "rows" => data.chunks(row).collect(),
                "chunk32768" => data.chunks(32768).collect(),
                _ => {
                    let (mut v, mut i, mut seed) = (Vec::new(), 0, len as u64);
                    let sizes = [1, 7, 300, 4096, 40000, 100000];
                    while i < len {
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                        let n = sizes[(seed >> 33) as usize % sizes.len()];
                        v.push(&data[i..(i + n).min(len)]);
                        i += n;
                    }
                    v
                }
            };
            let mut out = Vec::new();
            for p in parts {
                out.extend(z.compress(p).unwrap());
            }
            out.extend(z.flush(flush).unwrap());
            out
        };
        let label =
            format!("{} level {level} strategy {strategy} mem {mem} wbits {wbits} {plan} flush {flush}", item["name"]);
        if hex(&got) != c[7].as_str().unwrap() {
            fails.push(label.clone());
        }
        if plan == "rows" && level >= 1 && wbits == 15 && mem == 9 {
            let bufsize = 65536.max(row);
            if zip_encode(data, row, level, strategy, bufsize).unwrap().concat() != got {
                fails.push(format!("zip_encode {label}"));
            }
            zipenc += 1;
        }
    }
    println!(
        "zlib {} vs CPython's {}: {} cases, {zipenc} zip_encode checks, {} failures",
        version(),
        spec["zlib"],
        cases.len(),
        fails.len()
    );
    assert!(fails.is_empty(), "{} of {} differ: {:#?}", fails.len(), cases.len(), &fails[..fails.len().min(40)]);
}

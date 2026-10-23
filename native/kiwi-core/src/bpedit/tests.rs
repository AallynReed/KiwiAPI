use serde_json::json;

use super::{Fault, codec, editor, materials, py, qb};
use crate::blueprint::Voxel;

fn vox(x: i64, y: i64, z: i64, ty: u16, w: u8) -> Voxel {
    Voxel { x, y, z, r: 200, g: 100, b: 50, w, ty }
}

#[test]
fn python_number_formatting() {
    assert_eq!(py::commas(150_000), "150,000");
    assert_eq!(py::commas(-1_234_567), "-1,234,567");
    assert_eq!(py::commas(999), "999");
    assert_eq!([0.5, 1.5, 2.5, -0.5, -1.5, 2.4].map(py::round_half_even), [0, 2, 2, 0, -2, 2]);
    assert_eq!(py::fdiv(-7, 2), -4);
}

#[test]
fn python_int_and_truth() {
    assert_eq!(py::int(&json!("  -4_2 ")), Ok(-42));
    assert_eq!(py::int(&json!(2.9)), Ok(2));
    assert_eq!(py::int(&json!(true)), Ok(1));
    assert_eq!(py::int(&json!("x")), Err(py::IntErr::Value));
    assert_eq!(py::int(&json!(null)), Err(py::IntErr::Type));
    assert!(!py::truthy(&json!(0.0)) && py::truthy(&json!("0")) && !py::truthy(&json!({})));
    assert_eq!(py::str_of(&json!(["a", 1])), "['a', 1]");
}

#[test]
fn materials_read_like_python() {
    assert_eq!(materials::describe(18, 3), "Glass - 44% opacity");
    assert_eq!(materials::describe(55, 2), "Glowing Solid - unused finish 2");
    assert_eq!(materials::describe(21, 9), "Solid - finish 9");
    assert_eq!(materials::describe(39, 0), "Deco placeholder (type 39)");
    assert_eq!(materials::describe(7, 0), "Unknown type 7");
    assert_eq!(materials::validate(56, 99), Ok((56, 7)));
    assert_eq!(materials::validate(55, 3), Ok((55, 0)));
    assert_eq!(materials::validate(4, 0), Err("Type 4 is not an editable material.".into()));
}

#[test]
fn codec_round_trips_and_errors() {
    let vs = vec![vox(0, 0, 0, 21, 0), vox(2, 1, 0, 18, 3)];
    let v5 = codec::encode(&vs, 5, None, &[], [0, 0, 0], None).unwrap();
    let d = codec::decode(&v5).unwrap();
    assert_eq!((d.size, d.pos, d.voxels.len()), ([3, 2, 1], [-1, 0, 0], 2));
    assert_eq!(codec::attachment(&d), Some([1, 0, 0]));
    assert_eq!(codec::encode_doc(&d).unwrap(), v5);
    let err = |b: &[u8]| codec::decode(b).unwrap_err();
    assert_eq!(err(b"nope"), Fault::Blueprint("Not a Trove blueprint (missing 'kiwib' magic).".into()));
    assert_eq!(err(b"kiwib\x07\0\0\0\0"), Fault::Blueprint("Unsupported kiwib blueprint version: 7.".into()));
    assert_eq!(
        err(b"kiwib\x03\0\0\0\x00"),
        Fault::Blueprint("Blueprint contains no voxels (empty placeholder).".into())
    );
    assert_eq!(err(b"kiwib\x03\0\0\0\x01\x02"), Fault::Blueprint("Truncated LEB128 value.".into()));
    assert!(codec::is_empty(b"kiwib\x05\0\0\0\x78\x9c\xff"));
    let recs = vec![(1, -2, 3, b"abc".to_vec())];
    let blob = codec::write_entity_records(&recs, b"tail");
    assert_eq!(codec::read_entity_records(&blob), Some((recs, b"tail".to_vec())));
}

#[test]
fn edits_shift_the_grid_and_keep_the_grip() {
    let raw =
        codec::encode(&[vox(0, 0, 0, 21, 0), vox(1, 0, 0, 39, 0)], 5, Some([0, 0, 0]), &[], [0, 0, 0], None).unwrap();
    let edits = json!([{"add": [-1, 0, 0], "type": 21, "w": 1, "rgb": 255}, {"i": 1, "type": 18, "w": 1}]);
    let (out, summary) = editor::apply_edits(&raw, &edits).unwrap();
    assert_eq!(summary["ignored"], json!(1));
    let d = codec::decode(&out).unwrap();
    assert_eq!((d.size, codec::attachment(&d)), ([3, 1, 1], Some([1, 0, 0])));
    let bad = editor::apply_edits(&raw, &json!([{"add": [0, 0, 0], "type": 21, "w": 0}])).unwrap_err();
    assert!(matches!(bad, Fault::Unhandled(_)), "a missing rgb is Python's KeyError");
}

#[test]
fn qb_set_round_trips() {
    let raw = codec::encode(
        &[vox(0, 0, 0, 21, 2), vox(1, 0, 0, 56, 5), vox(1, 1, 0, 1, 0)],
        5,
        Some([0, -3, 0]),
        &[],
        [0; 3],
        None,
    )
    .unwrap();
    let exported = qb::from_blueprint(&codec::decode(&raw).unwrap(), "m");
    assert_eq!(
        exported.files.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["m.qb", "m_a.qb", "m_s.qb", "m_t.qb"]
    );
    let (back, summary) = qb::to_blueprint(&exported.files).unwrap();
    assert_eq!(summary["attachment_source"], json!("marker"));
    let d = codec::decode(&back).unwrap();
    assert_eq!((d.voxels.len(), codec::attachment(&d)), (3, Some([0, 3, 0])));
}

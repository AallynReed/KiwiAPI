//! Hand-checked cases; the golden comparisons against Python live in
//! `kiwi-server/src/updates/tests.rs`.

use super::{
    compare::{self, LineKind},
    difflib::{Tag, opcodes},
    pkfx, tree,
};

#[test]
fn opcodes_like_difflib() {
    // SequenceMatcher(None, "abxcd", "abcd", autojunk=False).get_opcodes()
    let a: Vec<char> = "abxcd".chars().collect();
    let b: Vec<char> = "abcd".chars().collect();
    let ops: Vec<_> = opcodes(&a, &b).iter().map(|o| (o.tag, o.i1, o.i2, o.j1, o.j2)).collect();
    assert_eq!(ops, vec![(Tag::Equal, 0, 2, 0, 2), (Tag::Delete, 2, 3, 2, 2), (Tag::Equal, 3, 5, 2, 4)]);
    assert!(opcodes::<u8>(&[], &[]).is_empty());
}

#[test]
fn hunks_bridge_short_gaps_and_keep_context() {
    let a: Vec<String> = (0..20).map(|i| i.to_string()).collect();
    let mut b = a.clone();
    b[5] = "five".into();
    b[10] = "ten".into();
    let h = compare::make_hunks(&a, &b, 3);
    assert_eq!(h.len(), 1, "a 4-line gap is bridged");
    assert_eq!((h[0].left_start, h[0].right_start), (3, 3));
    let kinds: Vec<_> = h[0].lines.iter().map(|l| l.kind).collect();
    assert_eq!(kinds.iter().filter(|k| **k == LineKind::Remove).count(), 2);
    assert_eq!(h[0].lines.last().map(|l| l.left), Some(Some(14)));
    assert!(compare::make_hunks(&a, &a, 3).is_empty());
}

#[test]
fn decode_escapes_controls_and_falls_back_to_latin1() {
    let d = compare::decode_blob(Some(b"a\x00b\tc\x0cd\xe9"));
    assert_eq!(d.lines, vec!["a\\x00b\tc", "d\u{e9}"]);
    assert!(compare::decode_blob(Some(&vec![b'x'; compare::MAX_DIFF_BYTES + 1])).reason == Some("too large"));
    assert_eq!(compare::decode_blob(None).lines, Vec::<String>::new());
}

#[test]
fn pkfx_refs() {
    let text = "CEffect $LOCAL$/A\n{\n\tT = \"Tex/a.DDS\";\n\tAnimResource = \"M/p.pkmm\";\n\tX = \"tex/A.dds\";\n}\n\
                CNEdEditorRoom $LOCAL$/B\n{\n\tM = \"Editor/room.fbx\";\n}\n";
    assert_eq!(pkfx::extract_refs(text), vec!["Tex/a.DDS", "M/p.pkan"]);
    assert_eq!(pkfx::media_type_for("a/b.PKFX"), "text/plain; charset=utf-8");
    assert!(pkfx::is_asset("x.TGA") && !pkfx::is_asset("tga") && !pkfx::is_asset("x.xml"));
}

#[test]
fn listing_and_view_kind() {
    let rows = vec![
        tree::Row { path: "b.txt".into(), size: 2, last_ordinal: 1 },
        tree::Row { path: "a/x".into(), size: 3, last_ordinal: 5 },
        tree::Row { path: "a/y/z".into(), size: 4, last_ordinal: 2 },
    ];
    let l = tree::directory_listing(&rows, "");
    assert_eq!(l.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(), vec!["a/", "b.txt"]);
    assert_eq!((l[0].file_count, l[0].size, l[0].last_ordinal), (2, 7, 5));
    assert_eq!(tree::view_kind("ui/x.PNG", 1 << 30), Some("image"));
    assert_eq!(tree::view_kind("dir.v/readme", 10), None);
    assert_eq!(tree::view_kind("big.txt", tree::VIEW_MAX_BYTES + 1), Some("too_large"));
    assert_eq!(tree::view_text(b"ok\xff"), None);
}

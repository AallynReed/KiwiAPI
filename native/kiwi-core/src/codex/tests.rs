//! Python string/bytes semantics the decoders depend on. The decoders
//! themselves are checked against Python output in kiwi-server's codex tests.

use super::{bonuses, links, mastery, py, recipe, types, wire};

#[test]
fn python_strings() {
    assert_eq!(py::title("1abc_def 2nd o'neil"), "1Abc_Def 2Nd O'Neil");
    for (p, stem) in
        [("a.", "a."), ("a.b.c", "a.b"), (".hidden", ".hidden"), ("item/foo/", "foo"), ("x//y.z", "y"), ("a..", "a..")]
    {
        assert_eq!(py::path_stem(p), stem, "{p}");
    }
    assert_eq!(py::utf8_ignore(b"a\xe2\x82b\xffc\xed\xa0\x80d"), "abcd");
    assert_eq!(py::ascii_ignore(b"a\x80b"), "ab");
    assert_eq!(py::latin1(b"\xe9"), "\u{e9}");
    assert_eq!(py::strip("\x1c x \u{a0}"), "x");
    assert_eq!(py::rstrip_chars("abilities/x./", "./"), "abilities/x");
}

#[test]
fn varints() {
    assert_eq!(wire::read_uleb(&[0x96, 0x01], 0), Ok((150, 2)));
    assert!(wire::read_uleb(&[0x80], 0).is_err());
    assert_eq!(wire::read_uleb(&[0xff; 11], 0).unwrap_err().0, "uleb128 too long");
    assert_eq!(wire::read_varint(&[0x80, 0x80], 0), (None, 0));
    assert_eq!(wire::unzig(3), -2);
    assert_eq!(wire::unzig(4), 2);
    assert_eq!(wire::content_start(&[1, 0, 2, 9, 9]), 3);
}

#[test]
fn identity_markers() {
    // header field, marker (wt 1), name key (f1), tradable (f14 = 2), category (f2).
    let d = [0x08, 0x02, b'h', b'i', 0x01, 0x18, 0x03, b'$', b'n', b'k', 0xE0, 0x01, 0x02, 0x28, 0x01, b'C'];
    let ident = wire::decode_identity(&d).unwrap().unwrap();
    assert_eq!(ident.name_key.as_deref(), Some("$nk"));
    assert_eq!(ident.category.as_deref(), Some("C"));
    assert_eq!(ident.tradable, Some(true));
    assert_eq!(wire::decode_identity(&[0x08, 0x01, b'x']).unwrap(), None);
}

#[test]
fn keys_and_paths() {
    assert_eq!(bonuses::bonus_key("$Stat_HealthRegen_controller"), "$Stat_HealthRegenBonus_controller");
    assert_eq!(bonuses::bonus_key("$Stat_Jump"), "$Stat_JumpBonus");
    assert_eq!(links::entry_path(" Item\\Foo "), "prefabs/item/foo.binfab");
    assert_eq!(types::classify("prefabs/collections/pet/dog_npc.binfab"), None);
    assert_eq!(types::classify("prefabs/collections/fishing/rod.binfab"), Some("fishingpole"));
    assert_eq!(mastery::infer_mastery_base("skins/knight", false), ("collections/skin/knight".into(), 0));
    assert_eq!(mastery::infer_mastery_base("knightbody", false), ("collections/skin/knightbody".into(), 35));
    assert_eq!(recipe::pretty_name("item/foo/mega_box_notrade.binfab"), "Mega Box");
    assert_eq!(recipe::category_from_output("item/pet/dog"), "Ally");
}

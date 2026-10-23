use serde_json::Value;

use super::*;

fn golden() -> Value {
    serde_json::from_str(include_str!("../../testdata/email/golden.json")).expect("email golden")
}

fn expect(v: &Value) -> Email {
    match (v.get("ok"), v.get("err")) {
        (Some(Value::String(s)), _) => Email::Valid(s.clone()),
        (_, Some(Value::String(e))) => {
            Email::Invalid(e.strip_prefix("value is not a valid email address: ").unwrap_or(e).to_owned())
        }
        _ => panic!("bad golden outcome {v}"),
    }
}

#[test]
fn matches_python_validators() {
    let g = golden();
    let mut failures = Vec::new();
    for case in g["cases"].as_array().unwrap() {
        let input = case["in"].as_str().unwrap();
        for (kind, got) in [("pydantic", validate(input)), ("direct", validate_direct(input))] {
            let want = expect(&case[kind]);
            if got != want {
                failures.push(format!("{kind} {input:?}\n  want {want:?}\n  got  {got:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn nfc_matches_unicodedata() {
    let g = golden();
    for pair in g["nfc_chars"].as_array().unwrap() {
        let c = char::from_u32(pair[0].as_u64().unwrap() as u32).unwrap();
        assert_eq!(nfc_str(&c.to_string()), pair[1].as_str().unwrap(), "U+{:04X}", c as u32);
    }
    for pair in g["nfc_strings"].as_array().unwrap() {
        assert_eq!(nfc_str(pair[0].as_str().unwrap()), pair[1].as_str().unwrap());
    }
    assert_eq!(nfc_str("\u{1100}\u{1161}\u{11a8}"), "\u{ac01}");
}

#[test]
fn punycode_round_trips() {
    let s: Vec<u32> = "bücher".chars().map(u32::from).collect();
    assert_eq!(idna::punycode_encode(&s), b"bcher-kva");
    let s: Vec<u32> = "公司".chars().map(u32::from).collect();
    assert_eq!(idna::punycode_encode(&s), b"55qx5d");
    assert_eq!(idna::repr(&[0x61, 0xD800, 0x27]), "\"a\\ud800'\"");
}

#[test]
fn nfc_window_matches_whole_suffix() {
    let g = golden();
    let mut inputs: Vec<String> =
        g["cases"].as_array().unwrap().iter().map(|c| c["in"].as_str().unwrap().to_owned()).collect();
    inputs.extend(g["nfc_strings"].as_array().unwrap().iter().map(|p| p[0].as_str().unwrap().to_owned()));
    inputs.push("\u{1100}\u{1161}\u{11a8}x\u{0b47}\u{0b3e}a\u{0328}\u{0301}".to_owned());
    for s in inputs {
        let chars: Vec<(usize, char)> = s.char_indices().collect();
        for (k, &(i, _)) in chars.iter().enumerate() {
            let stop = chars[k + 1..].iter().find(|&&(_, n)| unidata::stable_starter(u32::from(n)));
            let window = nfc_str(&s[i..stop.map_or(s.len(), |x| x.0)]);
            assert_eq!(window.chars().next(), nfc_str(&s[i..]).chars().next(), "{s:?} at {k}");
        }
    }
}

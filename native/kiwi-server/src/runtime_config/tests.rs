//! The registry, validation, stored-value decoding and float repr against
//! fixtures produced by the Python code (`golden/gen.py`).

use serde_json::{Map, Value};

use super::*;
use crate::config::tests::{fixture, same, settings_from};

fn num_json(n: Option<Num>) -> Value {
    n.map(Num::to_json).unwrap_or(Value::Null)
}

#[test]
fn registry_matches_python() {
    let fx = fixture("runtime_config_registry.json");
    let entries = fx["entries"].as_array().unwrap();
    assert_eq!(REGISTRY.len(), entries.len());
    let s = settings_from(&Map::new()).unwrap();
    for (spec, e) in REGISTRY.iter().zip(entries) {
        let key = spec.key;
        assert_eq!(key, e["key"], "order");
        assert_eq!(spec.ty.as_str(), e["type"], "{key}");
        assert_eq!(spec.category, e["category"], "{key}");
        assert_eq!(spec.description, e["description"], "{key}");
        assert_eq!(spec.secret, e["secret"], "{key}");
        assert!(same(&num_json(spec.min_value), &e["min_value"]), "{key} min");
        assert!(same(&num_json(spec.max_value), &e["max_value"]), "{key} max");
        let choices = spec.choices.map(|c| Value::from(c.to_vec())).unwrap_or(Value::Null);
        assert_eq!(choices, e["choices"], "{key}");
        assert_eq!(spec.pattern.map(Value::from).unwrap_or(Value::Null), e["pattern"], "{key}");
        assert!(same(&spec.default_value(&s), &e["default"]), "{key} default");
        let from_settings = matches!(spec.default, DefaultValue::Setting(_));
        assert_eq!(from_settings, !e["default_from"].is_null(), "{key} default source");
        assert!(super::spec(key).is_some_and(|x| std::ptr::eq(x, spec)));
    }
    assert!(super::spec("nope").is_none());
}

#[test]
fn registry_defaults_follow_settings() {
    let fx = fixture("runtime_config_registry.json");
    let s = settings_from(fx["custom_env"].as_object().unwrap()).unwrap();
    let want = fx["custom_defaults"].as_object().unwrap();
    for spec in REGISTRY {
        assert!(same(&spec.default_value(&s), &want[spec.key]), "{}", spec.key);
    }
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

fn synthetic(v: &Value) -> &'static Spec {
    let num = |x: &Value| match x {
        Value::Null => None,
        Value::Number(n) if n.is_f64() => Some(Num::Float(n.as_f64().unwrap())),
        Value::Number(n) => Some(Num::Int(n.as_i64().unwrap())),
        other => panic!("bound {other}"),
    };
    let choices = v.get("choices").map(|c| {
        let list: Vec<&'static str> = c.as_array().unwrap().iter().map(|s| leak(s.as_str().unwrap())).collect();
        &*Box::leak(list.into_boxed_slice())
    });
    Box::leak(Box::new(Spec {
        key: leak(v["key"].as_str().unwrap()),
        ty: match v["type"].as_str().unwrap() {
            "str" => Ty::Str,
            "int" => Ty::Int,
            "bool" => Ty::Bool,
            "float" => Ty::Float,
            other => panic!("type {other}"),
        },
        min_value: num(v.get("min_value").unwrap_or(&Value::Null)),
        max_value: num(v.get("max_value").unwrap_or(&Value::Null)),
        choices,
        pattern: v.get("pattern").and_then(Value::as_str).map(leak),
        ..Spec::BASE
    }))
}

#[test]
fn validation_matches_python() {
    let cases = fixture("runtime_config_validation.json");
    let cases = cases.as_array().unwrap();
    assert!(cases.len() > 1000);
    for case in cases {
        let spec = match case.get("spec") {
            Some(syn) => synthetic(syn),
            None => super::spec(case["key"].as_str().unwrap()).unwrap(),
        };
        let got = spec.validate(&case["value"]);
        match (&got, case.get("ok"), case.get("error")) {
            (Ok(v), Some(ok), None) => assert_eq!(encode_value(v), ok.as_str().unwrap(), "{case}"),
            (Err(e), None, Some(msg)) => assert_eq!(e.0, msg.as_str().unwrap(), "{case}"),
            _ => panic!("{case}: rust gave {got:?}"),
        }
    }
}

#[test]
fn stored_values_decode_like_json_loads() {
    for case in fixture("runtime_config_decode.json").as_array().unwrap() {
        let got = decode_stored(case["stored"].as_str().unwrap());
        match case.get("value") {
            Some(want) => assert!(got.as_ref().is_some_and(|g| same(g, want)), "{case}: {got:?}"),
            None => assert!(got.is_none(), "{case}: {got:?}"),
        }
    }
}

#[test]
fn float_repr_matches_python() {
    for r in fixture("float_repr.json").as_array().unwrap() {
        let r = r.as_str().unwrap();
        assert_eq!(py_float_repr(r.parse().unwrap()), r);
    }
}

#[test]
fn python_reprs() {
    assert_eq!(py_str_repr("a"), "'a'");
    assert_eq!(py_str_repr("it's"), "\"it's\"");
    assert_eq!(py_str_repr("both ' \""), "'both \\' \"'");
    assert_eq!(py_str_repr("\u{a0}\u{1}\u{e9}"), "'\\xa0\\x01\u{e9}'");
    assert_eq!(py_float_repr(f64::INFINITY), "Infinity");
    let v = serde_json::json!({"a": [1, 2.0, "\u{e9}"]});
    assert_eq!(encode_value(&v), "{\"a\": [1, 2.0, \"\\u00e9\"]}");
}

#[test]
fn python_patterns_translate() {
    assert_eq!(py_pattern(r"\s*[^\s]x\Z"), r"\A(?:[\s\x1C-\x1F]*[^\s\x1C-\x1F]x\z)\z");
    assert_eq!(py_pattern(r"[]a[&~]"), r"\A(?:[\]a\[\&\~])\z");
    assert!(py_fullmatch(r"[]a[&~]+", "]&~[a"));
    assert!(py_fullmatch(r"\s*[A-Z]{2}\s*", "\u{1c}FR "));
    assert!(!py_fullmatch(r"[A-Z]{2}", "FR\n"));
}

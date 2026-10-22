//! Settings against fixtures produced by the Python code (`golden/gen.py`).

use serde_json::{Map, Value};

use super::Settings;

pub(crate) fn fixture(name: &str) -> Value {
    let path = format!("{}/golden/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

pub(crate) fn settings_from(env: &Map<String, Value>) -> anyhow::Result<Settings> {
    let mut vars: Vec<(String, String)> = vec![("PY_UPSTREAM".into(), "http://py:8000/".into())];
    vars.extend(env.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned())));
    Settings::from_vars(vars)
}

/// Exact JSON equality, keeping int vs float apart; `{"$float": ..}` marks a
/// non-finite Python float, which a JSON dump writes as null.
pub(crate) fn same(got: &Value, want: &Value) -> bool {
    if let Some(f) = want.get("$float") {
        return got.is_null() && f.is_string();
    }
    match (got, want) {
        (Value::Number(a), Value::Number(b)) => {
            a.is_f64() == b.is_f64() && a.as_f64().map(f64::to_bits) == b.as_f64().map(f64::to_bits)
        }
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y)),
        _ => got == want,
    }
}

fn dump(s: &Settings) -> Map<String, Value> {
    let mut m = s.fields_json();
    m.insert("github_oauth_enabled".into(), s.github_oauth_enabled().into());
    m.insert("discord_oauth_enabled".into(), s.discord_oauth_enabled().into());
    m.insert("discord_install_link".into(), s.discord_install_link().into());
    m.insert("postgres_enabled".into(), s.postgres_enabled().into());
    m.insert("email_enabled".into(), s.email_enabled().into());
    m.insert("asset_url".into(), s.asset_url().into());
    m
}

#[test]
fn defaults_match_python() {
    let fx = fixture("settings.json");
    let want = fx["defaults"].as_object().unwrap();
    let s = settings_from(&Map::new()).unwrap();
    let got = dump(&s);
    let python_fields: Vec<&str> = want.keys().map(String::as_str).take(Settings::FIELDS.len()).collect();
    assert_eq!(Settings::FIELDS, python_fields.as_slice(), "field list/order");
    assert_eq!(got.len(), want.len());
    for (k, w) in want {
        assert!(same(&got[k], w), "{k}: rust {} vs python {w}", got[k]);
    }
    assert_eq!(s.py_upstream, "http://py:8000");
    assert_eq!(s.bind, "0.0.0.0:8000");
}

#[test]
fn env_cases_match_python() {
    let fx = fixture("settings.json");
    let defaults = fx["defaults"].as_object().unwrap();
    for case in fx["cases"].as_array().unwrap() {
        let env = case["env"].as_object().unwrap();
        let result = settings_from(env);
        if case.get("error").is_some() {
            assert!(result.is_err(), "{env:?} should be rejected");
            continue;
        }
        let s = result.unwrap_or_else(|e| panic!("{env:?}: {e:#}"));
        let got = dump(&s);
        let changed = case["changed"].as_object().unwrap();
        for (k, d) in defaults {
            let w = changed.get(k).unwrap_or(d);
            assert!(same(&got[k], w), "{env:?} {k}: rust {} vs python {w}", got[k]);
        }
        if let Some(f) = changed.get("cheaters_z_threshold").and_then(|v| v.get("$float")) {
            let v = s.cheaters_z_threshold;
            match f.as_str().unwrap() {
                "inf" => assert_eq!(v, f64::INFINITY),
                "nan" => assert!(v.is_nan()),
                other => panic!("unexpected {other}"),
            }
        }
    }
}

#[test]
fn py_upstream_is_required() {
    assert!(Settings::from_vars(Vec::<(String, String)>::new()).is_err());
    assert!(Settings::from_vars([("PY_UPSTREAM", "")]).is_err());
}

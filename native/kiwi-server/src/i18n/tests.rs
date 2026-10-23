//! Translations against `testdata/discord/golden.json` (from
//! `native/port/parity/gen_discord_fixtures.py`).

use serde_json::Value;

use super::{LANGS, Lang, discord_locale_to_lang, norm, py_format};

fn golden() -> &'static Value {
    crate::discord::tests::golden()
}

#[test]
fn t_matches_python() {
    for c in golden()["i18n"]["t"].as_array().unwrap() {
        let text = c["text"].as_str().unwrap();
        let l = Lang::new(c["lang"].as_str());
        let got = match c["fmt"].as_object() {
            None => l.t(text),
            Some(m) => {
                let args: Vec<(&str, &str)> = m.iter().map(|(k, v)| (k.as_str(), v.as_str().unwrap())).collect();
                l.f(text, &args)
            }
        };
        assert_eq!(got, c["out"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn format_matches_python() {
    for c in golden()["i18n"]["format"].as_array().unwrap() {
        let args: Vec<(&str, &str)> =
            c["kw"].as_object().unwrap().iter().map(|(k, v)| (k.as_str(), v.as_str().unwrap())).collect();
        let got = py_format(c["fmt"].as_str().unwrap(), &args).ok();
        assert_eq!(got.as_deref(), c["out"].as_str(), "{c}");
    }
}

#[test]
fn locales_norm_and_langs() {
    for c in golden()["i18n"]["locales"].as_array().unwrap() {
        assert_eq!(discord_locale_to_lang(c[0].as_str()), c[1].as_str().unwrap(), "{c}");
    }
    for c in golden()["i18n"]["norm"].as_array().unwrap() {
        assert_eq!(norm(c[0].as_str().unwrap()), c[1].as_str().unwrap());
    }
    let langs: Vec<(String, String)> = golden()["i18n"]["langs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p[0].as_str().unwrap().into(), p[1].as_str().unwrap().into()))
        .collect();
    let ours: Vec<(String, String)> = LANGS.iter().map(|(c, n)| ((*c).into(), (*n).into())).collect();
    assert_eq!(ours, langs);
}

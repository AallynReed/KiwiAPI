//! `substitute`, `parse_color` and `render_template` against Python
//! (`testdata/webhooks/golden.json` section `templates`).

use serde_json::{Map, Value};

use super::{EmbedTemplate, parse_color, render_template, substitute};
use crate::{
    outbound::testkit::{arr, golden, s},
    validation::{BodyInput, Field, FromBody, validate},
};

struct Body(EmbedTemplate);

impl FromBody for Body {
    fn from_body(body: Field<'_>) -> Option<Self> {
        Some(Body(EmbedTemplate::validate(body)))
    }
}

/// `EmbedTemplate(**kwargs)`.
pub fn template(v: &Value) -> EmbedTemplate {
    match validate::<Body>(&BodyInput::Json(v.clone())) {
        Ok(Body(t)) => t,
        Err(e) => panic!("template {v}: {e}"),
    }
}

pub fn ctx() -> Map<String, Value> {
    serde_json::from_str(
        r#"{"name": "Racing Challenge", "ends_at": 1718201200, "f": 1718201200.9, "s": " 12 ", "bad": "x1", "b": true,
            "none": null, "lst": [1, "a", null, 2.5, {"k": false}], "d": {"a": 1}, "fl": 2.5, "big": 12345678901234567890,
            "neg": -3, "u": "ünï ✓"}"#,
    )
    .unwrap()
}

#[test]
fn templates_match_python() {
    let ctx = ctx();
    let mut bad = Vec::new();
    let cases = arr(golden(), "templates");
    for case in cases {
        match s(&case["kind"]) {
            "substitute" => {
                let got = substitute(case["text"].as_str(), &ctx);
                if got != s(&case["out"]) {
                    bad.push(format!("substitute {}: {got:?} want {}", case["text"], case["out"]));
                }
            }
            "color" => {
                let got = parse_color(case["text"].as_str()).map(|c| c as i64);
                if got != case["out"].as_i64() {
                    bad.push(format!("color {}: {got:?} want {}", case["text"], case["out"]));
                }
            }
            _ => {
                let t = template(&case["template"]);
                let (embed, content) =
                    render_template(&t, &ctx, case.get("default_image_url").filter(|v| !v.is_null()));
                let got = serde_json::to_string(&Value::Object(embed)).unwrap();
                let want = serde_json::to_string(&case["embed"]).unwrap();
                if got != want {
                    bad.push(format!("render {}:\n  got  {got}\n  want {want}", case["template"]));
                }
                if content.as_deref() != case["content"].as_str() {
                    bad.push(format!("content {}: {content:?} want {}", case["template"], case["content"]));
                }
            }
        }
    }
    assert!(cases.len() > 50, "template cases missing");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn template_round_trips_through_bson() {
    let t = template(&serde_json::json!({
        "enabled": true, "title": "x", "fields": [{"name": "a", "value": "b", "inline": false}], "color": "#fff000",
        "image_design_id": "abc",
    }));
    assert_eq!(EmbedTemplate::from_bson(&t.to_bson()), t);
    let keys: Vec<String> = t.to_bson().keys().cloned().collect();
    let dumped: Vec<String> = t.to_value().as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, dumped);
}

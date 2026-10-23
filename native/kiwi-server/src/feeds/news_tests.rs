//! The news loop's parse against Python: `golden.json` (gen_feeds_fixtures.py,
//! the saved trovegame.com feed) and `news_cases.json` (gen_news_fixtures.py).

use kiwi_core::feeds::news::{FeedError, MailDate, NewsItem, parse_feed, parsedate_to_datetime, strip_html, truncate};
use serde_json::{Value, json};

fn load(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture json")
}

fn golden() -> Value {
    load(include_str!("../../testdata/feeds/golden.json"))
}

fn cases() -> Value {
    load(include_str!("../../testdata/feeds/news_cases.json"))
}

fn item_json(it: &NewsItem, published: Value) -> Value {
    json!({
        "url": it.url,
        "title": it.title,
        "author": it.author,
        "summary": it.summary,
        "category": it.category,
        "categories": it.categories,
        "image": it.image,
        "published_at": published,
    })
}

fn iso(ts: i64) -> String {
    crate::dt::isoformat(&chrono::DateTime::from_timestamp(ts, 0).unwrap())
}

fn naive_iso(d: &MailDate) -> String {
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", d.year, d.month, d.day, d.hour, d.minute, d.second)
}

#[test]
fn strip_and_truncate_match_python() {
    let g = golden();
    for row in g["strip_html"].as_array().unwrap() {
        assert_eq!(strip_html(row[0].as_str().unwrap()), row[1].as_str().unwrap(), "{row}");
    }
    for row in g["truncate"].as_array().unwrap() {
        let n = row[1].as_u64().unwrap() as usize;
        assert_eq!(truncate(row[0].as_str().unwrap(), n), row[2].as_str().unwrap(), "{row}");
    }
}

fn check_date(input: &str, want: &Value, ok_as_iso: bool) {
    let got = parsedate_to_datetime(kiwi_core::pystr::strip(input));
    if let Some(n) = want.get("naive") {
        let d = got.unwrap_or_else(|| panic!("{input:?} should parse"));
        assert_eq!(d.offset, None, "{input:?}");
        assert_eq!(naive_iso(&d), n.as_str().unwrap(), "{input:?}");
    } else if let Some(ok) = want.get("ok") {
        let d = got.unwrap_or_else(|| panic!("{input:?} should parse"));
        assert!(d.offset.is_some(), "{input:?}");
        let ts = d.utc_timestamp().unwrap();
        if ok_as_iso {
            assert_eq!(iso(ts), ok.as_str().unwrap(), "{input:?}");
        } else {
            assert_eq!(ts, ok.as_i64().unwrap(), "{input:?}");
        }
    } else {
        assert_eq!(got, None, "{input:?}");
    }
}

#[test]
fn parsedate_matches_python() {
    for row in golden()["rfc2822"].as_array().unwrap() {
        check_date(row[0].as_str().unwrap(), &row[1], true);
    }
    for row in cases()["dates"].as_array().unwrap() {
        check_date(row[0].as_str().unwrap(), &row[1], false);
    }
}

#[test]
fn saved_feed_parses_like_python() {
    let xml = include_str!("../../testdata/feeds/raw/news_feed.xml");
    let items = parse_feed(xml).unwrap();
    let want = golden()["news_feed"].as_array().unwrap().clone();
    assert_eq!(items.len(), want.len());
    for (it, w) in items.iter().zip(&want) {
        let published = it.published_at.map_or(Value::Null, |ts| Value::String(iso(ts)));
        assert_eq!(item_json(it, published), *w);
    }
}

#[test]
fn feed_corner_cases_match_python() {
    for case in cases()["feeds"].as_array().unwrap() {
        let xml = case["xml"].as_str().unwrap();
        match (parse_feed(xml), case.get("error").and_then(Value::as_str)) {
            (Ok(items), None) => {
                let got: Vec<Value> = items.iter().map(|it| item_json(it, json!(it.published_at))).collect();
                assert_eq!(Value::Array(got), case["items"], "{xml}");
            }
            (Err(FeedError::Xml(_)), Some("ParseError")) => {}
            (Err(FeedError::Raised(msg)), Some(name)) => assert!(msg.starts_with(name), "{xml}: {msg}"),
            (got, want) => panic!("{xml}: got {got:?}, Python {want:?}"),
        }
    }
}

#[test]
fn upserted_fields_in_model_order() {
    let it = NewsItem {
        url: "u".into(),
        title: "t".into(),
        author: "a".into(),
        summary: "s".into(),
        category: "c".into(),
        categories: vec!["c".into(), "d".into()],
        image: None,
        published_at: Some(1_792_510_679),
    };
    let d = super::news::fields(&it).unwrap();
    let keys: Vec<&str> = d.keys().map(String::as_str).collect();
    assert_eq!(keys, ["title", "author", "summary", "category", "categories", "image", "published_at"]);
    assert_eq!(d.get_datetime("published_at").unwrap().timestamp_millis(), 1_792_510_679_000);
    assert_eq!(d.get("image"), Some(&mongodb::bson::Bson::Null));
}

/// What `upsert_by` inserts (`url`, the fields, the two stamps) against
/// Beanie's encoding of the same `TroveNews` (producers golden).
#[test]
fn inserted_news_document_matches_beanie() {
    use mongodb::bson::{Bson, doc};
    let golden: Value = load(include_str!("../../testdata/producers/golden.json"));
    let it = NewsItem {
        url: "https://trovegame.com/x".into(),
        title: "T".into(),
        author: "Team Trove".into(),
        summary: "S".into(),
        category: "News".into(),
        categories: vec!["News".into(), "Events".into()],
        image: None,
        published_at: Some(1_792_666_800),
    };
    let stamp = Bson::DateTime(mongodb::bson::DateTime::from_millis(1_792_666_800_123));
    let mut d = doc! { "url": &it.url };
    d.extend(super::news::fields(&it).unwrap());
    d.insert("published_at", stamp.clone());
    d.insert("first_seen_at", stamp.clone());
    d.insert("updated_at", stamp);
    assert_eq!(Bson::Document(d).into_canonical_extjson(), golden["documents"]["news"]);
}

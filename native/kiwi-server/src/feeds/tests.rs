//! Golden checks against fixtures captured from the Python code
//! (`native/port/parity/gen_feeds_fixtures.py` -> `testdata/feeds/`).

use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use kiwi_core::feeds::{
    soup::{AttrValue, Node, Soup},
    unescape::unescape,
};
use mongodb::bson::{Bson, Document};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    events::parse_events,
    image::{ImageError, hdslb_target, status_message},
    pyval,
    relay::{self, BiliConfig, YtConfig},
    routes::{dedupe_news, feed_body, news_list},
    store::{self, News},
};
use crate::{dt, pyjson};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/feeds/golden.json")).expect("golden.json"))
}

fn html_fixtures() -> &'static Value {
    static H: OnceLock<Value> = OnceLock::new();
    H.get_or_init(|| serde_json::from_str(include_str!("../../testdata/feeds/html.json")).expect("html.json"))
}

const PAGE: &str = include_str!("../../testdata/feeds/raw/bilibili_search.html");
const BILI_REWRITE: (&str, &str) = ("//www.bilibili.com/zh-Hans/video/", "//www.bilibili.com/video/");

fn arr<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key].as_array().unwrap_or_else(|| panic!("fixture section {key}"))
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn utc(v: &Value) -> DateTime<Utc> {
    dt::parse_utc(s(v)).expect("datetime")
}

fn dump(soup: &Soup, node: &Node) -> Value {
    match node {
        Node::Text(kind, text) => json!([kind.name(), text]),
        Node::Tag(id) => {
            let t = soup.tag(*id);
            let attrs: Vec<Value> = t
                .attrs
                .iter()
                .map(|(k, v)| match v {
                    AttrValue::Str(s) => json!([k, s]),
                    AttrValue::List(l) => json!([k, l]),
                })
                .collect();
            let children: Vec<Value> = t.children.iter().map(|c| dump(soup, c)).collect();
            json!([t.name, attrs, children])
        }
    }
}

fn dump_root(soup: &Soup) -> Value {
    Value::Array(soup.tag(0).children.iter().map(|c| dump(soup, c)).collect())
}

#[test]
fn unescape_matches_python() {
    for case in arr(golden(), "unescape") {
        assert_eq!(unescape(s(&case[0])), s(&case[1]), "{case}");
    }
}

#[test]
fn soup_trees_match_bs4() {
    for case in arr(html_fixtures(), "trees") {
        let soup = Soup::parse(s(&case["html"])).expect("parses");
        assert_eq!(dump_root(&soup), case["tree"], "{}", case["html"]);
    }
    for case in arr(html_fixtures(), "errors") {
        assert_eq!(Soup::parse(s(&case["html"])).is_err(), !case["error"].is_null(), "{}", case["html"]);
    }
}

#[test]
fn bilibili_page_tree_and_cards_match_bs4() {
    for (name, html) in
        [("page_raw", PAGE.to_owned()), ("page_rewritten", PAGE.replace(BILI_REWRITE.0, BILI_REWRITE.1))]
    {
        let want = &html_fixtures()[name];
        let soup = Soup::parse(&html).expect("page parses");
        let dumped = pyjson::dumps(&dump_root(&soup));
        let sha = hex::encode(Sha256::digest(dumped.as_bytes()));
        assert_eq!(sha, s(&want["tree_sha256"]), "{name} tree");
        let cards = kiwi_core::feeds::bilibili::cards(&html).expect("cards");
        let got: Vec<Value> = cards
            .into_iter()
            .map(|c| json!({"href": c.href, "title": c.title, "author": c.author, "date": c.date, "srcset": c.srcset}))
            .collect();
        assert_eq!(Value::Array(got), want["cards"], "{name} cards");
    }
}

fn events_json(events: &[super::events::ParsedEvent]) -> Value {
    Value::Array(
        events
            .iter()
            .map(|e| {
                json!({
                    "event_id": e.event_id, "name": e.name, "url": e.url, "category": e.category,
                    "image": e.image, "icon": e.icon, "lookup": e.lookup,
                    "starts_at": e.starts_at as i64, "ends_at": e.ends_at as i64,
                })
            })
            .collect(),
    )
}

#[test]
fn events_parse_matches_python() {
    let g = golden();
    let real: Value = serde_json::from_str(include_str!("../../testdata/feeds/raw/trovesaurus_calendar.json")).unwrap();
    assert_eq!(events_json(&parse_events(&real).unwrap()), g["events_real"]);
    for case in arr(g, "events_cases") {
        let got = parse_events(&Value::Array(vec![case["in"].clone()])).unwrap();
        assert_eq!(events_json(&got), case["out"], "{}", case["in"]);
    }
    for case in arr(g, "events_errors") {
        assert!(parse_events(&case["in"]).is_err(), "{}", case["in"]);
    }
}

#[test]
fn feed_normalizers_match_python() {
    let g = golden();
    for case in arr(g, "twitch_normalize") {
        match relay::normalize_twitch(&case["in"]) {
            Ok(out) => assert_eq!(Value::Array(out), case["out"], "{}", case["in"]),
            Err(_) => assert!(case.get("error").is_some(), "{}", case["in"]),
        }
    }
    for case in arr(g, "videos_normalize") {
        assert_eq!(Value::Array(relay::normalize_videos(&case["in"])), case["out"], "{}", case["in"]);
    }
    for case in arr(g, "has_term") {
        assert_eq!(relay::has_term(s(&case[0]), s(&case[1])), case[2].as_bool().unwrap(), "{case}");
    }
    for case in arr(g, "iso8601_seconds") {
        let got = relay::iso8601_seconds(Some(&case[0])).unwrap().map(|v| v as i64);
        assert_eq!(json!(got), case[1], "{case}");
    }
    for case in arr(g, "csv_set") {
        assert_eq!(json!(relay::csv_set(&case[0]).unwrap()), case[1], "{case}");
    }
    for case in arr(g, "lower") {
        assert_eq!(s(&case[0]).to_lowercase(), s(&case[1]), "{case}");
    }
}

#[test]
fn bilibili_dates_match_python() {
    let dates = &golden()["bilibili_dates"];
    let now = utc(&dates["now"]);
    for case in arr(dates, "cases") {
        let got = match relay::parse_bilibili_date(s(&case[0]), now) {
            Ok(t) => dt::isoformat(&t),
            Err(_) => "error:OverflowError".into(),
        };
        assert_eq!(got, s(&case[1]), "{case}");
    }
}

#[test]
fn youtube_curation_matches_python() {
    for sc in arr(golden(), "youtube") {
        let st = &sc["settings"];
        let cfg = YtConfig {
            query: st["feeds_youtube_query"].clone(),
            excluded_channels: relay::csv_set(&st["feeds_youtube_excluded_channels"]).unwrap(),
            exclude_terms: relay::csv_set(&st["feeds_youtube_excluded_title_terms"]).unwrap(),
            require_terms: relay::csv_set(&st["feeds_youtube_require_terms"]).unwrap(),
            relevance_terms: relay::csv_set(&st["feeds_youtube_relevance_terms"]).unwrap(),
            gaming_cat: s(&st["feeds_youtube_video_category_id"]).trim().to_owned(),
            cutoff_days: st["feeds_video_cutoff_days"].as_i64().unwrap().into(),
            per_channel_max: st["feeds_per_channel_max"].as_i64().unwrap().into(),
            max_items: st["feeds_max_items"].as_i64().unwrap().into(),
        };
        let calls = arr(sc, "calls");
        let published_after = relay::iso_z(&(utc(&sc["now"]) - chrono::TimeDelta::days(90)));
        assert_eq!(calls[1]["params"]["publishedAfter"], json!(published_after));
        let snippets = relay::search_snippets(&sc["search"]).unwrap();
        if snippets.is_empty() {
            assert_eq!(calls.len(), 2, "{}", sc["name"]);
            assert_eq!(sc["out"], json!([]));
            continue;
        }
        let ids: Vec<&str> = snippets.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(calls[2]["params"]["id"], json!(ids.join(",")), "{}", sc["name"]);
        let got = relay::curate_youtube(&snippets, &sc["videos"], &cfg).unwrap();
        assert_eq!(Value::Array(got), sc["out"], "{}", sc["name"]);
    }
}

#[test]
fn bilibili_scrape_matches_python() {
    for sc in arr(golden(), "bilibili") {
        let html = match s(&sc["name"]) {
            "raw" => PAGE.to_owned(),
            "rewritten" | "rewritten_wide" => PAGE.replace(BILI_REWRITE.0, BILI_REWRITE.1),
            _ => s(&sc["html"]).to_owned(),
        };
        let st = &sc["settings"];
        let cfg = BiliConfig {
            cutoff_days: st["feeds_video_cutoff_days"].as_i64().unwrap().into(),
            per_channel_max: st["feeds_per_channel_max"].as_i64().unwrap().into(),
            max_items: st["feeds_max_items"].as_i64().unwrap().into(),
        };
        let got = relay::bilibili_videos(&html, utc(&sc["now"]), cfg).unwrap();
        assert_eq!(Value::Array(got), sc["out"], "{}", sc["name"]);
    }
}

#[test]
fn hdslb_target_matches_httpx() {
    for case in arr(golden(), "hdslb_target") {
        let url = s(&case["in"]);
        match (hdslb_target(url), case.get("ok")) {
            (Ok(t), Some(ok)) => {
                assert_eq!(t.to_string(), s(ok), "{url}");
                assert_eq!(t.raw_path(), s(&case["raw_path"]), "{url}");
                assert_eq!(t.display_host().unwrap(), s(&case["host"]), "{url}");
                assert_eq!(json!(t.port.map(|p| p as i64)), case["port"], "{url}");
                assert_eq!(t.userinfo, s(&case["userinfo"]), "{url}");
            }
            (Err(ImageError::BadRequest(m)), None) => {
                assert_eq!(s(&case["error"]), "ValueError", "{url}");
                assert_eq!(m, s(&case["msg"]), "{url}");
            }
            (Err(ImageError::InvalidUrl(m)), None) => {
                assert_eq!(s(&case["error"]), "InvalidURL", "{url}");
                assert_eq!(m, s(&case["msg"]), "{url}");
            }
            (got, _) => panic!("{url}: {got:?} vs {case}"),
        }
    }
}

#[test]
fn status_messages_match_httpx() {
    for case in arr(golden(), "status_errors") {
        let got = status_message(
            case["status"].as_u64().unwrap() as u16,
            s(&case["reason"]),
            case["location"].as_str(),
            s(&case["url"]),
        );
        assert_eq!(json!(got), case["message"], "{case}");
    }
}

fn news_from(v: &Value) -> News {
    News {
        url: s(&v["url"]).into(),
        title: s(&v["title"]).into(),
        author: s(&v["author"]).into(),
        summary: s(&v["summary"]).into(),
        category: s(&v["category"]).into(),
        categories: v["categories"].as_array().unwrap().iter().map(|c| s(c).to_owned()).collect(),
        image: v["image"].as_str().map(str::to_owned),
        published_at: v["published_at"].as_str().map(|t| dt::parse_utc(t).unwrap()),
    }
}

#[test]
fn route_bodies_match_fastapi() {
    let g = golden();
    let docs: Vec<News> = arr(g, "news_docs").iter().map(news_from).collect();
    for case in arr(g, "news_latest") {
        let limit = case["limit"].as_u64().unwrap() as usize;
        let fetched = docs.iter().take(limit * 3).cloned().collect();
        let body = pyjson::pydantic(&news_list(&dedupe_news(fetched, limit)));
        assert_eq!(body, s(&case["body"]), "limit {limit}");
    }
    let hist = &g["news_history"];
    let items: Vec<Value> = docs.iter().take(3).map(News::item).collect();
    let body = json!({"items": items, "count": 3, "total": hist["total"]});
    assert_eq!(pyjson::pydantic(&body), s(&hist["body"]));

    let ev = &g["events_items"];
    let now = ev["now"].as_i64().unwrap();
    let items: Vec<Value> = arr(ev, "events")
        .iter()
        .map(|e| {
            let d = match pyval::to_bson(e).unwrap() {
                Bson::Document(d) => d,
                _ => unreachable!(),
            };
            store::Event::from_doc(&d).item(now)
        })
        .collect();
    let count = items.len();
    assert_eq!(pyjson::pydantic(&json!({"items": items, "count": count})), s(&ev["body"]));

    let fb = &g["feed_bodies"];
    let docs = |key: &str| -> Vec<Document> {
        arr(fb, key)
            .iter()
            .map(|i| match pyval::to_bson(i).unwrap() {
                Bson::Document(d) => d,
                _ => unreachable!(),
            })
            .collect()
    };
    let fetched = utc(&fb["fetched_at"]);
    let twitch = feed_body(Some((docs("twitch_items"), fetched)), store::twitch_stream);
    assert_eq!(pyjson::pydantic(&twitch), s(&fb["twitch_body"]));
    let videos = feed_body(Some((docs("video_items"), fetched)), store::video);
    let mut want: Value = serde_json::from_str(s(&fb["videos_body"])).unwrap();
    want["fetched_at"] = json!(dt::pydantic(&fetched));
    assert_eq!(pyjson::pydantic(&videos), pyjson::pydantic(&want));
    assert_eq!(pyjson::pydantic(&feed_body(None, store::video)), s(&fb["empty_body"]));
}

#[test]
fn stored_items_use_python_int_widths() {
    let items = vec![json!({"viewers": 12, "big": 4_000_000_000_i64, "f": 1.5, "n": null, "b": true})];
    let Bson::Array(a) = store::items_bson(&items).unwrap() else { panic!() };
    let Bson::Document(d) = &a[0] else { panic!() };
    assert_eq!(d.get("viewers"), Some(&Bson::Int32(12)));
    assert_eq!(d.get("big"), Some(&Bson::Int64(4_000_000_000)));
    assert_eq!(d.get("f"), Some(&Bson::Double(1.5)));
    assert_eq!(d.get("n"), Some(&Bson::Null));
    assert_eq!(d.get("b"), Some(&Bson::Boolean(true)));
}

#[tokio::test]
#[ignore = "reaches hdslb.com, trovesaurus.com and search.bilibili.com"]
async fn live_upstreams() {
    let ok = "https://i2.hdslb.com/bfs/archive/29a70a5bd0c85845ab9c0c01be985189576e6ed9.jpg@672w_378h_1c_!web-search-common-cover.webp";
    let (bytes, ctype) = super::image::fetch(ok).await.unwrap();
    assert_eq!(ctype, "image/webp");
    assert!(bytes.len() > 1000);
    let missing = "https://i0.hdslb.com/bfs/archive/does-not-exist-123.jpg";
    assert_eq!(
        super::image::fetch(missing).await,
        Err(ImageError::Upstream(format!(
            "Client error '404 Not Found' for url '{missing}'\nFor more information check: \
             https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/404"
        )))
    );
    let clients = super::http::clients();
    let resp = clients.fetch.get("https://trovesaurus.com/calendar/feed").send().await.unwrap();
    let raw = super::http::json(super::http::check_status(resp).unwrap()).await.unwrap();
    parse_events(&raw).unwrap();
    let resp =
        clients.bilibili.get("https://search.bilibili.com/all").query(&[("keyword", "trove")]).send().await.unwrap();
    let html = super::http::check_status(resp).unwrap().text().await.unwrap();
    let cfg = BiliConfig { cutoff_days: 90, per_channel_max: 3, max_items: 10 };
    relay::bilibili_videos(&html, Utc::now(), cfg).unwrap();
}

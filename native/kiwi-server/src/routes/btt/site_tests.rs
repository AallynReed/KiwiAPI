//! `/site/btt/*`, `/site/changelog` and the refresh button against
//! `testdata/site_live/{btt,site_changelog,httpx_errors,refresh}.json`.

use serde_json::Value;

use super::*;
use crate::site_live::tests::{doc_of, fixture};

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn releases(f: &Value) -> Vec<Release> {
    f["releases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| Release::from_doc(&doc_of(r, &["published_at", "fetched_at"])).unwrap())
        .collect()
}

#[test]
fn btt_bodies_match_python() {
    let f = fixture("btt.json");
    let all = releases(&f);
    for case in f["latest"].as_array().unwrap() {
        let asked = s(&case["channel"]);
        let channel = if channel_prerelease(asked).is_some() { asked } else { "release" };
        let prerelease = channel_prerelease(channel).unwrap();
        let chan: Vec<Release> = releases(&f).into_iter().filter(|r| r.prerelease == prerelease).collect();
        let body = pyjson::dumps(&latest_body(channel, &chan).unwrap());
        assert_eq!(body, s(&case["body"]), "latest {asked:?}");
        assert_eq!(s(&case["cache_control"]), "public, max-age=180");
    }
    for case in f["lists"].as_array().unwrap() {
        let channel = case["channel"].as_str().filter(|c| channel_prerelease(c).is_some());
        let prerelease = channel.and_then(channel_prerelease);
        let chan: Vec<&Release> = all.iter().filter(|r| prerelease.is_none_or(|p| r.prerelease == p)).collect();
        let (limit, offset) = (case["limit"].as_u64().unwrap() as usize, case["offset"].as_u64().unwrap() as usize);
        let page: Vec<Value> = chan.iter().skip(offset).take(limit).map(|r| release_item(r)).collect();
        let body = json!({"channel": channel, "items": page, "count": page.len(), "total": chan.len()});
        assert_eq!(pyjson::dumps(&body), s(&case["body"]), "list {channel:?}");
    }
    for case in f["changelog"].as_array().unwrap() {
        let doc = case["doc"].as_object().map(|_| doc_of(&case["doc"], &["fetched_at"]));
        assert_eq!(pyjson::dumps(&changelog_body(doc.as_ref()).unwrap()), s(&case["body"]));
    }
}

#[test]
fn site_changelog_matches_python() {
    let st = Settings::default();
    for case in fixture("site_changelog.json").as_array().unwrap() {
        let tags = (!case["tags"].is_null()).then(|| case["tags"].clone());
        let commits = (!case["commits"].is_null()).then(|| case["commits"].clone());
        let got = changelog_payload(&st, tags, commits).unwrap();
        assert_eq!(pyjson::dumps(&got), s(&case["body"]), "{}", case["name"]);
        assert_eq!(pyjson::dumps_spaced(&got), s(&case["cached"]), "{}", case["name"]);
    }
}

#[test]
fn httpx_messages_match_python() {
    for case in fixture("httpx_errors.json").as_array().unwrap() {
        if let Some(code) = case["code"].as_u64() {
            let got = status_message(code as u16, s(&case["url"]), case["location"].as_str());
            assert_eq!(Some(got.as_str()), case["message"].as_str(), "{code}");
        } else {
            let body = s(&case["body"]).as_bytes();
            let got = pyjson::loads(body).err().map(|e| decode_message(body, &e));
            assert_eq!(got.as_deref(), case["message"].as_str(), "{:?}", case["body"]);
        }
    }
}

#[test]
fn refresh_body_matches_python() {
    let f = fixture("btt.json");
    let all = releases(&f);
    for case in fixture("refresh.json").as_array().unwrap() {
        let rel = match case["releases_error"].as_str() {
            Some(e) => Err(e.to_owned()),
            None => Ok(case["releases"].as_u64().unwrap() as usize),
        };
        let chl = match case["changelog_error"].as_str() {
            Some(e) => Err(e.to_owned()),
            None => Ok((
                case["changelog"]["groups"].as_u64().unwrap() as usize,
                case["changelog"]["rate_limited"].as_bool().unwrap(),
            )),
        };
        let mut latest = Vec::new();
        for channel in CHANNELS {
            let prerelease = channel_prerelease(channel).unwrap();
            let chan: Vec<Release> = releases(&f).into_iter().filter(|r| r.prerelease == prerelease).collect();
            for p in PLATFORMS {
                if let Some((r, _)) = walk_latest(&chan, p).unwrap() {
                    latest.push((format!("{channel}/{p}"), r.tag_name.clone()));
                }
            }
        }
        let downloads = case["downloads_ok"]
            .as_bool()
            .unwrap()
            .then(|| all.iter().flat_map(|r| r.assets.iter()).map(|a| download_count(a).unwrap()).sum());
        let got = pyjson::pydantic(&refresh_body(rel, chl, latest, downloads));
        assert_eq!(got, s(&case["body"]), "{}", case["name"]);
    }
}

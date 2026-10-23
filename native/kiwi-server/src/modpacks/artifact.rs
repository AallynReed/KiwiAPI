//! The download half of `build_artifact`: configs beside the packed mods,
//! the `modpack.json` manifest, and the `.tpack` / `.zip` / configs-zip bytes
//! (`zipfile` ZIP_DEFLATED and `tmod.build_tpack`, byte for byte).

use std::fmt::Write;

use serde_json::{Map, Value, json};

use super::{hub, text};
use crate::{blueprints::pack, error::ApiError, pyjson};

/// One `mods` record of the manifest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record {
    pub handle: String,
    pub slug: String,
    pub title: String,
    pub branch: String,
    pub version: Option<String>,
    pub locked: bool,
    pub available: bool,
    pub filename: Option<String>,
    pub config: Option<String>,
}

impl Record {
    fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("handle".into(), self.handle.clone().into());
        m.insert("slug".into(), self.slug.clone().into());
        m.insert("title".into(), self.title.clone().into());
        m.insert("branch".into(), self.branch.clone().into());
        m.insert("version".into(), self.version.clone().into());
        m.insert("locked".into(), self.locked.into());
        m.insert("available".into(), self.available.into());
        if let Some(f) = &self.filename {
            m.insert("filename".into(), f.clone().into());
        }
        if let Some(c) = &self.config {
            m.insert("config".into(), c.clone().into());
        }
        Value::Object(m)
    }
}

/// The pack fields the artifact carries.
pub struct Meta {
    pub title: String,
    pub handle: String,
    pub slug: String,
    pub author: String,
    pub variant: String,
    pub warnings: String,
}

/// `_collect_configs`: first config per (case-insensitive) name wins, and the
/// record named by `by_filename` (the last with that filename) notes it.
pub fn collect_configs(downloadable: &[(String, Vec<u8>)], manifest: &mut [Record]) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (name, data) in downloadable {
        let Some((cfg_name, cfg_data)) = hub::pack_config_for_artifact(data) else { continue };
        let key = text::lower(&cfg_name);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        if let Some(record) = manifest.iter_mut().rev().find(|m| m.filename.as_deref() == Some(name)) {
            record.config = Some(cfg_name.clone());
        }
        out.push((cfg_name, cfg_data));
    }
    out
}

pub fn manifest(meta: &Meta, records: &[Record]) -> Value {
    json!({
        "modpack": {
            "title": meta.title, "handle": meta.handle, "slug": meta.slug,
            "author": meta.author, "variant": meta.variant, "warnings": meta.warnings,
        },
        "mods": records.iter().map(Record::to_json).collect::<Vec<_>>(),
    })
}

/// `json.dumps(value, indent=2)`.
pub fn dumps_indent2(value: &Value) -> String {
    fn go(out: &mut String, v: &Value, level: usize) {
        match v {
            Value::Array(items) if !items.is_empty() => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    out.push_str(if i == 0 { "\n" } else { ",\n" });
                    out.push_str(&"  ".repeat(level + 1));
                    go(out, item, level + 1);
                }
                let _ = write!(out, "\n{}]", "  ".repeat(level));
            }
            Value::Object(map) if !map.is_empty() => {
                out.push('{');
                for (i, (k, item)) in map.iter().enumerate() {
                    out.push_str(if i == 0 { "\n" } else { ",\n" });
                    out.push_str(&"  ".repeat(level + 1));
                    pyjson::write_str(out, k, true);
                    out.push_str(": ");
                    go(out, item, level + 1);
                }
                let _ = write!(out, "\n{}}}", "  ".repeat(level));
            }
            other => out.push_str(&pyjson::dumps_spaced(other)),
        }
    }
    let mut out = String::new();
    go(&mut out, value, 0);
    out
}

/// `(bytes, filename, media type)` for `fmt` (`tpack`, `zip` or `configs`).
pub fn assemble(
    meta: &Meta,
    downloadable: &[(String, Vec<u8>)],
    records: &[Record],
    configs: &[(String, Vec<u8>)],
    fmt: &str,
    when: pack::DateTime6,
) -> Result<(Vec<u8>, String, &'static str), ApiError> {
    let safe = text::safe_filename(&meta.title, "modpack");
    let suffix =
        if meta.variant == super::model::DEFAULT_VARIANT { String::new() } else { format!("-{}", meta.variant) };
    let doc = manifest(meta, records);
    let cfg_entries = configs.iter().map(|(n, d)| (format!("ModCfgs/{n}"), d.clone()));
    match fmt {
        "configs" | "zip" => {
            if fmt == "configs" && configs.is_empty() {
                return Err(ApiError::bad_request("None of the mods in this modpack have settings to configure."));
            }
            let mut files = vec![("modpack.json".to_owned(), dumps_indent2(&doc).into_bytes())];
            if fmt == "zip" {
                files.extend(downloadable.iter().map(|(n, d)| (format!("mods/{n}"), d.clone())));
            }
            files.extend(cfg_entries);
            let blob = pack::zip(&files, when).ok_or_else(ApiError::internal)?;
            let name = if fmt == "zip" { format!("{safe}{suffix}.zip") } else { format!("{safe}{suffix}-configs.zip") };
            Ok((blob, name, "application/zip"))
        }
        _ => {
            let props: Vec<(String, String)> = [
                ("title", meta.title.clone()),
                ("author", meta.author.clone()),
                ("packVersion", "1".to_owned()),
                ("variant", meta.variant.clone()),
                ("notes", meta.warnings.clone()),
                ("manifest", pyjson::dumps_spaced(&doc)),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
            let mut inner: Vec<(String, Vec<u8>)> = downloadable.to_vec();
            inner.extend(cfg_entries);
            let blob = crate::tools::tmod::build(1, &props, &inner, crate::tools::tmod::KIWI_MOD_LOADER, false)
                .map_err(|e| {
                    tracing::error!("modpacks: tpack build failed: {e:?}");
                    ApiError::internal()
                })?;
            Ok((blob, format!("{safe}{suffix}.tpack"), "application/octet-stream"))
        }
    }
}

//! `site/templates` rendered with minijinja the way Starlette's `Jinja2Templates`
//! renders them with Jinja2: markupsafe escaping (`&#34;`, `&#39;`, `/` left
//! alone), Python `str()` of every printed value, Jinja2's `default`, `join`,
//! `tojson`, `urlencode`, the Python methods the templates call, newline
//! normalisation, and a reload when a template changes on disk
//! (`auto_reload`). Rendered pages get their `/static` links versioned.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use minijinja::value::{Rest, ValueKind};
use minijinja::{AutoEscape, Environment, Error, ErrorKind, State, Value};

use crate::middleware::security::StaticVersions;

/// markupsafe `escape()`.
pub fn escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    escape_into(&mut out, s);
    out
}

fn number_str(v: &Value) -> String {
    if v.is_integer() {
        if let Ok(i) = i128::try_from(v.clone()) {
            return i.to_string();
        }
    } else if let Ok(f) = f64::try_from(v.clone()) {
        return crate::pyfloat::repr(f);
    }
    v.to_string()
}

/// Python `str()` of a template value.
pub fn py_str(v: &Value) -> String {
    match v.kind() {
        ValueKind::Undefined => String::new(),
        ValueKind::None => "None".to_owned(),
        ValueKind::Bool => if v.is_true() { "True" } else { "False" }.to_owned(),
        ValueKind::Number => number_str(v),
        ValueKind::String => v.as_str().unwrap_or("").to_owned(),
        ValueKind::Seq | ValueKind::Map => py_repr(v),
        _ => v.to_string(),
    }
}

/// Python `repr()` of a template value.
pub fn py_repr(v: &Value) -> String {
    match v.kind() {
        ValueKind::String => kiwi_core::pystr::repr(v.as_str().unwrap_or("")),
        ValueKind::Seq => {
            let items: Vec<String> = v.try_iter().map(|it| it.map(|x| py_repr(&x)).collect()).unwrap_or_default();
            format!("[{}]", items.join(", "))
        }
        ValueKind::Map => {
            let items: Vec<String> = v
                .try_iter()
                .map(|it| {
                    it.map(|k| format!("{}: {}", py_repr(&k), py_repr(&v.get_item(&k).unwrap_or_default()))).collect()
                })
                .unwrap_or_default();
            format!("{{{}}}", items.join(", "))
        }
        _ => py_str(v),
    }
}

fn autoescaping(state: &State) -> bool {
    !matches!(state.auto_escape(), AutoEscape::None)
}

fn formatter(out: &mut minijinja::Output, state: &State, value: &Value) -> Result<(), Error> {
    let s = py_str(value);
    let res = if autoescaping(state) && !value.is_safe() { out.write_str(&escape(&s)) } else { out.write_str(&s) };
    res.map_err(|_| Error::new(ErrorKind::WriteFailure, "write failed"))
}

fn f_default(value: Value, args: Rest<Value>) -> Value {
    let other = args.first().cloned().unwrap_or_else(|| Value::from(""));
    let boolean = args.get(1).is_some_and(Value::is_true);
    if value.is_undefined() || (boolean && !value.is_true()) { other } else { value }
}

fn f_join(state: &State, value: Value, d: Option<Value>) -> Result<Value, Error> {
    let d = d.unwrap_or_else(|| Value::from(""));
    let items: Vec<Value> = if value.kind() == ValueKind::String {
        py_str(&value).chars().map(|c| Value::from(c.to_string())).collect()
    } else {
        value.try_iter()?.collect()
    };
    if autoescaping(state) && !d.is_safe() && items.iter().any(Value::is_safe) {
        let parts: Vec<String> =
            items.iter().map(|i| if i.is_safe() { py_str(i) } else { escape(&py_str(i)) }).collect();
        return Ok(Value::from_safe_string(parts.join(&escape(&py_str(&d)))));
    }
    let parts: Vec<String> = items.iter().map(py_str).collect();
    let joined = parts.join(&py_str(&d));
    Ok(if d.is_safe() && autoescaping(state) { Value::from_safe_string(joined) } else { Value::from(joined) })
}

fn sort_keys(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => {
            let mut items: Vec<(String, serde_json::Value)> = m.into_iter().map(|(k, v)| (k, sort_keys(v))).collect();
            items.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(items.into_iter().collect())
        }
        serde_json::Value::Array(a) => serde_json::Value::Array(a.into_iter().map(sort_keys).collect()),
        other => other,
    }
}

/// Jinja2 `htmlsafe_json_dumps`: `json.dumps(obj, sort_keys=True)` with
/// `< > & '` as `\u` escapes.
pub fn htmlsafe_json(v: &serde_json::Value) -> String {
    crate::pyjson::dumps_spaced(&sort_keys(v.clone()))
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\'', "\\u0027")
}

fn f_tojson(value: Value) -> Result<Value, Error> {
    let json = serde_json::to_value(&value).map_err(|e| Error::new(ErrorKind::BadSerialization, e.to_string()))?;
    Ok(Value::from_safe_string(htmlsafe_json(&json)))
}

fn f_urlencode(value: Value) -> Result<Value, Error> {
    let q = |s: &str, qs: bool| {
        let out = crate::middleware::pyurl::quote(s, if qs { "" } else { "/" });
        if qs { out.replace("%20", "+") } else { out }
    };
    if value.kind() == ValueKind::Map {
        let mut parts = Vec::new();
        for k in value.try_iter()? {
            let v = value.get_item(&k)?;
            parts.push(format!("{}={}", q(&py_str(&k), true), q(&py_str(&v), true)));
        }
        return Ok(Value::from(parts.join("&")));
    }
    Ok(Value::from(q(&py_str(&value), false)))
}

fn f_length(value: Value) -> Result<Value, Error> {
    Ok(Value::from(match value.kind() {
        ValueKind::Undefined => 0,
        ValueKind::String => py_str(&value).chars().count(),
        _ => value.len().ok_or_else(|| Error::new(ErrorKind::InvalidOperation, "object has no len()"))?,
    }))
}

fn f_replace(value: Value, old: Value, new: Value) -> Value {
    Value::from(py_str(&value).replace(&py_str(&old), &py_str(&new)))
}

fn py_method(_state: &State, value: &Value, method: &str, args: &[Value]) -> Result<Value, Error> {
    let unknown = || Error::new(ErrorKind::UnknownMethod, format!("no method {method}"));
    match value.kind() {
        ValueKind::String => {
            let s = py_str(value);
            let affix = |f: &dyn Fn(&str) -> bool| -> Result<Value, Error> {
                let arg = args.first().ok_or_else(unknown)?;
                if arg.kind() == ValueKind::Seq {
                    Ok(Value::from(arg.try_iter()?.any(|a| f(&py_str(&a)))))
                } else {
                    Ok(Value::from(f(&py_str(arg))))
                }
            };
            match method {
                "startswith" => affix(&|p| s.starts_with(p)),
                "endswith" => affix(&|p| s.ends_with(p)),
                "lower" => Ok(Value::from(s.to_lowercase())),
                "upper" => Ok(Value::from(s.to_uppercase())),
                "strip" => Ok(Value::from(kiwi_core::pystr::strip(&s))),
                _ => Err(unknown()),
            }
        }
        ValueKind::Map => match method {
            "items" => Ok(Value::from(
                value
                    .try_iter()?
                    .map(|k| {
                        let v = value.get_item(&k).unwrap_or_default();
                        Value::from(vec![k, v])
                    })
                    .collect::<Vec<_>>(),
            )),
            "keys" => Ok(Value::from(value.try_iter()?.collect::<Vec<_>>())),
            "values" => {
                Ok(Value::from(value.try_iter()?.map(|k| value.get_item(&k).unwrap_or_default()).collect::<Vec<_>>()))
            }
            "get" => {
                let key = args.first().ok_or_else(unknown)?;
                let v = value.get_item(key)?;
                Ok(if v.is_undefined() { args.get(1).cloned().unwrap_or(Value::from(())) } else { v })
            }
            _ => Err(unknown()),
        },
        _ => Err(unknown()),
    }
}

/// Jinja2's newline handling: `\r\n` and `\r` read as `\n`.
fn normalize_newlines(s: String) -> String {
    if s.contains('\r') { s.replace("\r\n", "\n").replace('\r', "\n") } else { s }
}

fn loader(root: PathBuf) -> impl for<'a> Fn(&'a str) -> Result<Option<String>, Error> + Send + Sync + 'static {
    move |name| {
        if name.split('/').any(|seg| seg.is_empty() || seg == "." || seg == "..") {
            return Ok(None);
        }
        match std::fs::read_to_string(root.join(name)) {
            Ok(s) => Ok(Some(normalize_newlines(s))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::new(ErrorKind::InvalidOperation, "could not read template").with_source(e)),
        }
    }
}

pub(crate) fn build(root: &Path) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_loader(loader(root.to_path_buf()));
    env.set_auto_escape_callback(|name| {
        let lower = name.to_lowercase();
        if [".html", ".htm", ".xml"].iter().any(|e| lower.ends_with(e)) { AutoEscape::Html } else { AutoEscape::None }
    });
    env.set_formatter(formatter);
    env.set_unknown_method_callback(py_method);
    env.add_filter("default", f_default);
    env.add_filter("d", f_default);
    env.add_filter("join", f_join);
    env.add_filter("tojson", f_tojson);
    env.add_filter("urlencode", f_urlencode);
    env.add_filter("length", f_length);
    env.add_filter("count", f_length);
    env.add_filter("replace", f_replace);
    env.add_filter("upper", |v: Value| Value::from(py_str(&v).to_uppercase()));
    env.add_filter("lower", |v: Value| Value::from(py_str(&v).to_lowercase()));
    env
}

type Fingerprint = Vec<(PathBuf, Option<SystemTime>, u64)>;

fn fingerprint(root: &Path) -> Fingerprint {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                stack.push(e.path());
            } else {
                out.push((e.path(), meta.modified().ok(), meta.len()));
            }
        }
    }
    out.sort();
    out
}

pub struct Templates {
    root: PathBuf,
    env: RwLock<Arc<Environment<'static>>>,
    seen: Mutex<(Instant, Fingerprint)>,
    pub versions: StaticVersions,
}

const RECHECK: Duration = Duration::from_secs(1);

impl Templates {
    pub fn new(site_root: &Path) -> Self {
        let root = site_root.join("templates");
        let fp = fingerprint(&root);
        Templates {
            env: RwLock::new(Arc::new(build(&root))),
            seen: Mutex::new((Instant::now(), fp)),
            versions: StaticVersions::new(&site_root.join("static")),
            root,
        }
    }

    fn env(&self) -> Arc<Environment<'static>> {
        let stale = {
            let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
            if seen.0.elapsed() < RECHECK {
                false
            } else {
                seen.0 = Instant::now();
                let fp = fingerprint(&self.root);
                let changed = fp != seen.1;
                seen.1 = fp;
                changed
            }
        };
        if stale {
            let fresh = Arc::new(build(&self.root));
            *self.env.write().unwrap_or_else(|e| e.into_inner()) = fresh.clone();
            return fresh;
        }
        self.env.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// `_LinkedTemplate.render`: the template, then `static_version.link`.
    pub fn render(&self, name: &str, ctx: Value) -> Result<String, Error> {
        let env = self.env();
        let html = env.get_template(name)?.render(ctx)?;
        Ok(link(&html, &self.versions))
    }
}

fn py_regex_space(c: char) -> bool {
    kiwi_core::pystr::is_space(c)
}

/// `static_version.link`: `(\s(?:src|href)=")(/static/[^"?#]+?(?<!\.woff2)(?<!\.woff))(")`
/// with `?v=<hash>` added when the file exists under the static root.
pub fn link(html: &str, versions: &StaticVersions) -> String {
    let mut out = String::with_capacity(html.len() + 512);
    let mut last = 0;
    let mut i = 0;
    while let Some(off) = html[i..].find(['s', 'h']) {
        let at = i + off;
        i = at + 1;
        let prev_ok = html[..at].chars().next_back().is_some_and(py_regex_space);
        if !prev_ok {
            continue;
        }
        let rest = &html[at..];
        let head = if rest.starts_with("src=\"") {
            5
        } else if rest.starts_with("href=\"") {
            6
        } else {
            continue;
        };
        let value_at = at + head;
        let value = &html[value_at..];
        if !value.starts_with("/static/") {
            continue;
        }
        let Some(stop) = value.find(['"', '?', '#']) else { continue };
        if !value[stop..].starts_with('"') || stop <= "/static/".len() {
            continue;
        }
        let url = &value[..stop];
        if url.ends_with(".woff2") || url.ends_with(".woff") {
            continue;
        }
        let end = value_at + stop + 1;
        if let Some(v) = versions.version(url) {
            out.push_str(&html[last..value_at + stop]);
            let _ = write!(out, "?v={v}");
            out.push('"');
            last = end;
        }
        i = end;
    }
    out.push_str(&html[last..]);
    out
}

//! The custom art release worker (`app/custom_art/builder.py`), run by the
//! `kiwi-builder` binary in the builder container. It drives the same external
//! tools: git, TroveUI's own release scripts (with the Flex SDK and FFDec they
//! call) and steamcmd.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::LazyLock,
    time::Duration,
};

use futures::TryStreamExt;
use kiwi_core::pystr;
use mongodb::{
    Collection,
    bson::{Bson, Document, doc},
};
use regex::Regex;
use serde_json::Value;

use super::{ArtRequest, COLLECTION, MODS, changelog, lane_label, lane_of, mods_of, released_mail, send};
use crate::{dt, media::cas::ContentStore, pyfloat, state::AppState};

/// Each mod's vanilla SWF in the archive and where the build expects it.
pub const VANILLA: [(&str, &str, &str); 3] = [
    (super::CHAT, "ui/chat.swf", "chat.vanilla.swf"),
    (super::NAMEPLATE, "ui/nameplate.swf", "nameplate.vanilla.swf"),
    (super::CLUBS, "ui/clubs.swf", "clubs.vanilla.swf"),
];
pub const ART: [&str; 4] = ["Zakros UI - Chat/art", "Zakros UI - Nameplate/art", "Zakros UI - Clubs/art", "originals"];
const TIMEOUT: Duration = Duration::from_secs(1800);
const LOG_MAX: usize = 60_000;
pub const STEAMCMD: &str = "/opt/steamcmd/steamcmd.sh";
pub const STEAM_HOME: &str = "/home/steam";
const STEAM_APP: &str = "304050";
const RESTARTED: &str = "The worker restarted in the middle of this release.";

/// Why a batch stopped: `BuildFailed`, or any other exception (`Type: text`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    Build(String),
    Other(String),
}

type R<T> = Result<T, Fail>;

fn py_repr_path(p: &Path) -> String {
    pystr::repr(&p.to_string_lossy())
}

/// An `OSError` as Python prints it.
pub fn os_error(e: &io::Error, path: &Path) -> Fail {
    let name = match e.kind() {
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::AlreadyExists => "FileExistsError",
        io::ErrorKind::IsADirectory => "IsADirectoryError",
        io::ErrorKind::NotADirectory => "NotADirectoryError",
        _ => "OSError",
    };
    Fail::Other(match e.raw_os_error() {
        Some(code) => {
            let text = e.to_string();
            let text = text.split(" (os error").next().unwrap_or_default().to_owned();
            format!("{name}: [Errno {code}] {text}: {}", py_repr_path(path))
        }
        None => format!("{name}: {e}"),
    })
}

/// `shutil.copyfile`.
fn copyfile(src: &Path, dst: &Path) -> R<()> {
    let mut from = std::fs::File::open(src).map_err(|e| os_error(&e, src))?;
    let mut to = std::fs::File::create(dst).map_err(|e| os_error(&e, dst))?;
    io::copy(&mut from, &mut to).map_err(|e| os_error(&e, dst))?;
    Ok(())
}

fn rmtree(p: &Path) {
    let _ = std::fs::remove_dir_all(p);
}

fn mkdirs(p: &Path) -> R<()> {
    std::fs::create_dir_all(p).map_err(|e| os_error(&e, p))
}

/// `mkdir(parents=True)` without `exist_ok`.
fn mkdir_new(p: &Path) -> R<()> {
    if p.exists() {
        return Err(os_error(&io::Error::from_raw_os_error(17), p));
    }
    mkdirs(p)
}

/// What a `text=True` pipe reads: decoded, universal newlines.
pub fn text_of(b: &[u8]) -> String {
    String::from_utf8_lossy(b).replace("\r\n", "\n").replace('\r', "\n")
}

/// `Run`: one command at a time, everything it printed kept for the log.
pub struct Run {
    env: Vec<(String, String)>,
    pub lines: Vec<String>,
}

impl Run {
    pub fn new(env: Vec<(String, String)>) -> Self {
        Run { env, lines: Vec::new() }
    }

    pub fn say(&mut self, text: impl Into<String>) {
        let text = text.into();
        tracing::info!("{text}");
        self.lines.push(text);
    }

    pub async fn call(&mut self, args: &[String], cwd: &Path, check: bool, env: &[(&str, &str)]) -> R<bool> {
        let shown = args.join(" ");
        self.say(format!("$ {shown}"));
        let mut cmd = tokio::process::Command::new(&args[0]);
        cmd.args(&args[1..]).current_dir(cwd).stdin(Stdio::null()).kill_on_drop(true);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        let child = cmd.output();
        let done = match tokio::time::timeout(TIMEOUT, child).await {
            Err(_) => return Err(Fail::Build(format!("timed out: {shown}"))),
            Ok(Err(e)) => {
                let path = if e.kind() == io::ErrorKind::NotFound && !cwd.is_dir() { cwd } else { Path::new(&args[0]) };
                return Err(os_error(&e, path));
            }
            Ok(Ok(out)) => out,
        };
        let out = text_of(&done.stdout) + &text_of(&done.stderr);
        let out = out.trim_end_matches(pystr::is_space);
        if !out.is_empty() {
            self.lines.push(out.to_owned());
        }
        let code = exit_code(&done.status);
        if code != 0 && check {
            return Err(Fail::Build(format!("exit {code}: {shown}")));
        }
        Ok(code == 0)
    }

    /// `"\n".join(lines)[-60000:]`.
    pub fn text(&self) -> String {
        let all = self.lines.join("\n");
        let n = all.chars().count();
        if n <= LOG_MAX {
            return all;
        }
        all.chars().skip(n - LOG_MAX).collect()
    }
}

/// `returncode`: negative for a signal, as Python reports it.
fn exit_code(s: &std::process::ExitStatus) -> i32 {
    if let Some(c) = s.code() {
        return c;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = s.signal() {
            return -sig;
        }
    }
    -1
}

/// `_short`.
pub fn short(mod_: &str) -> &str {
    mod_.strip_prefix("Zakros UI - ").unwrap_or(mod_)
}

/// `_art_message`.
pub fn art_message(requests: &[ArtRequest]) -> String {
    let names: Vec<&str> = requests.iter().map(|r| r.name.as_str()).collect();
    let mut shown = names.iter().take(4).copied().collect::<Vec<_>>().join(", ");
    if names.len() > 4 {
        shown.push_str(&format!(" and {} more", names.len() - 4));
    }
    format!("Custom art: {shown}")
}

static MOD_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""modVersion":[\s\x1c-\x1f]*"([^"]*)""#).expect("modVersion regex"));

/// `_version`'s search over `build.py`.
pub fn find_version(text: &str) -> Option<String> {
    MOD_VERSION.captures(text).map(|c| c[1].to_owned())
}

/// `_vdf_safe`.
pub fn vdf_safe(text: &str) -> String {
    text.replace('\\', " ").replace('"', "'").replace('\n', " ")
}

/// `str(value)` of a parsed JSON value.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.to_string(),
            None => match n.as_u64() {
                Some(u) => u.to_string(),
                None => pyfloat::repr(n.as_f64().unwrap_or_default()),
            },
        },
        other => crate::pyjson::dumps_spaced(other),
    }
}

/// The `item.vdf` steamcmd builds from.
pub fn vdf(item: &str, content: &str, note: &str) -> String {
    format!(
        "\"workshopitem\"\n{{\n\t\"appid\"\t\t\"{STEAM_APP}\"\n\t\"publishedfileid\"\t\"{item}\"\n\t\"contentfolder\"\t\"{content}\"\n\
         \t\"changenote\"\t\"{}\"\n}}\n",
        vdf_safe(note)
    )
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// The worker's settings and stores.
pub struct Builder {
    pub state: AppState,
    /// `sys.executable` of the Python worker: the interpreter for TroveUI's scripts.
    pub python: String,
    pub steamcmd: String,
    pub steam_home: String,
    /// Extra environment for every command (tests give git an identity here).
    pub env: Vec<(String, String)>,
}

struct Project {
    slug: String,
    owner_handle: String,
    is_beta: bool,
}

impl Builder {
    pub fn new(state: AppState) -> Self {
        let python = std::env::var("KIWI_BUILDER_PYTHON").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| {
            if Path::new("/usr/local/bin/python").is_file() { "/usr/local/bin/python".into() } else { "python3".into() }
        });
        Builder { state, python, steamcmd: STEAMCMD.into(), steam_home: STEAM_HOME.into(), env: Vec::new() }
    }

    /// The `Run` a batch logs to (`os.environ` plus `TROVE_DIR`).
    pub fn new_run(&self) -> Run {
        let mut env = self.env.clone();
        env.push(("TROVE_DIR".into(), self.root().join("trove").to_string_lossy().into_owned()));
        Run::new(env)
    }

    fn root(&self) -> PathBuf {
        PathBuf::from(&self.state.settings().custom_art_workspace)
    }

    fn coll(&self) -> Collection<Document> {
        self.state.mongo().collection(COLLECTION)
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    pub(crate) async fn checkout(&self, run: &mut Run) -> R<PathBuf> {
        let st = self.state.settings();
        let root = self.root();
        let tree = root.join("TroveUI");
        mkdirs(&root)?;
        if !tree.join(".git").is_dir() {
            run.call(&Self::s(&["git", "clone", &st.custom_art_remote, &tree.to_string_lossy()]), &root, true, &[])
                .await?;
        }
        run.call(&Self::s(&["git", "fetch", "origin"]), &tree, true, &[]).await?;
        run.call(&Self::s(&["git", "reset", "--hard", "origin/master"]), &tree, true, &[]).await?;
        run.call(&Self::s(&["git", "clean", "-fd"]), &tree, true, &[]).await?;
        let lib = tree.join("lib");
        if !lib.join(".git").is_dir() {
            rmtree(&lib);
            let args = Self::s(&["git", "clone", "--depth", "1", &st.custom_art_lib_remote, &lib.to_string_lossy()]);
            run.call(&args, &tree, true, &[]).await?;
        } else {
            run.call(&Self::s(&["git", "fetch", "--depth", "1", "origin", "HEAD"]), &lib, true, &[]).await?;
            run.call(&Self::s(&["git", "reset", "--hard", "FETCH_HEAD"]), &lib, true, &[]).await?;
        }
        copyfile(Path::new(&st.custom_art_env_file), &tree.join(".env"))?;
        Ok(tree)
    }

    async fn state_rows(&self, filter: Document) -> R<Vec<(String, String)>> {
        let docs: Vec<Document> = self
            .state
            .mongo()
            .collection::<Document>("update_state")
            .find(filter)
            .await
            .map_err(other)?
            .try_collect()
            .await
            .map_err(other)?;
        docs.iter()
            .map(|d| match (d.get_str("path"), d.get_str("content_sha256")) {
                (Ok(p), Ok(s)) => Ok((p.to_owned(), s.to_owned())),
                _ => Err(Fail::Other("ValidationError: an update_state row is not an UpdateState".into())),
            })
            .collect()
    }

    pub(crate) async fn game_files(&self, tree: &Path, run: &mut Run) -> R<()> {
        let st = self.state.settings();
        let blobs = ContentStore::new(&st.trove_update_store_dir);
        let branch = &st.custom_art_branch;
        let copy = |sha: &str, dest: &Path| -> R<()> {
            let src = blobs.path_for(sha);
            if !src.is_file() {
                return Err(Fail::Build(format!("archive blob {sha} is missing")));
            }
            copyfile(&src, dest)
        };
        let lang = self.root().join("trove").join("languages").join("en");
        rmtree(&lang);
        mkdir_new(&lang)?;
        let rows =
            self.state_rows(doc! {"branch": branch, "path": {"$regex": r"^languages/en/[^/]+\.binfab$"}}).await?;
        if rows.is_empty() {
            return Err(Fail::Build(format!("no English language files archived on {branch}")));
        }
        for (path, sha) in &rows {
            let name = path.rsplit('/').next().unwrap_or_default();
            copy(sha, &lang.join(name))?;
        }
        let mut wanted: Vec<(&str, PathBuf)> =
            VANILLA.iter().map(|(m, path, name)| (*path, tree.join(m).join(name))).collect();
        wanted.push(("Trove_x64.exe", self.root().join("trove").join("Trove_x64.exe")));
        for (path, dest) in wanted {
            let rows = self.state_rows(doc! {"branch": branch, "path": path}).await?;
            let Some((_, sha)) = rows.first() else {
                return Err(Fail::Build(format!("{path} is not archived on {branch}")));
            };
            copy(sha, &dest)?;
        }
        run.say(format!("{} language files, the client and {} vanilla SWFs from {branch}", rows.len(), VANILLA.len()));
        Ok(())
    }

    pub(crate) async fn place(&self, tree: &Path, requests: &[ArtRequest], run: &mut Run) -> R<()> {
        let incoming = self.root().join("incoming");
        rmtree(&incoming);
        mkdir_new(&incoming)?;
        let store = ContentStore::new(&self.state.settings().mods_store_dir);
        for r in requests {
            for (slot, picture) in &r.pictures {
                let Some(lane) = lane_of(&r.kind, slot) else {
                    return Err(Fail::Other(format!("KeyError: ({}, {})", pystr::repr(&r.kind), pystr::repr(slot))));
                };
                let path = store.path_for(&picture.sha);
                let data =
                    if path.is_file() { Some(std::fs::read(&path).map_err(|e| os_error(&e, &path))?) } else { None };
                let Some(data) = data else {
                    return Err(Fail::Build(format!(
                        "the {} for {} is missing from the store",
                        lane_label(lane),
                        pystr::repr(&r.name)
                    )));
                };
                let source = incoming.join(format!("{}-{slot}", r.id.to_hex()));
                std::fs::write(&source, data).map_err(|e| os_error(&e, &source))?;
                let args = vec![
                    self.python.clone(),
                    "custom_art.py".into(),
                    lane.into(),
                    r.name.clone(),
                    source.to_string_lossy().into_owned(),
                ];
                run.call(&args, tree, true, &[]).await?;
            }
        }
        Ok(())
    }

    /// `_commit`: add, commit and push `paths` when they changed.
    pub(crate) async fn commit(&self, run: &mut Run, tree: &Path, paths: &[&str], message: &str) -> R<()> {
        let mut status = tokio::process::Command::new("git");
        status.args(["status", "--porcelain", "--"]).args(paths).current_dir(tree).stdin(Stdio::null());
        let out = status.output().await.map_err(|e| os_error(&e, Path::new("git")))?;
        if pystr::strip(&text_of(&out.stdout)).is_empty() {
            run.say(format!("nothing to commit for {}", pystr::repr(message)));
            return Ok(());
        }
        let mut add = Self::s(&["git", "add", "-A", "--"]);
        add.extend(paths.iter().map(|p| (*p).to_owned()));
        run.call(&add, tree, true, &[]).await?;
        run.call(&Self::s(&["git", "commit", "-m", message]), tree, true, &[]).await?;
        run.call(&Self::s(&["git", "push", "origin", "HEAD:master"]), tree, true, &[]).await?;
        Ok(())
    }

    pub(crate) fn version(&self, tree: &Path, mod_: &str) -> R<String> {
        let path = tree.join(mod_).join("build.py");
        let bytes = std::fs::read(&path).map_err(|e| os_error(&e, &path))?;
        let text =
            String::from_utf8(bytes).map_err(|_| Fail::Other("UnicodeDecodeError: build.py is not UTF-8".into()))?;
        find_version(&text).ok_or_else(|| Fail::Build(format!("no modVersion in {mod_}/build.py")))
    }

    /// `_steam`: never fatal; true only when steamcmd reported success.
    pub(crate) async fn steam(&self, tree: &Path, mod_: &str, note: &str, run: &mut Run) -> R<bool> {
        let user = self.state.settings().custom_art_steam_user.clone();
        if user.is_empty() {
            run.say(format!("WARNING no Steam account configured - {mod_} not sent to Steam"));
            return Ok(false);
        }
        let ids = tree.join("steam_ids.json");
        let item = if ids.is_file() {
            let raw = std::fs::read(&ids).map_err(|e| os_error(&e, &ids))?;
            match crate::pyjson::loads(&raw) {
                Ok(Value::Object(m)) => m.get(mod_).cloned().filter(|v| !v.is_null()),
                Ok(other) => {
                    return Err(Fail::Other(format!(
                        "AttributeError: '{}' object has no attribute 'get'",
                        type_name(&other)
                    )));
                }
                Err(_) => return Err(Fail::Other("JSONDecodeError: steam_ids.json is not JSON".into())),
            }
        } else {
            None
        };
        let Some(item) = item else {
            run.say(format!("WARNING {mod_} has no Steam id in steam_ids.json - not sent to Steam"));
            return Ok(false);
        };
        let item = py_str(&item);
        let art = tree.join(mod_).join("dist").join(format!("{mod_}.tmod"));
        if !art.is_file() {
            run.say(format!("WARNING {mod_} has no built .tmod - not sent to Steam"));
            return Ok(false);
        }
        let work = self.root().join("steam");
        rmtree(&work);
        let content = work.join("content");
        mkdirs(&content)?;
        copyfile(&art, &content.join(format!("{mod_}.tmod")))?;
        let vdf_path = work.join("item.vdf");
        let posix = content.to_string_lossy().replace('\\', "/");
        std::fs::write(&vdf_path, vdf(&item, &posix, note)).map_err(|e| os_error(&e, &vdf_path))?;
        let args = vec![
            self.steamcmd.clone(),
            "+login".into(),
            user,
            "+workshop_build_item".into(),
            vdf_path.to_string_lossy().into_owned(),
            "+quit".into(),
        ];
        let ok = run.call(&args, &work, false, &[("HOME", &self.steam_home)]).await?;
        let tail = run.lines.last().cloned().unwrap_or_default();
        if !ok || !tail.contains("Success.") {
            run.say(format!("WARNING Steam upload failed for {mod_} - push it by hand"));
            return Ok(false);
        }
        run.say(format!("{mod_} is on the Steam Workshop, item {item}"));
        Ok(true)
    }

    async fn project(&self, title: &str) -> R<Option<Project>> {
        let d = self
            .state
            .mongo()
            .collection::<Document>("mod_projects")
            .find_one(doc! {"title": title})
            .await
            .map_err(other)?;
        let Some(d) = d else { return Ok(None) };
        let bad = || Fail::Other("ValidationError: the hub project is not a ModProject".into());
        let slug = d.get_str("slug").map_err(|_| bad())?.to_owned();
        let owner_handle = match d.get("owner_handle") {
            None => String::new(),
            Some(Bson::String(s)) => s.clone(),
            Some(_) => return Err(bad()),
        };
        let is_beta = match d.get("is_beta") {
            None => false,
            Some(Bson::Boolean(b)) => *b,
            Some(_) => return Err(bad()),
        };
        Ok(Some(Project { slug, owner_handle, is_beta }))
    }

    async fn save(&self, r: &ArtRequest) -> R<()> {
        self.coll().update_one(doc! {"_id": r.id}, doc! {"$set": r.to_doc()}).upsert(true).await.map_err(other)?;
        Ok(())
    }

    async fn publish(
        &self,
        tree: &Path,
        mod_: &str,
        requests: &mut [ArtRequest],
        mine: &[usize],
        run: &mut Run,
        steamed: &mut HashMap<String, bool>,
    ) -> R<String> {
        let Some(project) = self.project(mod_).await? else {
            return Err(Fail::Build(format!("no hub project titled {}", pystr::repr(mod_))));
        };
        let picked: Vec<ArtRequest> = mine.iter().map(|&i| requests[i].clone()).collect();
        let note = changelog(&picked, mod_);
        let args = vec![
            self.python.clone(),
            "release.py".into(),
            mod_.into(),
            "-m".into(),
            note.clone(),
            "--silent".into(),
            "--no-steam".into(),
        ];
        run.call(&args, tree, true, &[]).await?;
        let version = self.version(tree, mod_)?;
        let build = format!("{mod_}/build.py");
        match self.commit(run, tree, &[&build], &format!("{}: record {version}", short(mod_))).await {
            Err(Fail::Build(e)) => run.say(format!("WARNING the version bump did not reach origin: {e}")),
            other => other?,
        }
        if project.is_beta {
            run.say(format!("{mod_} is beta - hub only, Trovesaurus skipped"));
        } else {
            let args = vec![
                self.python.clone(),
                "trovesaurus.py".into(),
                mod_.into(),
                "-m".into(),
                note.clone(),
                "--upload".into(),
                "--silent".into(),
            ];
            if !run.call(&args, tree, false, &[]).await? {
                run.say(format!("WARNING Trovesaurus upload failed for {mod_} - send it by hand"));
            }
        }
        let ok = self.steam(tree, mod_, &note, run).await?;
        steamed.insert(mod_.to_owned(), ok);
        for &i in mine {
            requests[i].set_version(mod_, &version);
            self.save(&requests[i]).await?;
        }
        Ok(format!(
            "{}/mods/{}/{}",
            self.state.settings().app_url.trim_end_matches('/'),
            project.owner_handle,
            project.slug
        ))
    }

    async fn steps(
        &self,
        requests: &mut [ArtRequest],
        run: &mut Run,
        pages: &mut HashMap<String, String>,
        steamed: &mut HashMap<String, bool>,
    ) -> R<()> {
        let tree = self.checkout(run).await?;
        self.game_files(&tree, run).await?;
        self.place(&tree, requests, run).await?;
        self.commit(run, &tree, &ART, &art_message(requests)).await?;
        for mod_ in MODS {
            let mine: Vec<usize> = (0..requests.len())
                .filter(|&i| mods_of(&requests[i]).contains(&mod_) && requests[i].version(mod_).is_none())
                .collect();
            if !mine.is_empty() {
                let page = self.publish(&tree, mod_, requests, &mine, run, steamed).await?;
                pages.insert(mod_.to_owned(), page);
            }
        }
        Ok(())
    }

    /// `_process`: build and publish one batch, then settle every request in it.
    pub async fn process(&self, requests: &mut [ArtRequest]) -> anyhow::Result<()> {
        let mut run = self.new_run();
        let mut pages = HashMap::new();
        let mut steamed = HashMap::new();
        match self.steps(requests, &mut run, &mut pages, &mut steamed).await {
            Ok(()) => {}
            Err(Fail::Build(e)) => run.say(format!("FAILED {e}")),
            Err(Fail::Other(e)) => {
                tracing::error!("custom art batch crashed: {e}");
                run.say(format!("FAILED {e}"));
            }
        }
        let log = run.text();
        for r in requests.iter_mut() {
            r.log = Some(log.clone());
            let mods = mods_of(r);
            if mods.iter().all(|m| r.version(m).is_some()) {
                r.status = "released".into();
                r.steam_pending = !mods.iter().all(|m| steamed.get(*m).copied().unwrap_or(false));
                r.released_at = Some(dt::now());
                self.save(r).await.map_err(fail_err)?;
                if let Some(mail) = released_mail(r, &pages) {
                    send(&self.state, &mail).await;
                }
            } else {
                r.status = "failed".into();
                self.save(r).await.map_err(fail_err)?;
            }
        }
        Ok(())
    }

    /// `_claim`: every queued request becomes this batch.
    pub async fn claim(&self) -> anyhow::Result<Vec<ArtRequest>> {
        let mut raw = [0u8; 6];
        getrandom::fill(&mut raw)?;
        let batch = hex::encode(raw);
        self.coll()
            .update_many(doc! {"status": "queued"}, doc! {"$set": {"status": "building", "batch": &batch}})
            .await?;
        let docs: Vec<Document> = self
            .coll()
            .find(doc! {"batch": &batch, "status": "building"})
            .sort(doc! {"created_at": 1})
            .await?
            .try_collect()
            .await?;
        docs.iter().map(|d| ArtRequest::from_doc(d).map_err(anyhow::Error::msg)).collect()
    }

    /// `main`: fail whatever a restart interrupted, then poll forever.
    pub async fn run_forever(&self) -> anyhow::Result<()> {
        self.coll()
            .update_many(doc! {"status": "building"}, doc! {"$set": {"status": "failed", "log": RESTARTED}})
            .await?;
        let poll = self.state.settings().custom_art_poll_seconds.max(0) as u64;
        tracing::info!("custom art worker up, polling every {poll}s");
        loop {
            match self.claim().await {
                Ok(mut batch) if !batch.is_empty() => {
                    tracing::info!("releasing {} custom art request(s)", batch.len());
                    if let Err(e) = self.process(&mut batch).await {
                        tracing::error!("custom art worker iteration failed: {e:#}");
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::error!("custom art worker iteration failed: {e:#}"),
            }
            tokio::time::sleep(Duration::from_secs(poll)).await;
        }
    }
}

fn other(e: impl std::fmt::Display) -> Fail {
    Fail::Other(format!("{e}"))
}

fn fail_err(f: Fail) -> anyhow::Error {
    match f {
        Fail::Build(e) | Fail::Other(e) => anyhow::anyhow!(e),
    }
}

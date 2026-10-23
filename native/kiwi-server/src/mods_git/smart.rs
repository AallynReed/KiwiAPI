//! The git smart-HTTP protocol, spoken by the `git` binary (`upload-pack` /
//! `receive-pack --stateless-rpc`) instead of dulwich. Bodies stream both
//! ways; a gzip request body is decoded first. Pushes keep dulwich's
//! behaviour: the pack is applied, then any ref whose new tip holds a `.tmod`
//! is rewound and reported `ng` with Python's message.

use std::{path::Path, process::Stdio};

use axum::body::Body;
use bytes::Bytes;
use futures::StreamExt;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdout, Command},
};

use super::{
    odb::{GitError, GitResult, Repo, from_hex},
    refs, store,
};

pub const BLOCKED_PUSH_STATUS: &str = "a built .tmod belongs in a release, not in the files";
const CHUNK: usize = 64 * 1024;
const SIDEBAND_MAX: usize = 65515;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    UploadPack,
    ReceivePack,
}

impl Service {
    pub fn from_name(s: &str) -> Option<Service> {
        match s {
            "git-upload-pack" => Some(Service::UploadPack),
            "git-receive-pack" => Some(Service::ReceivePack),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Service::UploadPack => "git-upload-pack",
            Service::ReceivePack => "git-receive-pack",
        }
    }

    fn sub(self) -> &'static str {
        &self.name()[4..]
    }
}

pub fn pkt_line(data: &[u8]) -> Vec<u8> {
    let mut out = format!("{:04x}", data.len() + 4).into_bytes();
    out.extend_from_slice(data);
    out
}

fn null_device() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

/// `git <service> --stateless-rpc` on a repository, isolated from any user
/// or system config, writing what dulwich writes: no reflogs, packs kept as
/// packs, no auto-gc.
fn command(svc: Service) -> Command {
    let mut c = Command::new("git");
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("GIT_") {
            c.env_remove(k);
        }
    }
    c.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_device())
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "safe.directory=*",
            "-c",
            "core.logAllRefUpdates=false",
            "-c",
            "receive.autogc=false",
            "-c",
            "receive.unpackLimit=1",
            "-c",
            "pack.writeReverseIndex=false",
            "-c",
            "gc.auto=0",
            svc.sub(),
            "--stateless-rpc",
        ])
        .stderr(Stdio::null())
        .kill_on_drop(true);
    c
}

/// The `info/refs` body: the service banner, a flush, then the
/// advertisement `git <service> --advertise-refs` prints.
pub async fn advertise(svc: Service, dir: &Path) -> GitResult<Vec<u8>> {
    let out = command(svc).arg("--advertise-refs").arg(dir).stdin(Stdio::null()).output().await?;
    if !out.status.success() {
        return Err(GitError::Invalid(format!("git {} --advertise-refs failed: {}", svc.sub(), out.status)));
    }
    let mut body = pkt_line(format!("# service={}\n", svc.name()).as_bytes());
    body.extend_from_slice(b"0000");
    body.extend_from_slice(&out.stdout);
    Ok(body)
}

/// A request body, gunzipped on the fly when it says it is gzipped.
struct Decoder(Option<flate2::write::GzDecoder<Vec<u8>>>);

impl Decoder {
    fn new(gzip: bool) -> Self {
        Decoder(gzip.then(|| flate2::write::GzDecoder::new(Vec::new())))
    }

    fn feed(&mut self, chunk: &[u8]) -> std::io::Result<Vec<u8>> {
        use std::io::Write;
        match &mut self.0 {
            None => Ok(chunk.to_vec()),
            Some(d) => {
                d.write_all(chunk)?;
                Ok(std::mem::take(d.get_mut()))
            }
        }
    }

    fn finish(&mut self) -> std::io::Result<Vec<u8>> {
        match self.0.take() {
            None => Ok(Vec::new()),
            Some(d) => d.finish(),
        }
    }
}

/// One client command of a push.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushCommand {
    pub old: Vec<u8>,
    pub new: Vec<u8>,
    pub name: Vec<u8>,
}

/// The command list at the head of a receive-pack request: pkt-lines up to
/// the first flush. `None` while more bytes are needed.
pub fn parse_commands(buf: &[u8]) -> Option<(Vec<PushCommand>, Vec<u8>)> {
    let mut out = Vec::new();
    let mut caps = Vec::new();
    let mut i = 0;
    loop {
        let len = std::str::from_utf8(buf.get(i..i + 4)?).ok().and_then(|h| usize::from_str_radix(h, 16).ok());
        let Some(len) = len else { return Some((out, caps)) };
        if len == 0 {
            return Some((out, caps));
        }
        if len < 4 {
            return Some((out, caps));
        }
        let mut line = buf.get(i + 4..i + len)?;
        i += len;
        if let Some(nul) = line.iter().position(|&b| b == 0) {
            if caps.is_empty() {
                caps = line[nul + 1..].to_vec();
            }
            line = &line[..nul];
        }
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        let parts: Vec<&[u8]> = line.split(|&b| b == b' ').filter(|p| !p.is_empty()).collect();
        if let [old, new, name] = parts[..] {
            out.push(PushCommand { old: old.to_vec(), new: new.to_vec(), name: name.to_vec() });
        }
    }
}

async fn feed(
    stdin: &mut tokio::process::ChildStdin,
    data: &[u8],
    head: &mut Vec<u8>,
    parsed: &mut Option<Parsed>,
    want: bool,
) -> bool {
    if want && parsed.is_none() {
        head.extend_from_slice(data);
        *parsed = parse_commands(head);
    }
    stdin.write_all(data).await.is_ok()
}

type Parsed = (Vec<PushCommand>, Vec<u8>);

async fn pump(body: Body, mut stdin: tokio::process::ChildStdin, gzip: bool, want: bool) -> Parsed {
    let mut dec = Decoder::new(gzip);
    let mut stream = body.into_data_stream();
    let mut head: Vec<u8> = Vec::new();
    let mut parsed = None;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else { break };
        let Ok(data) = dec.feed(&chunk) else { break };
        if !data.is_empty() && !feed(&mut stdin, &data, &mut head, &mut parsed, want).await {
            return parsed.unwrap_or_default();
        }
    }
    if let Ok(rest) = dec.finish()
        && !rest.is_empty()
    {
        feed(&mut stdin, &rest, &mut head, &mut parsed, want).await;
    }
    drop(stdin);
    parsed.unwrap_or_default()
}

fn spawn(svc: Service, dir: &Path) -> GitResult<Child> {
    Ok(command(svc).arg(dir).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?)
}

/// `git-upload-pack` with the response streamed. A git that dies before
/// writing anything is the 500 dulwich's exception was.
pub async fn upload_pack(dir: &Path, body: Body, gzip: bool) -> GitResult<Body> {
    let mut child = spawn(Service::UploadPack, dir)?;
    let stdin = child.stdin.take().ok_or_else(|| GitError::Invalid("no stdin".into()))?;
    let mut stdout = child.stdout.take().ok_or_else(|| GitError::Invalid("no stdout".into()))?;
    tokio::spawn(pump(body, stdin, gzip, false));
    let mut first = vec![0u8; CHUNK];
    let n = stdout.read(&mut first).await?;
    if n == 0 {
        let status = child.wait().await?;
        if !status.success() {
            return Err(GitError::Invalid(format!("git upload-pack failed: {status}")));
        }
        return Ok(Body::empty());
    }
    first.truncate(n);
    struct St {
        out: ChildStdout,
        child: Child,
        first: Option<Vec<u8>>,
    }
    let st = St { out: stdout, child, first: Some(first) };
    let stream = futures::stream::unfold(Some(st), |st| async move {
        let mut st = st?;
        if let Some(f) = st.first.take() {
            return Some((Ok::<Bytes, std::io::Error>(Bytes::from(f)), Some(st)));
        }
        let mut buf = vec![0u8; CHUNK];
        match st.out.read(&mut buf).await {
            Ok(0) => {
                let _ = st.child.wait().await;
                None
            }
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(Bytes::from(buf)), Some(st)))
            }
            Err(e) => Some((Err(e), None)),
        }
    });
    Ok(Body::from_stream(stream))
}

/// `git-receive-pack`, then the `.tmod` guard over what it accepted.
pub async fn receive_pack(dir: &Path, body: Body, gzip: bool) -> GitResult<Vec<u8>> {
    let mut child = spawn(Service::ReceivePack, dir)?;
    let stdin = child.stdin.take().ok_or_else(|| GitError::Invalid("no stdin".into()))?;
    let mut stdout = child.stdout.take().ok_or_else(|| GitError::Invalid("no stdout".into()))?;
    let pumping = tokio::spawn(pump(body, stdin, gzip, true));
    let mut out = Vec::new();
    stdout.read_to_end(&mut out).await?;
    let status = child.wait().await?;
    let (commands, caps) = pumping.await.unwrap_or_default();
    if !status.success() {
        return Err(GitError::Invalid(format!("git receive-pack failed: {status}")));
    }
    let dir = dir.to_path_buf();
    let rejected = tokio::task::spawn_blocking(move || guard(&dir, &commands))
        .await
        .map_err(|e| GitError::Invalid(e.to_string()))??;
    if rejected.is_empty() {
        return Ok(out);
    }
    let sideband = caps.split(u8::is_ascii_whitespace).any(|c| c == b"side-band-64k" || c == b"side-band");
    Ok(rewrite_report(&out, &rejected, sideband))
}

/// Rewinds every ref the push moved onto a commit holding a `.tmod`;
/// `(ref, ng reason)` for each.
pub fn guard(dir: &Path, commands: &[PushCommand]) -> GitResult<Vec<(Vec<u8>, Vec<u8>)>> {
    let repo = Repo::at(dir);
    let mut rejected = Vec::new();
    for cmd in commands {
        if cmd.new.iter().all(|&b| b == b'0') {
            continue;
        }
        if refs::get(dir, &cmd.name)?.as_deref() != Some(&cmd.new[..]) {
            continue;
        }
        let Some(oid) = from_hex(&cmd.new) else { continue };
        let Some(path) = store::blocked_in_commit(&repo, &oid)? else { continue };
        let rewind = if cmd.old.iter().all(|&b| b == b'0') {
            refs::remove_if_equals(dir, &cmd.name, Some(&cmd.new))
        } else {
            refs::set_if_equals(dir, &cmd.name, Some(&cmd.new), &cmd.old)
        };
        if let Err(e) = rewind {
            tracing::warn!("mods git: rewinding {} failed: {e}", String::from_utf8_lossy(&cmd.name));
        }
        let reason = format!("{BLOCKED_PUSH_STATUS} ({path})").into_bytes();
        rejected.push((cmd.name.clone(), reason));
    }
    Ok(rejected)
}

enum Pkt {
    Data(Vec<u8>),
    Special(Vec<u8>),
}

fn split_pkts(buf: &[u8]) -> Option<Vec<Pkt>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let len = usize::from_str_radix(std::str::from_utf8(buf.get(i..i + 4)?).ok()?, 16).ok()?;
        if len < 4 {
            out.push(Pkt::Special(buf[i..i + 4].to_vec()));
            i += 4;
            continue;
        }
        out.push(Pkt::Data(buf.get(i + 4..i + len)?.to_vec()));
        i += len;
    }
    Some(out)
}

fn rewrite_lines(pkts: Vec<Pkt>, rejected: &[(Vec<u8>, Vec<u8>)]) -> Vec<Pkt> {
    pkts.into_iter()
        .map(|p| match p {
            Pkt::Data(d) => {
                let line = d.strip_suffix(b"\n").unwrap_or(&d);
                match line.strip_prefix(b"ok ").and_then(|r| rejected.iter().find(|(n, _)| n == r)) {
                    Some((name, reason)) => Pkt::Data([b"ng ", &name[..], b" ", &reason[..], b"\n"].concat()),
                    None => Pkt::Data(d),
                }
            }
            s => s,
        })
        .collect()
}

fn join_pkts(pkts: &[Pkt]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in pkts {
        match p {
            Pkt::Data(d) => out.extend_from_slice(&pkt_line(d)),
            Pkt::Special(s) => out.extend_from_slice(s),
        }
    }
    out
}

/// The report-status with each rejected ref's `ok` turned into `ng`, inside
/// side-band channel 1 when the client asked for side-band.
pub fn rewrite_report(out: &[u8], rejected: &[(Vec<u8>, Vec<u8>)], sideband: bool) -> Vec<u8> {
    let Some(pkts) = split_pkts(out) else { return out.to_vec() };
    if !sideband {
        return join_pkts(&rewrite_lines(pkts, rejected));
    }
    let mut inner = Vec::new();
    for p in &pkts {
        if let Pkt::Data(d) = p
            && d.first() == Some(&1)
        {
            inner.extend_from_slice(&d[1..]);
        }
    }
    let Some(report) = split_pkts(&inner) else { return out.to_vec() };
    let report = join_pkts(&rewrite_lines(report, rejected));
    let mut rebuilt = Vec::new();
    let mut placed = false;
    for p in pkts {
        match p {
            Pkt::Data(d) if d.first() == Some(&1) => {
                if !placed {
                    for chunk in report.chunks(SIDEBAND_MAX) {
                        rebuilt.push(Pkt::Data([&[1u8][..], chunk].concat()));
                    }
                    placed = true;
                }
            }
            other => rebuilt.push(other),
        }
    }
    join_pkts(&rebuilt)
}

//! dulwich's `DiskRefsContainer`: loose refs plus `packed-refs`, symrefs
//! followed five deep, writes behind `.lock` files.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use super::odb::{GitError, GitResult, ZERO_HEX, create_lock, lock_path, locked_write};

const SYMREF: &[u8] = b"ref: ";
const BAD_REF_CHARS: &[u8] = b"\x7f ~^:?*[";

/// `check_ref_format` (the name without its leading `refs/`).
pub fn check_ref_format(name: &[u8]) -> bool {
    if name == b"@" || !name.contains(&b'/') || contains(name, b"..") {
        return false;
    }
    if name.iter().any(|&c| c < 0o40 || BAD_REF_CHARS.contains(&c)) {
        return false;
    }
    if matches!(name.last(), Some(b'/' | b'.')) || contains(name, b"@{") || name.contains(&b'\\') {
        return false;
    }
    name.split(|&c| c == b'/').all(|c| !c.is_empty() && !c.starts_with(b".") && !c.ends_with(b".lock"))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn collapse_slashes(name: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(name.len());
    for &c in name {
        if !(c == b'/' && out.last() == Some(&b'/')) {
            out.push(c);
        }
    }
    out
}

/// `_check_refname`: HEAD, refs/stash, or a well-formed `refs/...` (a name
/// valid once its repeated slashes collapse is let through, with a warning).
pub fn refname_ok(name: &[u8]) -> bool {
    if name == b"HEAD" || name == b"refs/stash" {
        return true;
    }
    let Some(rest) = name.strip_prefix(b"refs/") else { return false };
    check_ref_format(rest) || (contains(name, b"//") && check_ref_format(&collapse_slashes(rest)))
}

fn check_refname(name: &[u8]) -> GitResult<()> {
    if refname_ok(name) {
        Ok(())
    } else {
        Err(GitError::Invalid(format!("bad ref name {:?}", String::from_utf8_lossy(name))))
    }
}

/// `refname_is_safe`.
fn refname_is_safe(name: &[u8]) -> bool {
    if name == b"HEAD" {
        return true;
    }
    if name.starts_with(b"/") || name.contains(&b'\0') {
        return false;
    }
    name.starts_with(b"refs/") && !name.split(|&c| c == b'/').any(|p| p == b"..")
}

pub fn refpath(dir: &Path, name: &[u8]) -> PathBuf {
    let mut p = dir.to_path_buf();
    for part in String::from_utf8_lossy(name).split('/') {
        if !part.is_empty() {
            p.push(part);
        }
    }
    p
}

/// `_read_loose_ref_file`: a symref's first line, else the first line.
fn read_loose_file(dir: &Path, name: &[u8]) -> Option<Vec<u8>> {
    let data = fs::read(refpath(dir, name)).ok()?;
    let line_end = data.iter().position(|&c| c == b'\n').map(|i| i + 1).unwrap_or(data.len());
    let mut line = data[..line_end].to_vec();
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    Some(line)
}

fn read_loose(dir: &Path, name: &[u8]) -> Option<Vec<u8>> {
    if !refname_ok(name) {
        return None;
    }
    read_loose_file(dir, name)
}

/// `get_packed_refs()`.
pub fn packed_refs(dir: &Path) -> GitResult<BTreeMap<Vec<u8>, Vec<u8>>> {
    let data = match fs::read(dir.join("packed-refs")) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = BTreeMap::new();
    for line in data.split(|&c| c == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() || line.starts_with(b"#") || line.starts_with(b"^") {
            continue;
        }
        let mut parts = line.split(|&c| c == b' ');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(sha), Some(name), None) if sha.len() == 40 && sha.iter().all(u8::is_ascii_hexdigit) => {
                out.insert(name.to_vec(), sha.to_vec());
            }
            _ => return Err(GitError::Corrupt("packed-refs line".into())),
        }
    }
    Ok(out)
}

fn read_ref(dir: &Path, name: &[u8], packed: &BTreeMap<Vec<u8>, Vec<u8>>) -> Option<Vec<u8>> {
    match read_loose(dir, name) {
        Some(c) if !c.is_empty() => Some(c),
        _ => packed.get(name).cloned(),
    }
}

type Chain = (Vec<Vec<u8>>, Option<Vec<u8>>);

/// `follow(name)`: the symref chain and the final contents.
fn follow(dir: &Path, name: &[u8], packed: &BTreeMap<Vec<u8>, Vec<u8>>) -> Result<Chain, ()> {
    let mut contents = Some([SYMREF, name].concat());
    let mut names = Vec::new();
    let mut depth = 0;
    while let Some(c) = contents.as_ref().filter(|c| c.starts_with(SYMREF)) {
        let target = c[SYMREF.len()..].to_vec();
        contents = read_ref(dir, &target, packed);
        names.push(target);
        if contents.is_none() {
            break;
        }
        depth += 1;
        if depth > 5 {
            return Err(());
        }
    }
    Ok((names, contents))
}

/// `refs[name]`: the value a ref resolves to.
pub fn get(dir: &Path, name: &[u8]) -> GitResult<Option<Vec<u8>>> {
    let packed = packed_refs(dir)?;
    Ok(follow(dir, name, &packed).ok().and_then(|(_, c)| c))
}

fn walk_loose(dir: &Path, rel: &str, out: &mut Vec<Vec<u8>>) {
    let Ok(rd) = fs::read_dir(dir.join(rel)) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let child = format!("{rel}/{name}");
        match e.file_type() {
            Ok(t) if t.is_dir() => walk_loose(dir, &child, out),
            Ok(_) => {
                if check_ref_format(child.as_bytes()) {
                    out.push(child.into_bytes());
                }
            }
            Err(_) => {}
        }
    }
}

/// `get_refs()` (`as_dict()`), in name order: every ref that resolves.
pub fn all(dir: &Path) -> GitResult<BTreeMap<Vec<u8>, Vec<u8>>> {
    let packed = packed_refs(dir)?;
    let mut keys = Vec::new();
    if refpath(dir, b"HEAD").exists() {
        keys.push(b"HEAD".to_vec());
    }
    walk_loose(dir, "refs", &mut keys);
    keys.extend(packed.keys().cloned());
    let mut out = BTreeMap::new();
    for k in keys {
        if let Ok((_, Some(v))) = follow(dir, &k, &packed) {
            out.insert(k, v);
        }
    }
    Ok(out)
}

fn parent_of(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&c| c == b'/') {
        Some(i) => &name[..i],
        None => b"",
    }
}

fn abort(lock: &Path) {
    let _ = fs::remove_file(lock);
}

/// `set_if_equals(name, old, new)` (`refs[name] = new` when `old` is None).
pub fn set_if_equals(dir: &Path, name: &[u8], old: Option<&[u8]>, new: &[u8]) -> GitResult<bool> {
    check_refname(name)?;
    if new.starts_with(SYMREF) || !(new.len() == 40 && new.iter().all(u8::is_ascii_hexdigit)) {
        return Err(GitError::Invalid("bad ref value".into()));
    }
    let packed = packed_refs(dir)?;
    let realname = match follow(dir, name, &packed) {
        Ok((names, _)) => names.last().cloned().unwrap_or_else(|| name.to_vec()),
        Err(()) => name.to_vec(),
    };
    if !refname_is_safe(&realname) {
        return Err(GitError::Invalid("unsafe ref name".into()));
    }
    let filename = refpath(dir, &realname);
    let mut probe = parent_of(&realname);
    while !probe.is_empty() {
        if packed.contains_key(probe) {
            return Err(GitError::Invalid("ref directory is a packed ref".into()));
        }
        probe = parent_of(probe);
    }
    if let Some(p) = filename.parent() {
        fs::create_dir_all(p)?;
    }
    let lock = lock_path(&filename);
    let mut f = create_lock(&lock, 0o644)?;
    let current = || read_loose_file(dir, &realname).or_else(|| packed.get(&realname).cloned());
    if let Some(old) = old {
        let orig = current().unwrap_or_else(|| ZERO_HEX.as_bytes().to_vec());
        if orig != old {
            abort(&lock);
            return Ok(false);
        }
    }
    if current().as_deref() == Some(new) {
        abort(&lock);
        return Ok(true);
    }
    let res = (|| -> io::Result<()> {
        use io::Write;
        f.write_all(&[new, b"\n"].concat())?;
        f.sync_all()?;
        drop(f);
        fs::rename(&lock, &filename)
    })();
    if let Err(e) = res {
        abort(&lock);
        return Err(e.into());
    }
    Ok(true)
}

pub fn set(dir: &Path, name: &[u8], new: &[u8]) -> GitResult<()> {
    set_if_equals(dir, name, None, new).map(|_| ())
}

/// `remove_if_equals(name, old)` (`del refs[name]` when `old` is None),
/// then the now-empty parent folders up to `refs/`.
pub fn remove_if_equals(dir: &Path, name: &[u8], old: Option<&[u8]>) -> GitResult<bool> {
    check_refname(name)?;
    let filename = refpath(dir, name);
    if let Some(p) = filename.parent() {
        fs::create_dir_all(p)?;
    }
    let lock = lock_path(&filename);
    let f = create_lock(&lock, 0o644)?;
    let res = (|| -> GitResult<bool> {
        let packed = packed_refs(dir)?;
        if let Some(old) = old {
            let orig = read_loose(dir, name)
                .or_else(|| packed.get(name).cloned())
                .unwrap_or_else(|| ZERO_HEX.as_bytes().to_vec());
            if orig != old {
                return Ok(false);
            }
        }
        if fs::symlink_metadata(&filename).is_ok() {
            fs::remove_file(&filename)?;
        }
        if packed.contains_key(name) {
            remove_packed(dir, name, packed)?;
        }
        Ok(true)
    })();
    drop(f);
    abort(&lock);
    if !res.as_ref().is_ok_and(|ok| *ok) {
        return res;
    }
    let mut parent = parent_of(name);
    while !parent.is_empty() && parent != b"refs" {
        if fs::remove_dir(refpath(dir, parent)).is_err() {
            break;
        }
        parent = parent_of(parent);
    }
    Ok(true)
}

fn remove_packed(dir: &Path, name: &[u8], mut packed: BTreeMap<Vec<u8>, Vec<u8>>) -> GitResult<()> {
    packed.remove(name);
    let mut out = b"# pack-refs with: peeled\n".to_vec();
    for (k, v) in &packed {
        out.extend_from_slice(v);
        out.push(b' ');
        out.extend_from_slice(k);
        out.push(b'\n');
    }
    locked_write(&dir.join("packed-refs"), &out, 0o644, true)
}

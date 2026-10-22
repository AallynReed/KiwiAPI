//! File drops (`app/drops/service.py`) and the parts of Starlette 1.9's
//! `FileResponse` its download uses: range parsing and the attachment header.

use std::sync::LazyLock;

use regex::Regex;

use crate::{pynum, pystr};

static UNSAFE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^A-Za-z0-9._ ()\[\]-]+").expect("regex"));

/// `safe_filename`: the basename, anything unusual replaced, at most 120
/// characters, `upload.bin` when nothing is left.
pub fn safe_filename(name: Option<&str>) -> String {
    let name = pystr::strip(name.unwrap_or("")).replace('\\', "/");
    let base = name.rsplit('/').next().unwrap_or("");
    let cleaned = UNSAFE.replace_all(base, "_");
    let cleaned = cleaned.trim_matches([' ', '.']);
    let cut = pystr::prefix(cleaned, 120);
    if cut.is_empty() { "upload.bin".into() } else { cut.to_owned() }
}

/// `urllib.parse.quote(s)` (safe `/`, UTF-8).
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `FileResponse`'s `Content-Disposition` for `filename`.
pub fn content_disposition(filename: &str) -> String {
    let quoted = quote(filename);
    if quoted != filename {
        format!("attachment; filename*=utf-8''{quoted}")
    } else {
        format!("attachment; filename=\"{filename}\"")
    }
}

/// Why a `Range` header was refused: `MalformedRangeHeader` (400, plain
/// text) or `RangeNotSatisfiable` (416).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeError {
    Malformed(&'static str),
    NotSatisfiable,
}

pub const MAX_RANGES: usize = 100;

/// `FileResponse._parse_range_header`: half-open `(start, end)` ranges, merged
/// and sorted when there are several. Empty means "send the whole file".
pub fn parse_range_header(http_range: &str, size: u64) -> Result<Vec<(u64, u64)>, RangeError> {
    let Some((units, range)) = http_range.split_once('=') else {
        return Err(RangeError::Malformed("Malformed range header."));
    };
    if pystr::strip(units).to_lowercase() != "bytes" {
        return Err(RangeError::Malformed("Only support bytes range"));
    }
    if range.matches(',').count() + 1 > MAX_RANGES {
        return Ok(Vec::new());
    }
    let mut ranges = parse_ranges(range, size);
    if ranges.is_empty() {
        return Err(RangeError::Malformed("Range header: range must be requested"));
    }
    let size_i = i128::from(size);
    if ranges.iter().any(|&(start, _)| !(0 <= start && start < size_i)) {
        return Err(RangeError::NotSatisfiable);
    }
    if ranges.iter().any(|&(start, end)| start >= end) {
        return Err(RangeError::Malformed("Range header: start must be less than end"));
    }
    let to_u = |(s, e): (i128, i128)| (s as u64, e as u64);
    if ranges.len() == 1 {
        return Ok(vec![to_u(ranges[0])]);
    }
    ranges.sort();
    let mut out: Vec<(i128, i128)> = vec![ranges[0]];
    for &(start, end) in &ranges[1..] {
        let last = out.last_mut().expect("non-empty");
        if start <= last.1 {
            last.1 = last.1.max(end);
        } else {
            out.push((start, end));
        }
    }
    Ok(out.into_iter().map(to_u).collect())
}

fn parse_ranges(range: &str, size: u64) -> Vec<(i128, i128)> {
    let size = i128::from(size);
    let mut out = Vec::new();
    for part in range.split(',') {
        let part = pystr::strip(part);
        if part.is_empty() || part == "-" {
            continue;
        }
        let Some((start_s, end_s)) = part.split_once('-') else { continue };
        let (start_s, end_s) = (pystr::strip(start_s), pystr::strip(end_s));
        let start = if start_s.is_empty() {
            match pynum::int(end_s) {
                Some(e) => (size - i128::from(e)).max(0),
                None => continue,
            }
        } else {
            match pynum::int(start_s) {
                Some(s) => i128::from(s),
                None => continue,
            }
        };
        let end = if !start_s.is_empty() && !end_s.is_empty() {
            match pynum::int(end_s) {
                Some(e) if i128::from(e) < size => i128::from(e) + 1,
                Some(_) => size,
                None => continue,
            }
        } else {
            size
        };
        out.push((start, end));
    }
    out
}

/// One part header of a `multipart/byteranges` reply.
pub fn byterange_header(boundary: &str, content_type: &str, start: u64, end: u64, size: u64) -> String {
    format!("--{boundary}\r\nContent-Type: {content_type}\r\nContent-Range: bytes {start}-{}/{size}\r\n\r\n", end - 1)
}

/// `generate_multipart`'s Content-Length for these ranges.
pub fn byteranges_length(ranges: &[(u64, u64)], boundary: &str, content_type: &str, size: u64) -> u64 {
    let fixed = 49 + boundary.len() + content_type.len() + size.to_string().len();
    let parts: u64 = ranges
        .iter()
        .map(|&(start, end)| (start.to_string().len() + (end - 1).to_string().len() + fixed) as u64 + (end - start))
        .sum();
    parts + 4 + boundary.len() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames() {
        assert_eq!(safe_filename(None), "upload.bin");
        assert_eq!(safe_filename(Some("  C:\\Users\\me\\My File (1).PNG ")), "My File (1).PNG");
        assert_eq!(safe_filename(Some("../../etc/passwd")), "passwd");
        assert_eq!(safe_filename(Some("..")), "upload.bin");
        assert_eq!(safe_filename(Some("résumé ✓.pdf")), "r_sum_ _.pdf");
    }

    #[test]
    fn disposition() {
        assert_eq!(content_disposition("a-b_c.txt"), "attachment; filename=\"a-b_c.txt\"");
        assert_eq!(content_disposition("a b[1].txt"), "attachment; filename*=utf-8''a%20b%5B1%5D.txt");
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range_header("bytes=0-9", 100), Ok(vec![(0, 10)]));
        assert_eq!(parse_range_header("bytes=-5", 100), Ok(vec![(95, 100)]));
        assert_eq!(parse_range_header("bytes=90-", 100), Ok(vec![(90, 100)]));
        assert_eq!(parse_range_header("bytes=0-1000", 100), Ok(vec![(0, 100)]));
        assert_eq!(parse_range_header("bytes=50-60, 0-9, 55-70", 100), Ok(vec![(0, 10), (50, 71)]));
        assert_eq!(parse_range_header("bytes", 100), Err(RangeError::Malformed("Malformed range header.")));
        assert_eq!(parse_range_header("items=0-1", 100), Err(RangeError::Malformed("Only support bytes range")));
        assert_eq!(
            parse_range_header("bytes=x-y", 100),
            Err(RangeError::Malformed("Range header: range must be requested"))
        );
        assert_eq!(parse_range_header("bytes=100-", 100), Err(RangeError::NotSatisfiable));
        assert_eq!(
            parse_range_header("bytes=5-1", 100),
            Err(RangeError::Malformed("Range header: start must be less than end"))
        );
        assert_eq!(parse_range_header(&format!("bytes={}", vec!["0-1"; 101].join(",")), 100), Ok(vec![]));
    }

    #[test]
    fn byteranges() {
        let ranges = [(0, 10), (50, 71)];
        let b = "0123456789abcdef0123456789";
        let ct = "application/octet-stream";
        let expect: usize =
            ranges.iter().map(|&(s, e)| byterange_header(b, ct, s, e, 100).len() + (e - s) as usize + 2).sum::<usize>()
                + format!("--{b}--").len();
        assert_eq!(byteranges_length(&ranges, b, ct, 100), expect as u64);
    }
}

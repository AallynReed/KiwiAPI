//! Python 3.13 `mimetypes.guess_type` for a file path: the built-in table, then
//! every `mimetypes.knownfiles` entry that exists, read in order (later wins),
//! the way `mimetypes.init()` builds its database on Linux.

use std::collections::HashMap;
use std::sync::OnceLock;

const KNOWN_FILES: [&str; 9] = [
    "/etc/mime.types",
    "/etc/httpd/mime.types",
    "/etc/httpd/conf/mime.types",
    "/etc/apache/mime.types",
    "/etc/apache2/mime.types",
    "/usr/local/etc/httpd/conf/mime.types",
    "/usr/local/lib/netscape/mime.types",
    "/usr/local/etc/httpd/conf/mime.types",
    "/usr/local/etc/mime.types",
];

const SUFFIX_MAP: [(&str, &str); 6] = [
    (".svgz", ".svg.gz"),
    (".tgz", ".tar.gz"),
    (".taz", ".tar.gz"),
    (".tz", ".tar.gz"),
    (".tbz2", ".tar.bz2"),
    (".txz", ".tar.xz"),
];

const ENCODINGS: [&str; 5] = [".gz", ".Z", ".bz2", ".xz", ".br"];

#[rustfmt::skip]
const DEFAULT_TYPES: [(&str, &str); 154] = [
    (".js", "text/javascript"), (".mjs", "text/javascript"), (".json", "application/json"),
    (".webmanifest", "application/manifest+json"), (".doc", "application/msword"),
    (".dot", "application/msword"), (".wiz", "application/msword"), (".nq", "application/n-quads"),
    (".nt", "application/n-triples"), (".bin", "application/octet-stream"),
    (".a", "application/octet-stream"), (".dll", "application/octet-stream"),
    (".exe", "application/octet-stream"), (".o", "application/octet-stream"),
    (".obj", "application/octet-stream"), (".so", "application/octet-stream"),
    (".oda", "application/oda"), (".pdf", "application/pdf"), (".p7c", "application/pkcs7-mime"),
    (".ps", "application/postscript"), (".ai", "application/postscript"),
    (".eps", "application/postscript"), (".trig", "application/trig"),
    (".m3u", "application/vnd.apple.mpegurl"), (".m3u8", "application/vnd.apple.mpegurl"),
    (".xls", "application/vnd.ms-excel"), (".xlb", "application/vnd.ms-excel"),
    (".ppt", "application/vnd.ms-powerpoint"), (".pot", "application/vnd.ms-powerpoint"),
    (".ppa", "application/vnd.ms-powerpoint"), (".pps", "application/vnd.ms-powerpoint"),
    (".pwz", "application/vnd.ms-powerpoint"), (".wasm", "application/wasm"),
    (".bcpio", "application/x-bcpio"), (".cpio", "application/x-cpio"), (".csh", "application/x-csh"),
    (".dvi", "application/x-dvi"), (".gtar", "application/x-gtar"), (".hdf", "application/x-hdf"),
    (".h5", "application/x-hdf5"), (".latex", "application/x-latex"), (".mif", "application/x-mif"),
    (".cdf", "application/x-netcdf"), (".nc", "application/x-netcdf"), (".p12", "application/x-pkcs12"),
    (".pfx", "application/x-pkcs12"), (".ram", "application/x-pn-realaudio"),
    (".pyc", "application/x-python-code"), (".pyo", "application/x-python-code"),
    (".sh", "application/x-sh"), (".shar", "application/x-shar"),
    (".swf", "application/x-shockwave-flash"), (".sv4cpio", "application/x-sv4cpio"),
    (".sv4crc", "application/x-sv4crc"), (".tar", "application/x-tar"), (".tcl", "application/x-tcl"),
    (".tex", "application/x-tex"), (".texi", "application/x-texinfo"),
    (".texinfo", "application/x-texinfo"), (".roff", "application/x-troff"),
    (".t", "application/x-troff"), (".tr", "application/x-troff"), (".man", "application/x-troff-man"),
    (".me", "application/x-troff-me"), (".ms", "application/x-troff-ms"),
    (".ustar", "application/x-ustar"), (".src", "application/x-wais-source"),
    (".xsl", "application/xml"), (".rdf", "application/xml"), (".wsdl", "application/xml"),
    (".xpdl", "application/xml"), (".zip", "application/zip"), (".3gp", "audio/3gpp"),
    (".3gpp", "audio/3gpp"), (".3g2", "audio/3gpp2"), (".3gpp2", "audio/3gpp2"), (".aac", "audio/aac"),
    (".adts", "audio/aac"), (".loas", "audio/aac"), (".ass", "audio/aac"), (".au", "audio/basic"),
    (".snd", "audio/basic"), (".mp3", "audio/mpeg"), (".mp2", "audio/mpeg"), (".opus", "audio/opus"),
    (".aif", "audio/x-aiff"), (".aifc", "audio/x-aiff"), (".aiff", "audio/x-aiff"),
    (".ra", "audio/x-pn-realaudio"), (".wav", "audio/x-wav"), (".avif", "image/avif"),
    (".bmp", "image/bmp"), (".gif", "image/gif"), (".ief", "image/ief"), (".jpg", "image/jpeg"),
    (".jpe", "image/jpeg"), (".jpeg", "image/jpeg"), (".heic", "image/heic"), (".heif", "image/heif"),
    (".png", "image/png"), (".svg", "image/svg+xml"), (".tiff", "image/tiff"), (".tif", "image/tiff"),
    (".ico", "image/vnd.microsoft.icon"), (".webp", "image/webp"), (".ras", "image/x-cmu-raster"),
    (".pnm", "image/x-portable-anymap"), (".pbm", "image/x-portable-bitmap"),
    (".pgm", "image/x-portable-graymap"), (".ppm", "image/x-portable-pixmap"), (".rgb", "image/x-rgb"),
    (".xbm", "image/x-xbitmap"), (".xpm", "image/x-xpixmap"), (".xwd", "image/x-xwindowdump"),
    (".eml", "message/rfc822"), (".mht", "message/rfc822"), (".mhtml", "message/rfc822"),
    (".nws", "message/rfc822"), (".css", "text/css"), (".csv", "text/csv"), (".html", "text/html"),
    (".htm", "text/html"), (".md", "text/markdown"), (".markdown", "text/markdown"), (".n3", "text/n3"),
    (".txt", "text/plain"), (".bat", "text/plain"), (".c", "text/plain"), (".h", "text/plain"),
    (".ksh", "text/plain"), (".pl", "text/plain"), (".srt", "text/plain"), (".rtx", "text/richtext"),
    (".rtf", "text/rtf"), (".tsv", "text/tab-separated-values"), (".vtt", "text/vtt"),
    (".py", "text/x-python"), (".rst", "text/x-rst"), (".etx", "text/x-setext"), (".sgm", "text/x-sgml"),
    (".sgml", "text/x-sgml"), (".vcf", "text/x-vcard"), (".xml", "text/xml"), (".mp4", "video/mp4"),
    (".mpeg", "video/mpeg"), (".m1v", "video/mpeg"), (".mpa", "video/mpeg"), (".mpe", "video/mpeg"),
    (".mpg", "video/mpeg"), (".mov", "video/quicktime"), (".qt", "video/quicktime"),
    (".webm", "video/webm"), (".avi", "video/x-msvideo"), (".movie", "video/x-sgi-movie"),
];

fn defaults() -> HashMap<String, String> {
    DEFAULT_TYPES.iter().map(|(e, t)| ((*e).to_owned(), (*t).to_owned())).collect()
}

fn db() -> &'static HashMap<String, String> {
    static DB: OnceLock<HashMap<String, String>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut m = defaults();
        for file in KNOWN_FILES {
            if let Ok(text) = std::fs::read_to_string(file) {
                read_into(&mut m, &text);
            }
        }
        m
    })
}

/// `MimeTypes.readfp`.
pub fn read_into(m: &mut HashMap<String, String>, text: &str) {
    for line in text.lines() {
        let mut words: Vec<&str> = line.split(crate::pystr::is_space).filter(|w| !w.is_empty()).collect();
        if let Some(i) = words.iter().position(|w| w.starts_with('#')) {
            words.truncate(i);
        }
        let Some((ty, suffixes)) = words.split_first() else { continue };
        for suff in suffixes {
            m.insert(format!(".{suff}"), (*ty).to_owned());
        }
    }
}

/// `os.path.splitext` (posix).
fn splitext(p: &str) -> (String, String) {
    let sep = p.rfind('/').map_or(0, |i| i + 1);
    match p.rfind('.').filter(|&d| d > sep && !p[sep..d].chars().all(|c| c == '.')) {
        Some(d) => (p[..d].to_owned(), p[d..].to_owned()),
        None => (p.to_owned(), String::new()),
    }
}

/// `mimetypes.guess_type(path)[0]` (strict) for a filesystem path.
pub fn guess_type(path: &str) -> Option<String> {
    guess_in(db(), path)
}

fn guess_in(m: &HashMap<String, String>, path: &str) -> Option<String> {
    let (mut base, mut ext) = splitext(path);
    while let Some((_, to)) = SUFFIX_MAP.iter().find(|(from, _)| *from == ext.to_lowercase()) {
        (base, ext) = splitext(&format!("{base}{to}"));
    }
    if ENCODINGS.contains(&ext.as_str()) {
        ext = splitext(&base).1;
    }
    m.get(&ext.to_lowercase()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_answers() {
        let m = defaults();
        assert_eq!(guess_in(&m, "/a/style.min.css").as_deref(), Some("text/css"));
        assert_eq!(guess_in(&m, "/a/x.JS").as_deref(), Some("text/javascript"));
        assert_eq!(guess_in(&m, "/a/x.woff2"), None);
        assert_eq!(guess_in(&m, "/a/x.css.gz").as_deref(), Some("text/css"));
        assert_eq!(guess_in(&m, "/a/x.svgz").as_deref(), Some("image/svg+xml"));
        assert_eq!(guess_in(&m, "/a/.css"), None);
        assert_eq!(guess_in(&m, "/a.b/README"), None);
        assert_eq!(guess_in(&m, "/a/x.md").as_deref(), Some("text/markdown"));
        let mut m2 = m.clone();
        read_into(&mut m2, "# c\nfont/woff2 woff2 # x\napplication/javascript js\n");
        assert_eq!(guess_in(&m2, "/a/x.woff2").as_deref(), Some("font/woff2"));
        assert_eq!(guess_in(&m2, "/a/x.js").as_deref(), Some("application/javascript"));
    }
}

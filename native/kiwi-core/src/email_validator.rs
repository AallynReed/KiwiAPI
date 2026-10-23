//! pydantic's `EmailStr` (`pydantic.networks.validate_email` over
//! email-validator 2.3.0, `check_deliverability=False`) with idna 3.20 and
//! Python 3.13's Unicode 15.1 tables, messages included.

use std::sync::LazyLock;

use regex::Regex;

use crate::pystr;

#[rustfmt::skip]
#[path = "email_table.rs"]
mod table;
#[path = "email_idna.rs"]
mod idna;
#[path = "email_unidata.rs"]
mod unidata;

use unidata::{category, decomposes_to_dot, nfc_str};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Email {
    /// The normalized address (`ValidatedEmail.normalized`).
    Valid(String),
    /// The reason pydantic puts in `value is not a valid email address: {reason}`.
    Invalid(String),
}

pub const MAX_EMAIL_LENGTH: usize = 2048;
const EMAIL_MAX_LENGTH: usize = 254;
const DNS_LABEL_LENGTH_LIMIT: usize = 63;
const DOMAIN_MAX_LENGTH: usize = 253;
const CASE_INSENSITIVE_MAILBOX_NAMES: [&str; 15] = [
    "info",
    "marketing",
    "sales",
    "support",
    "abuse",
    "noc",
    "security",
    "postmaster",
    "hostmaster",
    "usenet",
    "news",
    "webmaster",
    "www",
    "uucp",
    "ftp",
];
const SPECIAL_USE_DOMAIN_NAMES: [&str; 6] = ["arpa", "invalid", "local", "localhost", "onion", "test"];

static PRETTY: LazyLock<Regex> = LazyLock::new(|| {
    let s = r"[\t\n\x0B\x0C\r\x1C-\x1F \x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}]";
    let mut word = String::new();
    for &(lo, hi) in table::WORD {
        word.push_str(&format!(r"\x{{{lo:X}}}-\x{{{hi:X}}}"));
    }
    let name = format!(r"[{word}.!#$%&'*+\-/=?^_`{{|}}~]");
    let unquoted = format!(r"((?:{name}+{s}+)*{name}+)");
    let quoted = r#""((?:[^"]|")+)""#;
    regex::RegexBuilder::new(&format!(r"\A{s}*(?:{unquoted}|{quoted})?{s}*<(.+)>{s}*\z"))
        .size_limit(64 << 20)
        .build()
        .expect("pretty email regex")
});

/// `pydantic.networks.validate_email(value)`.
pub fn validate(value: &str) -> Email {
    let r = (|| {
        if value.chars().count() > MAX_EMAIL_LENGTH {
            return Err(format!("Length must not exceed {MAX_EMAIL_LENGTH} characters"));
        }
        if value.contains('\r') || value.contains('\n') {
            return Err("Carriage return and line feed characters are not allowed".into());
        }
        let mut email = value;
        if let Some(c) = PRETTY.captures(value) {
            email = c.get(3).map_or(value, |m| m.as_str());
        }
        validate_email(pystr::strip(email))
    })();
    r.map_or_else(Email::Invalid, Email::Valid)
}

/// `email_validator.validate_email(value, check_deliverability=False)` called
/// directly: no pydantic wrapper, so no display-name unwrapping or length cap.
pub fn validate_direct(value: &str) -> Email {
    validate_email(value).map_or_else(Email::Invalid, Email::Valid)
}

type R<T> = Result<T, String>;

fn cps(s: &str) -> Vec<u32> {
    s.chars().map(u32::from).collect()
}

fn is_atext(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_!#$%&'*+-/=?^`{|}~".contains(c)
}

fn is_atext_intl(c: char) -> bool {
    is_atext(c) || c >= '\u{80}'
}

fn is_hostname_intl(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '.' || c >= '\u{80}'
}

fn lnps(cat: &str) -> bool {
    matches!(cat.as_bytes()[0], b'L' | b'N' | b'P' | b'S')
}

/// `safe_character_display`.
fn safe_display(c: char) -> String {
    if c == '\\' {
        return "\"\\\"".into();
    }
    let n = u32::from(c);
    if lnps(category(n)) {
        return pystr::repr(&c.to_string());
    }
    match unidata::name(n) {
        Some(name) => name.into(),
        None if n < 0xFFFF => format!("U+{n:04X}"),
        None => format!("U+{n:08X}"),
    }
}

/// `", ".join(sorted({safe_character_display(c) for c in s if bad(c)}))`.
fn bad_set(s: &str, bad: impl Fn(char) -> bool) -> String {
    let mut items: Vec<String> = s.chars().filter(|&c| bad(c)).map(safe_display).collect();
    items.sort();
    items.dedup();
    items.join(", ")
}

fn check_unsafe_chars(s: &str, allow_space: bool) -> R<()> {
    let mut bad: Vec<char> = Vec::new();
    for (i, c) in s.chars().enumerate() {
        let cat = category(u32::from(c));
        let unsafe_ = match cat.as_bytes()[0] {
            b'L' | b'N' | b'P' | b'S' => false,
            b'M' => i == 0,
            _ if cat == "Zs" => !allow_space,
            _ => true,
        };
        if unsafe_ {
            bad.push(c);
        }
    }
    if bad.is_empty() {
        return Ok(());
    }
    bad.sort_unstable();
    bad.dedup();
    let shown: Vec<String> = bad.into_iter().map(safe_display).collect();
    Err(format!("The email address contains unsafe characters: {}.", shown.join(", ")))
}

/// `split_string_at_unquoted_special`.
fn split_at_unquoted<'a>(text: &'a str, specials: &[char]) -> R<(&'a str, &'a str)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let (mut inside_quote, mut escaped) = (false, false);
    let mut end = text.len();
    for (k, &(i, c)) in chars.iter().enumerate() {
        let combines = !c.is_ascii() || chars.get(k + 1).is_some_and(|&(_, n)| !n.is_ascii());
        if combines {
            let stop = chars[k + 1..].iter().find(|&&(_, n)| unidata::stable_starter(u32::from(n)));
            if !nfc_str(&text[i..stop.map_or(text.len(), |s| s.0)]).starts_with(c) {
                continue;
            }
        }
        if inside_quote {
            if c == '\\' && !escaped {
                escaped = true;
            } else if c == '"' && !escaped {
                inside_quote = false;
                escaped = false;
            } else {
                escaped = false;
            }
        } else if c == '"' {
            inside_quote = true;
        } else if specials.contains(&c) {
            end = i;
            break;
        }
    }
    if end == text.len() {
        if text.contains('＠') {
            return Err(
                "The email address has the \"full-width\" at-sign (@) character instead of a regular at-sign.".into()
            );
        }
        if text.contains('﹫') {
            return Err(
                "The email address has the \"small commercial at\" character instead of a regular at-sign.".into()
            );
        }
        return Err("An email address must have an @-sign.".into());
    }
    Ok((&text[..end], &text[end..]))
}

fn unquote(text: &str) -> R<(String, bool)> {
    let (mut quoted, mut escaped) = (false, false);
    let mut value = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if quoted {
            if escaped {
                value.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                if i != chars.len() - 1 {
                    let rest: Vec<String> = chars[i + 1..].iter().map(|&c| safe_display(c)).collect();
                    return Err(format!("Extra character(s) found after close quote: {}", rest.join(", ")));
                }
                break;
            } else {
                value.push(c);
            }
        } else if i == 0 && c == '"' {
            quoted = true;
        } else {
            value.push(c);
        }
    }
    Ok((value, quoted))
}

struct Split {
    display_name: Option<String>,
    local: String,
    domain: String,
    quoted: bool,
}

fn split_email(email: &str) -> R<Split> {
    let (left, right) = split_at_unquoted(email, &['@', '<'])?;
    let (display_name, local, mut domain);
    if right.starts_with('<') {
        let left = left.trim_end_matches(pystr::is_space);
        let (name, name_quoted) = unquote(left)?;
        if !name_quoted {
            let bad = bad_set(&name, |c| (!(is_atext(c) || c == '.') && c != ' ') || c == '.');
            if !bad.is_empty() {
                return Err(format!("The display name contains invalid characters when not quoted: {bad}."));
            }
        }
        check_unsafe_chars(&name, true)?;
        if !right.contains('>') {
            return Err("An open angle bracket at the start of the email address has to be followed by a close angle bracket at the end.".into());
        }
        let right = right.trim_end_matches(' ');
        if !right.ends_with('>') {
            return Err("There can't be anything after the email address.".into());
        }
        let addr_spec = right[1..].trim_end_matches('>');
        let (l, d) = split_at_unquoted(addr_spec, &['@'])?;
        (local, domain) = (l, d);
        display_name = Some(name);
    } else {
        display_name = None;
        (local, domain) = (left, right);
    }
    if let Some(d) = domain.strip_prefix('@') {
        domain = d;
    }
    let (local, quoted) = unquote(local)?;
    Ok(Split { display_name, local, domain: domain.to_owned(), quoted })
}

fn dot_atom(s: &str, atext: fn(char) -> bool) -> bool {
    !s.is_empty() && s.split('.').all(|p| !p.is_empty() && p.chars().all(atext))
}

fn check_dot_atom(label: &str, start: &str, end: &str, hostname: bool) -> R<()> {
    let fill = |t: &str, what: &str| t.replace("{}", what);
    if label.ends_with('.') {
        return Err(fill(end, "period"));
    }
    if label.starts_with('.') {
        return Err(fill(start, "period"));
    }
    if label.contains("..") {
        return Err("An email address cannot have two periods in a row.".into());
    }
    if hostname {
        if label.ends_with('-') {
            return Err(fill(end, "hyphen"));
        }
        if label.starts_with('-') {
            return Err(fill(start, "hyphen"));
        }
        if label.contains(".-") || label.contains("-.") {
            return Err("An email address cannot have a period and a hyphen next to each other.".into());
        }
    }
    Ok(())
}

struct Local {
    local: String,
    ascii: Option<String>,
}

/// `validate_email_local_part` (`allow_smtputf8`, not strict).
fn local_part(local: &str, quoted: bool) -> R<Local> {
    if local.is_empty() {
        return Err("There must be something before the @-sign.".into());
    }
    if dot_atom(local, is_atext) {
        return Ok(Local { local: local.to_owned(), ascii: Some(local.to_owned()) });
    }
    let (is_quoted, smtputf8);
    if dot_atom(local, is_atext_intl) {
        (is_quoted, smtputf8) = (false, true);
    } else if quoted {
        let bad = bad_set(local, |c| !(' '..='~').contains(&c) && c < '\u{80}');
        if !bad.is_empty() {
            return Err(format!("The email address contains invalid characters in quotes before the @-sign: {bad}."));
        }
        (is_quoted, smtputf8) = (true, local.chars().any(|c| !(' '..='~').contains(&c)));
    } else {
        let bad = bad_set(local, |c| !(is_atext_intl(c) || c == '.'));
        if !bad.is_empty() {
            return Err(format!("The email address contains invalid characters before the @-sign: {bad}."));
        }
        check_dot_atom(
            local,
            "An email address cannot start with a {}.",
            "An email address cannot have a {} immediately before the @-sign.",
            false,
        )?;
        return Err("The email address contains invalid characters before the @-sign.".into());
    }
    check_unsafe_chars(local, is_quoted)?;
    let mut out = local.to_owned();
    if is_quoted {
        out = String::from('"');
        for c in local.chars() {
            if c == '"' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('"');
    }
    let ascii = (!smtputf8).then(|| out.clone());
    Ok(Local { local: out, ascii })
}

fn length_reason(len: usize, limit: usize) -> String {
    let diff = len - limit;
    format!("({diff} character{} too many)", if diff > 1 { "s" } else { "" })
}

/// `uts46_valid_char`.
fn uts46_valid_char(ch: char) -> bool {
    let c = u32::from(ch);
    if (0x80..=0x9f).contains(&c) {
        return false;
    }
    if ((0x2010..=0x2060).contains(&c) && !(0x2024..=0x2026).contains(&c) && !(0x2028..=0x202E).contains(&c))
        || matches!(c, 0xAD | 0x2064 | 0xFF0E)
        || (0x200B..=0x200D).contains(&c)
        || (0x1BCA0..=0x1BCA3).contains(&c)
    {
        return true;
    }
    if matches!(category(c), "Cf" | "Cn" | "Co" | "Cs" | "Zs" | "Zl" | "Zp") {
        return false;
    }
    !decomposes_to_dot(c)
}

fn hostname_label(l: &str) -> bool {
    let b = l.as_bytes();
    !b.is_empty() && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-') && b[0] != b'-' && b[b.len() - 1] != b'-'
}

fn dot_atom_hostname(s: &str) -> bool {
    s.split('.').all(hostname_label)
}

/// `validate_email_domain_name` (not a test environment, globally deliverable):
/// the ASCII and the internationalized domain.
fn domain_name(original: &str) -> R<(String, String)> {
    let bad = bad_set(original, |c| !is_hostname_intl(c));
    if !bad.is_empty() {
        return Err(format!("The part after the @-sign contains invalid characters: {bad}."));
    }
    check_unsafe_chars(original, false)?;
    let bad = bad_set(original, |c| !uts46_valid_char(c));
    if !bad.is_empty() {
        return Err(format!("The part after the @-sign contains invalid characters: {bad}."));
    }
    let domain = match idna::uts46_remap(&cps(original)) {
        Ok(d) => unidata::to_string(&d),
        Err(e) => return Err(format!("The part after the @-sign contains invalid characters ({}).", e.0)),
    };
    let bad = bad_set(&domain, |c| !is_hostname_intl(c));
    if !bad.is_empty() {
        return Err(format!(
            "The part after the @-sign contains invalid characters after Unicode normalization: {bad}."
        ));
    }
    check_dot_atom(
        &domain,
        "An email address cannot have a {} immediately after the @-sign.",
        "An email address cannot end with a {}.",
        true,
    )?;
    for label in domain.split('.') {
        let c: Vec<char> = label.chars().take(4).collect();
        if c.len() == 4 && c[2] == '-' && c[3] == '-' && !(matches!(c[0], 'x' | 'X') && matches!(c[1], 'n' | 'N')) {
            return Err("An email address cannot have two letters followed by two dashes immediately after the @-sign or after a period, except Punycode.".into());
        }
    }
    let ascii_domain = if dot_atom_hostname(&domain) {
        domain.clone()
    } else {
        let mut labels = Vec::new();
        for label in domain.split('.') {
            match idna::alabel(&cps(label)) {
                Ok(a) => labels.push(String::from_utf8_lossy(&a).into_owned()),
                Err(e) => return Err(format!("The part after the @-sign is invalid ({}).", e.0)),
            }
        }
        let a = labels.join(".");
        if !dot_atom_hostname(&a) {
            return Err("The email address contains invalid characters after the @-sign after IDNA encoding.".into());
        }
        a
    };
    if ascii_domain.len() > DOMAIN_MAX_LENGTH {
        if ascii_domain == original {
            return Err(format!(
                "The email address is too long after the @-sign {}.",
                length_reason(ascii_domain.len(), DOMAIN_MAX_LENGTH)
            ));
        }
        let diff = ascii_domain.len() - DOMAIN_MAX_LENGTH;
        let s = if diff == 1 { "" } else { "s" };
        return Err(format!(
            "The email address is too long after the @-sign ({diff} byte{s} too many after IDNA encoding)."
        ));
    }
    for label in ascii_domain.split('.') {
        if label.len() > DNS_LABEL_LENGTH_LIMIT {
            return Err(format!(
                "After the @-sign, periods cannot be separated by so many characters {}.",
                length_reason(label.len(), DNS_LABEL_LENGTH_LIMIT)
            ));
        }
    }
    if !ascii_domain.contains('.') {
        return Err("The part after the @-sign is not valid. It should have a period.".into());
    }
    if !ascii_domain.ends_with(|c: char| c.is_ascii_alphabetic()) {
        return Err("The part after the @-sign is not valid. It is not within a valid top-level domain.".into());
    }
    for d in SPECIAL_USE_DOMAIN_NAMES {
        if ascii_domain == d || ascii_domain.ends_with(&format!(".{d}")) {
            return Err(
                "The part after the @-sign is a special-use or reserved name that cannot be used with email.".into()
            );
        }
    }
    let i18n = match idna::decode(&cps(&ascii_domain)) {
        Ok(d) => d,
        Err(e) => return Err(format!("The part after the @-sign is not valid IDNA ({}).", e.0)),
    };
    let domain_i18n = unidata::to_string(&i18n);
    let bad = bad_set(&domain_i18n, |c| !is_hostname_intl(c));
    if !bad.is_empty() {
        return Err(format!("The part after the @-sign contains invalid characters: {bad}."));
    }
    check_unsafe_chars(&domain_i18n, false)?;
    if let Err(e) = idna::encode(&i18n) {
        return Err(format!(
            "The part after the @-sign became invalid after normalizing to international characters ({}).",
            e.0
        ));
    }
    Ok((ascii_domain, domain_i18n))
}

/// `ipaddress.IPv4Address(s)`'s `AddressValueError` text, `Ok` when it parses.
fn ipv4(s: &str) -> R<u32> {
    if s.is_empty() {
        return Err("Address cannot be empty".into());
    }
    let octets: Vec<&str> = s.split('.').collect();
    if octets.len() != 4 {
        return Err(format!("Expected 4 octets in {}", pystr::repr(s)));
    }
    let mut out = 0u32;
    for o in octets {
        let v = (|| {
            if o.is_empty() {
                return Err("Empty octet not permitted".to_owned());
            }
            if !o.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("Only decimal digits permitted in {}", pystr::repr(o)));
            }
            if o.len() > 3 {
                return Err(format!("At most 3 characters permitted in {}", pystr::repr(o)));
            }
            if o != "0" && o.starts_with('0') {
                return Err(format!("Leading zeros are not permitted in {}", pystr::repr(o)));
            }
            let n: u32 = o.parse().unwrap_or(0);
            if n > 255 {
                return Err(format!("Octet {n} (> 255) not permitted"));
            }
            Ok(n)
        })()
        .map_err(|e| format!("{e} in {}", pystr::repr(s)))?;
        out = out << 8 | v;
    }
    Ok(out)
}

/// `ipaddress.IPv6Address(s)`'s `AddressValueError` text.
fn ipv6(address: &str) -> R<()> {
    if address.contains('/') {
        return Err(format!("Unexpected '/' in {}", pystr::repr(address)));
    }
    let s = match address.split_once('%') {
        None => address,
        Some((a, scope)) if !scope.is_empty() && !scope.contains('%') => a,
        Some(_) => return Err(format!("Invalid IPv6 address: \"{}\"", pystr::repr(address))),
    };
    if s.is_empty() {
        return Err("Address cannot be empty".into());
    }
    let n = s.chars().count();
    if n > 45 {
        let mut shorten = s.to_owned();
        if n > 100 {
            let head: String = s.chars().take(45).collect();
            let tail: String = s.chars().skip(n - 45).collect();
            shorten = format!("{head}({} chars elided){tail}", n - 90);
        }
        return Err(format!("At most 45 characters expected in {}", pystr::repr(&shorten)));
    }
    let mut parts: Vec<String> = s.splitn(10, ':').map(str::to_owned).collect();
    if parts.len() < 3 {
        return Err(format!("At least 3 parts expected in {}", pystr::repr(s)));
    }
    if parts.last().is_some_and(|p| p.contains('.')) {
        let v4 = parts.pop().unwrap_or_default();
        let ip = ipv4(&v4).map_err(|e| format!("{e} in {}", pystr::repr(s)))?;
        parts.push(format!("{:x}", (ip >> 16) & 0xFFFF));
        parts.push(format!("{:x}", ip & 0xFFFF));
    }
    if parts.len() > 9 {
        return Err(format!("At most 8 colons permitted in {}", pystr::repr(s)));
    }
    let mut skip = None;
    for (i, p) in parts.iter().enumerate().take(parts.len() - 1).skip(1) {
        if p.is_empty() {
            if skip.is_some() {
                return Err(format!("At most one '::' permitted in {}", pystr::repr(s)));
            }
            skip = Some(i);
        }
    }
    let lead = || format!("Leading ':' only permitted as part of '::' in {}", pystr::repr(s));
    let trail = || format!("Trailing ':' only permitted as part of '::' in {}", pystr::repr(s));
    let (hi, lo) = if let Some(k) = skip {
        let (mut hi, mut lo) = (k, parts.len() - k - 1);
        if parts[0].is_empty() {
            hi -= 1;
            if hi != 0 {
                return Err(lead());
            }
        }
        if parts[parts.len() - 1].is_empty() {
            lo -= 1;
            if lo != 0 {
                return Err(trail());
            }
        }
        if 8 < hi + lo + 1 {
            return Err(format!("Expected at most 7 other parts with '::' in {}", pystr::repr(s)));
        }
        (hi, lo)
    } else {
        if parts.len() != 8 {
            return Err(format!("Exactly 8 parts expected without '::' in {}", pystr::repr(s)));
        }
        if parts[0].is_empty() {
            return Err(lead());
        }
        if parts[7].is_empty() {
            return Err(trail());
        }
        (8, 0)
    };
    let hextets = parts[..hi].iter().chain(parts[parts.len() - lo..].iter());
    for h in hextets {
        if !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("Only hex digits permitted in {} in {}", pystr::repr(h), pystr::repr(s)));
        }
        if h.chars().count() > 4 {
            return Err(format!("At most 4 characters permitted in {} in {}", pystr::repr(h), pystr::repr(s)));
        }
    }
    Ok(())
}

/// `validate_email_domain_literal`; `Ok` when it parses.
fn domain_literal(lit: &str) -> R<()> {
    let digits = lit.strip_suffix('\n').unwrap_or(lit);
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return ipv4(lit).map(|_| ()).map_err(|e| {
            format!(
                "The address in brackets after the @-sign is not valid: It is not an IPv4 address ({e}) or is missing an address literal tag."
            )
        });
    }
    if let Some(v6) = lit.strip_prefix("IPv6:") {
        return ipv6(v6).map_err(|e| format!("The IPv6 address in brackets after the @-sign is not valid ({e})."));
    }
    if !lit.contains(':') {
        return Err(
            "The part after the @-sign in brackets is not an IPv4 address and has no address literal tag.".into()
        );
    }
    let bad = bad_set(lit, |c| !('\u{21}'..='\u{FA}').contains(&c));
    if !bad.is_empty() {
        return Err(format!("The part after the @-sign contains invalid characters in brackets: {bad}."));
    }
    Err("The part after the @-sign contains an invalid address literal tag in brackets.".into())
}

/// `validate_email_length`.
fn check_length(addr: &str, reason: Option<&str>) -> R<()> {
    let chars = addr.chars().count();
    let bytes = addr.len();
    if bytes <= EMAIL_MAX_LENGTH {
        return Ok(());
    }
    let diff = bytes - EMAIL_MAX_LENGTH;
    let s = if diff > 1 { "s" } else { "" };
    let reason = match reason {
        None if chars == bytes => length_reason(chars, EMAIL_MAX_LENGTH),
        None => {
            let mbpc = addr.chars().map(char::len_utf8).max().unwrap_or(1);
            let mchars = (diff / mbpc).max(1);
            if mchars == diff {
                format!("({diff} character{s} too many)")
            } else {
                format!("({mchars}-{diff} character{s} too many)")
            }
        }
        Some(r) => format!("{r} ({diff} byte{s} too many)"),
    };
    Err(format!("The email address is too long {reason}."))
}

fn validate_email(email: &str) -> R<String> {
    let split = split_email(email)?;
    let original = if split.quoted {
        format!("\"{}\"@{}", split.local, split.domain)
    } else {
        format!("{}@{}", split.local, split.domain)
    };
    let mut info = local_part(&split.local, split.quoted)?;
    let normalized_local = nfc_str(&info.local);
    if normalized_local != info.local {
        if let Err(e) = local_part(&normalized_local, split.quoted) {
            return Err(format!("After Unicode normalization: {e}"));
        }
        info.local = normalized_local;
    }
    if split.quoted {
        return Err("Quoting the part before the @-sign is not allowed here.".into());
    }
    if let Some(a) = &info.ascii
        && CASE_INSENSITIVE_MAILBOX_NAMES.contains(&a.to_ascii_lowercase().as_str())
    {
        info.ascii = Some(a.to_ascii_lowercase());
        info.local = info.local.to_lowercase();
    }
    if split.domain.is_empty() {
        return Err("There must be something after the @-sign.".into());
    }
    let (ascii_domain, domain) = if split.domain.starts_with('[') && split.domain.ends_with(']') {
        domain_literal(&split.domain[1..split.domain.len() - 1])?;
        return Err("A bracketed IP address after the @-sign is not allowed here.".into());
    } else {
        domain_name(&split.domain)?
    };
    let normalized = format!("{}@{domain}", info.local);
    let ascii_form = format!("{}@{ascii_domain}", info.ascii.as_deref().unwrap_or(&info.local));
    check_length(&original, None)?;
    check_length(&normalized, Some("after normalization"))?;
    check_length(&ascii_form, Some("when the part after the @-sign is converted to IDNA ASCII"))?;
    if split.display_name.is_some() {
        return Err("A display name and angle brackets around the email address are not permitted here.".into());
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests;

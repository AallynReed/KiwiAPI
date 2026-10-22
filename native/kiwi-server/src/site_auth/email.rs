//! pydantic `EmailStr` (pydantic 2.14 `validate_email` over email-validator
//! 2.3 with `check_deliverability=False`): whether an address validates, and
//! the normalized form it validates to. Every rejection is the same error to
//! the caller, so only acceptance and the result are reproduced.

use kiwi_core::pystr;
use unicode_normalization::UnicodeNormalization;

use super::email_table as t;

const MAX_INPUT: usize = 2048;
const EMAIL_MAX: usize = 254;
const DOMAIN_MAX: usize = 253;
const LABEL_MAX: usize = 63;
const SPECIAL_USE: [&str; 6] = ["arpa", "invalid", "local", "localhost", "onion", "test"];
const NAME_PUNCT: &str = ".!#$%&'*+-/=?^_`{|}~";
const ATEXT_PUNCT: &str = "_!#$%&'*+-/=?^`{|}~";
const CASE_INSENSITIVE: [&str; 15] = [
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

/// `TypeAdapter(EmailStr).validate_python(value)`: the normalized address.
pub fn validate(value: &str) -> Option<String> {
    if value.chars().count() > MAX_INPUT || value.contains(['\r', '\n']) {
        return None;
    }
    let inner = pretty_address(value).unwrap_or(value);
    validate_email(pystr::strip(inner))
}

fn is_ws(c: char) -> bool {
    pystr::is_space(c)
}

fn is_name_char(c: char) -> bool {
    pystr::is_word(c) || NAME_PUNCT.contains(c)
}

/// The `<...>` part of pydantic's `pretty_email_regex` fullmatch, if it matches.
fn pretty_address(s: &str) -> Option<&str> {
    let end = s.strip_suffix('>')?.len();
    let start = s.len() - s.trim_start_matches(is_ws).len();
    let inner = |lt: usize| (lt + 1 < end).then(|| &s[lt + 1..end]);
    let lt_after_ws = |from: usize| {
        let rest = &s[from..];
        let skip = rest.len() - rest.trim_start_matches(is_ws).len();
        rest[skip..].starts_with('<').then_some(from + skip)
    };
    if let Some(found) = unquoted_name(s, start).and_then(lt_after_ws).and_then(inner) {
        return Some(found);
    }
    if s[start..].starts_with('"')
        && let Some(first) = s[start + 1..].chars().next()
    {
        let from = start + 1 + first.len_utf8();
        let quotes: Vec<usize> = s[from..].match_indices('"').map(|(i, _)| from + i).collect();
        for &q in quotes.iter().rev() {
            if let Some(found) = lt_after_ws(q + 1).and_then(inner) {
                return Some(found);
            }
        }
    }
    s[start..].starts_with('<').then_some(start).and_then(inner)
}

/// `(?:N+\s+)*N+` from `at`: the end of a maximal run of name tokens.
fn unquoted_name(s: &str, at: usize) -> Option<usize> {
    let mut pos = at;
    let mut matched = None;
    loop {
        let rest = &s[pos..];
        let token = rest.len() - rest.trim_start_matches(is_name_char).len();
        if token == 0 {
            return matched;
        }
        pos += token;
        matched = Some(pos);
        let rest = &s[pos..];
        let ws = rest.len() - rest.trim_start_matches(is_ws).len();
        if ws == 0 || !rest[ws..].starts_with(is_name_char) {
            return matched;
        }
        pos += ws;
    }
}

/// Coarse general category: 0 = L/N/P/S, 1 = M, 2 = Zs, 3 = C/Zl/Zp.
fn class(c: char) -> u8 {
    let n = c as u32;
    if n < 0x80 {
        return match n {
            0x20 => 2,
            0..0x20 | 0x7f => 3,
            _ => 0,
        };
    }
    let i = t::CLASS_RUNS.partition_point(|&(start, _)| start <= n);
    t::CLASS_RUNS[i - 1].1
}

fn in_ranges(ranges: &[(u32, u32)], n: u32) -> bool {
    let i = ranges.partition_point(|&(lo, _)| lo <= n);
    i > 0 && n <= ranges[i - 1].1
}

/// `check_unsafe_chars`.
fn safe_chars(s: &str, allow_space: bool) -> bool {
    s.chars().enumerate().all(|(i, c)| match class(c) {
        0 => true,
        1 => i != 0,
        2 => allow_space,
        _ => false,
    })
}

fn is_atext(c: char) -> bool {
    c.is_ascii_alphanumeric() || ATEXT_PUNCT.contains(c)
}

fn is_atext_intl(c: char) -> bool {
    is_atext(c) || c as u32 >= 0x80
}

fn is_hostname_intl(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '.' || c as u32 >= 0x80
}

/// `[X]+(?:\.[X]+)*\Z`.
fn dot_atom(s: &str, ok: fn(char) -> bool) -> bool {
    s.split('.').all(|l| !l.is_empty() && l.chars().all(ok))
}

/// `DOT_ATOM_TEXT_HOSTNAME`.
fn ascii_hostname(s: &str) -> bool {
    s.split('.').all(|l| {
        let b = l.as_bytes();
        !b.is_empty()
            && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
            && b[0].is_ascii_alphanumeric()
            && b[b.len() - 1].is_ascii_alphanumeric()
    })
}

/// The local part as `validate_email_local_part` accepts it (quoting is
/// refused later regardless), NFC re-checked like `validate_email`.
fn local_part(local: &str) -> Option<(String, bool)> {
    let accepts = |s: &str| -> Option<bool> {
        if dot_atom(s, is_atext) {
            Some(false)
        } else if dot_atom(s, is_atext_intl) && safe_chars(s, false) {
            Some(true)
        } else {
            None
        }
    };
    let smtputf8 = accepts(local)?;
    let nfc: String = local.nfc().collect();
    if nfc != local {
        accepts(&nfc)?;
    }
    Some((nfc, smtputf8))
}

/// `check_dot_atom(..., is_hostname=True)`.
fn hostname_dots_ok(d: &str) -> bool {
    !(d.starts_with('.')
        || d.ends_with('.')
        || d.contains("..")
        || d.starts_with('-')
        || d.ends_with('-')
        || d.contains(".-")
        || d.contains("-."))
}

/// `re.match(r"(?!xn)..--", label, re.I)`.
fn reserved_ldh(label: &str) -> bool {
    let c: Vec<char> = label.chars().take(4).collect();
    c.len() == 4 && c[2] == '-' && c[3] == '-' && !(c[0].eq_ignore_ascii_case(&'x') && c[1].eq_ignore_ascii_case(&'n'))
}

/// `uts46_valid_char`.
fn uts46_valid_char(c: char) -> bool {
    let n = c as u32;
    if (0x80..=0x9f).contains(&n) {
        return false;
    }
    if ((0x2010..=0x2060).contains(&n) && !(0x2024..=0x2026).contains(&n) && !(0x2028..=0x202e).contains(&n))
        || matches!(n, 0xad | 0x2064 | 0xff0e)
        || (0x200b..=0x200d).contains(&n)
        || (0x1bca0..=0x1bca3).contains(&n)
    {
        return true;
    }
    if n == 0x20 || (n >= 0x80 && class(c) >= 2) {
        return false;
    }
    t::DOT_DECOMP.binary_search(&n).is_err()
}

/// `idna.core.valid_contexto`.
fn contexto_ok(label: &[char], pos: usize) -> bool {
    let n = label[pos] as u32;
    let script = |c: char, table: &[(u32, u32)]| in_ranges(table, c as u32);
    match n {
        0xb7 => pos > 0 && pos + 1 < label.len() && label[pos - 1] == 'l' && label[pos + 1] == 'l',
        0x375 => pos + 1 < label.len() && script(label[pos + 1], t::GREEK),
        0x5f3 | 0x5f4 => pos > 0 && script(label[pos - 1], t::HEBREW),
        0x30fb => label
            .iter()
            .any(|&c| c != '\u{30fb}' && (script(c, t::HIRAGANA) || script(c, t::KATAKANA) || script(c, t::HAN))),
        0x660..=0x669 => !label.iter().any(|&c| (0x6f0..=0x6f9).contains(&(c as u32))),
        0x6f0..=0x6f9 => !label.iter().any(|&c| (0x660..=0x669).contains(&(c as u32))),
        _ => false,
    }
}

const CONTEXTO: [(u32, u32); 6] =
    [(0xb7, 0xb7), (0x375, 0x375), (0x5f3, 0x5f4), (0x660, 0x669), (0x6f0, 0x6f9), (0x30fb, 0x30fb)];

/// `idna.core.check_label` on a U-label, without the Bidi Rule (the UTS 46
/// pass in [`domain_name`] applies RFC 5893 to the domain instead).
fn check_label(label: &str) -> bool {
    let chars: Vec<char> = label.chars().collect();
    if chars.is_empty() || label.nfc().ne(label.chars()) {
        return false;
    }
    let hyphen_34 = chars.len() >= 4 && chars[2] == '-' && chars[3] == '-';
    if hyphen_34 || label.starts_with('-') || label.ends_with('-') || class(chars[0]) == 1 {
        return false;
    }
    (0..chars.len()).all(|i| {
        let n = chars[i] as u32;
        in_ranges(t::PVALID, n) || (in_ranges(&CONTEXTO, n) && contexto_ok(&chars, i))
    })
}

/// `idna.core.ulabel` of one ASCII label: the U-label it names.
fn ulabel_ascii(label: &str) -> Option<String> {
    let lower = label.to_ascii_lowercase();
    let Some(puny) = lower.strip_prefix("xn--") else {
        return check_label(&lower).then_some(lower);
    };
    if puny.is_empty() || puny.ends_with('-') {
        return None;
    }
    let decoded = idna::punycode::decode_to_string(puny)?;
    if idna::punycode::encode_str(&decoded)? != puny {
        return None;
    }
    check_label(&decoded).then_some(decoded)
}

/// `idna.core.alabel` of one label.
fn alabel(label: &str) -> Option<String> {
    let out = if label.is_ascii() {
        ulabel_ascii(label)?;
        label.to_owned()
    } else {
        if !check_label(label) {
            return None;
        }
        format!("xn--{}", idna::punycode::encode_str(label)?)
    };
    (out.len() <= LABEL_MAX).then_some(out)
}

/// `validate_email_domain_name`: (ascii form, normalized Unicode form).
fn domain_name(domain: &str) -> Option<(String, String)> {
    if !domain.chars().all(is_hostname_intl) || !safe_chars(domain, false) || !domain.chars().all(uts46_valid_char) {
        return None;
    }
    let mapped = if domain.is_ascii() {
        domain.to_ascii_lowercase()
    } else {
        let (mapped, ok) = idna::domain_to_unicode(domain);
        ok.ok()?;
        mapped
    };
    if !mapped.chars().all(is_hostname_intl) || !hostname_dots_ok(&mapped) || mapped.split('.').any(reserved_ldh) {
        return None;
    }
    let ascii = if ascii_hostname(&mapped) {
        mapped
    } else {
        let labels: Option<Vec<String>> = mapped.split('.').map(alabel).collect();
        let ascii = labels?.join(".");
        if !ascii_hostname(&ascii) {
            return None;
        }
        ascii
    };
    if ascii.len() > DOMAIN_MAX || ascii.split('.').any(|l| l.len() > LABEL_MAX) {
        return None;
    }
    if !ascii.contains('.') || !ascii.ends_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    if SPECIAL_USE.iter().any(|d| ascii == *d || ascii.ends_with(&format!(".{d}"))) {
        return None;
    }
    let labels: Option<Vec<String>> = ascii.split('.').map(ulabel_ascii).collect();
    let unicode = labels?.join(".");
    if !unicode.chars().all(is_hostname_intl) || !safe_chars(&unicode, false) {
        return None;
    }
    unicode.split('.').try_for_each(|l| alabel(l).map(|_| ()))?;
    Some((ascii, unicode))
}

/// `email_validator.validate_email(email, check_deliverability=False)`.
fn validate_email(email: &str) -> Option<String> {
    // A quote or angle bracket fails some later check whatever its position:
    // quoted local parts, display names and these characters elsewhere are all refused.
    if email.contains(['"', '<']) {
        return None;
    }
    let (local, domain) = email.split_once('@')?;
    if local.is_empty() || domain.is_empty() || (domain.starts_with('[') && domain.ends_with(']')) {
        return None;
    }
    let (mut local_norm, smtputf8) = local_part(local)?;
    let mut ascii_local = if smtputf8 { None } else { Some(local.to_owned()) };
    if let Some(a) = &ascii_local
        && CASE_INSENSITIVE.contains(&a.to_ascii_lowercase().as_str())
    {
        ascii_local = Some(a.to_ascii_lowercase());
        local_norm = local_norm.to_lowercase();
    }
    let (ascii_domain, unicode_domain) = domain_name(domain)?;
    let normalized = format!("{local_norm}@{unicode_domain}");
    let ascii_local = ascii_local.as_deref().unwrap_or(&local_norm);
    let fits = |s: &str| s.len() <= EMAIL_MAX;
    (fits(&format!("{local}@{domain}")) && fits(&normalized) && fits(&format!("{ascii_local}@{ascii_domain}")))
        .then_some(normalized)
}

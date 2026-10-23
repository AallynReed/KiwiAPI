//! pydantic's `EmailStr` (`pydantic.networks.validate_email` over
//! email-validator 2.3.0, `check_deliverability=False`) for ASCII input.
//! Non-ASCII input, Punycode labels and bracketed domain literals need
//! Unicode/IDNA/ipaddress tables this port does not carry: they are reported
//! as [`Email::Unsupported`] so the caller can hand the request to Python.

use std::sync::LazyLock;

use regex::Regex;

use crate::pystr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Email {
    /// The normalized address (`ValidatedEmail.normalized`).
    Valid(String),
    /// The reason pydantic puts in `value is not a valid email address: {reason}`.
    Invalid(String),
    Unsupported,
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

/// Inputs [`validate`] cannot judge exactly.
pub fn needs_unicode(value: &str) -> bool {
    !value.is_ascii() || value.to_ascii_lowercase().contains("xn--") || value.contains('[')
}

static PRETTY: LazyLock<Regex> = LazyLock::new(|| {
    let s = r"[\t\n\x0B\x0C\r\x1C-\x1F ]";
    let name = r"[A-Za-z0-9_.!#$%&'*+\-/=?^_`{|}~]";
    let unquoted = format!(r"((?:{name}+{s}+)*{name}+)");
    let quoted = r#""((?:[^"\n]|")+)""#;
    Regex::new(&format!(r"\A{s}*(?:{unquoted}|{quoted})?{s}*<([^\n]+)>{s}*\z")).expect("pretty email regex")
});

pub fn validate(value: &str) -> Email {
    if needs_unicode(value) {
        return Email::Unsupported;
    }
    match validate_ascii(value) {
        Ok(email) => Email::Valid(email),
        Err(reason) => Email::Invalid(reason),
    }
}

/// `email_validator.validate_email(value, check_deliverability=False)` called
/// directly: no pydantic wrapper, so no display-name unwrapping or length cap.
pub fn validate_direct(value: &str) -> Email {
    if needs_unicode(value) {
        return Email::Unsupported;
    }
    match validate_email(value) {
        Ok(email) => Email::Valid(email),
        Err(reason) => Email::Invalid(reason),
    }
}

type R<T> = Result<T, String>;

fn validate_ascii(value: &str) -> R<String> {
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
}

fn is_atext(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_!#$%&'*+-/=?^`{|}~".contains(c)
}

/// `safe_character_display` for an ASCII character.
fn safe_display(c: char) -> String {
    if c == '\\' {
        return "\"\\\"".into();
    }
    if c.is_ascii_graphic() {
        return pystr::repr(&c.to_string());
    }
    if c == ' ' {
        return "SPACE".into();
    }
    format!("U+{:04X}", c as u32)
}

fn sorted_join(mut items: Vec<String>) -> String {
    items.sort();
    items.dedup();
    items.join(", ")
}

fn bad_set(s: &str, bad: impl Fn(char) -> bool) -> Vec<String> {
    s.chars().filter(|&c| bad(c)).map(safe_display).collect()
}

fn check_unsafe_chars(s: &str, allow_space: bool) -> R<()> {
    let mut bad: Vec<char> = s.chars().filter(|&c| !c.is_ascii_graphic() && !(c == ' ' && allow_space)).collect();
    if bad.is_empty() {
        return Ok(());
    }
    bad.sort();
    bad.dedup();
    let shown: Vec<String> = bad.into_iter().map(safe_display).collect();
    Err(format!("The email address contains unsafe characters: {}.", shown.join(", ")))
}

fn split_at_unquoted(text: &str, specials: &[char]) -> R<(String, String)> {
    let (mut inside_quote, mut escaped) = (false, false);
    let mut left = String::new();
    let mut found = false;
    for c in text.chars() {
        if inside_quote {
            left.push(c);
            if c == '\\' && !escaped {
                escaped = true;
            } else if c == '"' && !escaped {
                inside_quote = false;
                escaped = false;
            } else {
                escaped = false;
            }
        } else if c == '"' {
            left.push(c);
            inside_quote = true;
        } else if specials.contains(&c) {
            found = true;
            break;
        } else {
            left.push(c);
        }
    }
    if !found {
        return Err("An email address must have an @-sign.".into());
    }
    let right = text[left.len()..].to_owned();
    Ok((left, right))
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
    let (display_name, mut local, mut domain);
    if right.starts_with('<') {
        let left = left.trim_end_matches(pystr::is_space);
        let (name, name_quoted) = unquote(left)?;
        if !name_quoted {
            let bad = bad_set(&name, |c| (!(is_atext(c) || c == '.') && c != ' ') || c == '.');
            if !bad.is_empty() {
                return Err(format!(
                    "The display name contains invalid characters when not quoted: {}.",
                    sorted_join(bad)
                ));
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
        (local, domain) = split_at_unquoted(addr_spec, &['@'])?;
        display_name = Some(name);
    } else {
        display_name = None;
        (local, domain) = (left, right);
    }
    if let Some(d) = domain.strip_prefix('@') {
        domain = d.to_owned();
    }
    let quoted;
    (local, quoted) = unquote(&local)?;
    Ok(Split { display_name, local, domain, quoted })
}

fn dot_atom(s: &str) -> bool {
    !s.is_empty() && s.split('.').all(|p| !p.is_empty() && p.chars().all(is_atext))
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

/// `validate_email_local_part` (ASCII, `allow_smtputf8`, not strict).
fn local_part(local: &str, quoted: bool) -> R<String> {
    if local.is_empty() {
        return Err("There must be something before the @-sign.".into());
    }
    if dot_atom(local) {
        return Ok(local.to_owned());
    }
    if quoted {
        let bad = bad_set(local, |c| !(' '..='~').contains(&c));
        if !bad.is_empty() {
            return Err(format!(
                "The email address contains invalid characters in quotes before the @-sign: {}.",
                sorted_join(bad)
            ));
        }
        check_unsafe_chars(local, true)?;
        let mut out = String::from('"');
        for c in local.chars() {
            if c == '"' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('"');
        return Ok(out);
    }
    let bad = bad_set(local, |c| !(is_atext(c) || c == '.'));
    if !bad.is_empty() {
        return Err(format!("The email address contains invalid characters before the @-sign: {}.", sorted_join(bad)));
    }
    check_dot_atom(
        local,
        "An email address cannot start with a {}.",
        "An email address cannot have a {} immediately before the @-sign.",
        false,
    )?;
    Err("The email address contains invalid characters before the @-sign.".into())
}

fn length_reason(len: usize, limit: usize) -> String {
    let diff = len - limit;
    format!("({diff} character{} too many)", if diff > 1 { "s" } else { "" })
}

/// `validate_email_domain_name` (ASCII, not a test environment, globally deliverable).
fn domain_name(original: &str) -> R<String> {
    let bad = bad_set(original, |c| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'));
    if !bad.is_empty() {
        return Err(format!("The part after the @-sign contains invalid characters: {}.", sorted_join(bad)));
    }
    let domain = original.to_ascii_lowercase();
    check_dot_atom(
        &domain,
        "An email address cannot have a {} immediately after the @-sign.",
        "An email address cannot end with a {}.",
        true,
    )?;
    for label in domain.split('.') {
        let b = label.as_bytes();
        if b.len() >= 4 && &b[2..4] == b"--" && !b[..2].eq_ignore_ascii_case(b"xn") {
            return Err("An email address cannot have two letters followed by two dashes immediately after the @-sign or after a period, except Punycode.".into());
        }
    }
    if domain.len() > DOMAIN_MAX_LENGTH {
        if domain == original {
            return Err(format!(
                "The email address is too long after the @-sign {}.",
                length_reason(domain.len(), DOMAIN_MAX_LENGTH)
            ));
        }
        let diff = domain.len() - DOMAIN_MAX_LENGTH;
        let s = if diff == 1 { "" } else { "s" };
        return Err(format!(
            "The email address is too long after the @-sign ({diff} byte{s} too many after IDNA encoding)."
        ));
    }
    for label in domain.split('.') {
        if label.len() > DNS_LABEL_LENGTH_LIMIT {
            return Err(format!(
                "After the @-sign, periods cannot be separated by so many characters {}.",
                length_reason(label.len(), DNS_LABEL_LENGTH_LIMIT)
            ));
        }
    }
    if !domain.contains('.') {
        return Err("The part after the @-sign is not valid. It should have a period.".into());
    }
    if !domain.ends_with(|c: char| c.is_ascii_alphabetic()) {
        return Err("The part after the @-sign is not valid. It is not within a valid top-level domain.".into());
    }
    for d in SPECIAL_USE_DOMAIN_NAMES {
        if domain == d || domain.ends_with(&format!(".{d}")) {
            return Err(
                "The part after the @-sign is a special-use or reserved name that cannot be used with email.".into()
            );
        }
    }
    Ok(domain)
}

fn validate_email(email: &str) -> R<String> {
    let split = split_email(email)?;
    let original = format!("{}@{}", split.local, split.domain);
    let mut local = local_part(&split.local, split.quoted)?;
    if split.quoted {
        return Err("Quoting the part before the @-sign is not allowed here.".into());
    }
    if CASE_INSENSITIVE_MAILBOX_NAMES.contains(&local.to_ascii_lowercase().as_str()) {
        local = local.to_ascii_lowercase();
    }
    if split.domain.is_empty() {
        return Err("There must be something after the @-sign.".into());
    }
    let domain = domain_name(&split.domain)?;
    let normalized = format!("{local}@{domain}");
    if original.len() > EMAIL_MAX_LENGTH {
        return Err(format!("The email address is too long {}.", length_reason(original.len(), EMAIL_MAX_LENGTH)));
    }
    for (addr, why) in [
        (&normalized, "after normalization"),
        (&normalized, "when the part after the @-sign is converted to IDNA ASCII"),
    ] {
        if addr.len() > EMAIL_MAX_LENGTH {
            let diff = addr.len() - EMAIL_MAX_LENGTH;
            let s = if diff > 1 { "s" } else { "" };
            return Err(format!("The email address is too long {why} ({diff} byte{s} too many)."));
        }
    }
    if split.display_name.is_some() {
        return Err("A display name and angle brackets around the email address are not permitted here.".into());
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        assert_eq!(validate("Foo@Example.COM"), Email::Valid("Foo@example.com".into()));
        assert_eq!(validate("Support@x.io"), Email::Valid("support@x.io".into()));
        assert_eq!(validate("Ann <a@b.co>"), Email::Valid("a@b.co".into()));
        assert_eq!(
            validate("a@b.test"),
            Email::Invalid(
                "The part after the @-sign is a special-use or reserved name that cannot be used with email.".into()
            )
        );
        assert_eq!(validate("é@x.com"), Email::Unsupported);
        assert_eq!(validate("a@xn--bcher-kva.de"), Email::Unsupported);
    }
}

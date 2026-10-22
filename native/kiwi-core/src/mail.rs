//! The text of outgoing mail: `app/core/email_render.py`'s branded HTML
//! (Jinja2 output byte for byte, markupsafe escaping), the plain-text part
//! `app/auth/emails.py` pairs with it, and the rate-limit digest of
//! `app/core/maintenance.py`.

/// `markupsafe.escape`.
pub fn escape_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
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
    out
}

/// `html.escape(s)` (quote=True).
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

/// The call-to-action of a mail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    pub label: String,
    pub url: String,
}

/// The site names the template embeds.
#[derive(Clone, Debug)]
pub struct Brand<'a> {
    pub app_name: &'a str,
    pub dev_url: &'a str,
    pub docs_url: &'a str,
}

/// `render_email(heading, paragraphs, button, note)`.
pub fn render_email(
    brand: &Brand<'_>,
    heading: &str,
    paragraphs: &[String],
    button: Option<&Button>,
    note: Option<&str>,
) -> String {
    let e = escape_markup;
    let mut out = String::with_capacity(3000);
    out.push_str(concat!(
        "<!doctype html>\n<html lang=\"en\">\n<body style=\"margin:0;padding:0;background:#0d1117;\n",
        "  font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;\">\n",
        "  <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\"\n",
        "    style=\"background:#0d1117;padding:28px 0;\">\n    <tr><td align=\"center\">\n",
        "      <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\"\n",
        "        style=\"max-width:520px;background:#161b22;border:1px solid #232a33;border-radius:14px;\">\n",
        "        <tr><td style=\"padding:26px 30px 8px;\">\n",
        "          <div style=\"font-size:1.25rem;font-weight:700;color:#e6edf3;\">\n",
        "            <span style=\"color:#58a6ff;\">&#9670;</span> ",
    ));
    out.push_str(&e(brand.app_name));
    out.push_str(concat!(
        "\n          </div>\n        </td></tr>\n        <tr><td style=\"padding:8px 30px 4px;\">\n",
        "          <h1 style=\"margin:0 0 6px;font-size:1.3rem;color:#e6edf3;\">",
    ));
    out.push_str(&e(heading));
    out.push_str(concat!(
        "</h1>\n        </td></tr>\n",
        "        <tr><td style=\"padding:4px 30px 8px;color:#c2ccd6;line-height:1.6;font-size:.97rem;\">\n          ",
    ));
    for p in paragraphs {
        out.push_str("<p style=\"margin:0 0 14px;\">");
        out.push_str(&e(p));
        out.push_str("</p>");
    }
    out.push_str("\n          ");
    if let Some(b) = button {
        out.push_str("\n          <p style=\"margin:22px 0;\">\n            <a href=\"");
        out.push_str(&e(&b.url));
        out.push_str(concat!(
            "\" style=\"display:inline-block;background:#238636;color:#fff;\n",
            "              text-decoration:none;padding:11px 20px;border-radius:8px;font-weight:600;\">",
        ));
        out.push_str(&e(&b.label));
        out.push_str(concat!(
            "</a>\n          </p>\n",
            "          <p style=\"margin:0 0 14px;color:#8b949e;font-size:.85rem;word-break:break-all;\">\n",
            "            Or paste this link into your browser:<br>",
        ));
        out.push_str(&e(&b.url));
        out.push_str("</p>\n          ");
    }
    out.push_str("\n          ");
    if let Some(n) = note.filter(|n| !n.is_empty()) {
        out.push_str("<p style=\"margin:14px 0 0;color:#8b949e;font-size:.85rem;\">");
        out.push_str(&e(n));
        out.push_str("</p>");
    }
    out.push_str(concat!(
        "\n        </td></tr>\n",
        "        <tr><td style=\"padding:18px 30px 26px;border-top:1px solid #232a33;color:#6e7681;font-size:.8rem;\">\n          ",
    ));
    out.push_str(&e(brand.app_name));
    out.push_str(" \u{b7} <a href=\"");
    out.push_str(&e(brand.dev_url));
    out.push_str("\" style=\"color:#58a6ff;\">Developer portal</a>\n          \u{b7} <a href=\"");
    out.push_str(&e(brand.docs_url));
    out.push_str("\" style=\"color:#58a6ff;\">Docs</a>\n        </td></tr>\n      </table>\n    </td></tr>\n  </table>\n</body>\n</html>");
    out
}

/// The plain-text part `_queue` builds next to the HTML.
pub fn plain_text(paragraphs: &[String], button: Option<&Button>, note: Option<&str>) -> String {
    let mut text = paragraphs.join("\n\n");
    if let Some(b) = button {
        text.push_str(&format!("\n\n{}: {}", b.label, b.url));
    }
    if let Some(n) = note.filter(|n| !n.is_empty()) {
        text.push_str(&format!("\n\n{n}"));
    }
    text
}

/// One `send_*` mail of `app/auth/emails.py`, before rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub subject: String,
    pub heading: String,
    pub paragraphs: Vec<String>,
    pub button: Option<Button>,
    pub note: Option<String>,
}

impl Notice {
    pub fn text(&self) -> String {
        plain_text(&self.paragraphs, self.button.as_ref(), self.note.as_deref())
    }

    pub fn html(&self, brand: &Brand<'_>) -> String {
        render_email(brand, &self.heading, &self.paragraphs, self.button.as_ref(), self.note.as_deref())
    }
}

/// `send_token_expiring_email`.
pub fn token_expiring(app_name: &str, dev_url: &str, token_name: &str, prefix: &str, days_left: i64) -> Notice {
    let when = if days_left <= 0 { "today".to_owned() } else { format!("in {days_left} day(s)") };
    Notice {
        subject: format!("Your {app_name} API token expires {when}"),
        heading: "API token expiring".into(),
        paragraphs: vec![
            format!("Your {app_name} API token \"{token_name}\" ({prefix}\u{2026}) expires {when}."),
            "Once it expires it will stop authenticating requests. Rotate or replace it from the developer portal."
                .into(),
        ],
        button: Some(Button { label: "Open the portal".into(), url: dev_url.into() }),
        note: Some("If you no longer need this token, you can ignore this message.".into()),
    }
}

/// One row of the 429 digest: hits, account email, token name and prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestRow {
    pub count: i64,
    pub email: String,
    pub token_name: String,
    pub token_prefix: String,
}

/// `daily_rate_limit_digest`'s subject, text and HTML. `hours` is the
/// setting as Python formats it.
pub fn rate_limit_digest(app_name: &str, hours: &str, total: i64, rows: &[DigestRow]) -> (String, String, String) {
    let lines: Vec<String> = rows
        .iter()
        .map(|r| format!("{:>6}  {}  -  {} ({}\u{2026})", r.count, r.email, r.token_name, r.token_prefix))
        .collect();
    let text = format!(
        "{total} rate-limit hits (429s) in the last {hours}h across {} token(s).\n\n{:>6}  account  -  token\n{}",
        rows.len(),
        "hits",
        lines.join("\n")
    );
    let html_rows: String = rows
        .iter()
        .map(|r| {
            format!(
                "<tr><td align='right'>{}</td><td>{}</td><td>{} <code>{}\u{2026}</code></td></tr>",
                r.count,
                escape_html(&r.email),
                escape_html(&r.token_name),
                escape_html(&r.token_prefix)
            )
        })
        .collect();
    let html = format!(
        "<p><b>{total}</b> rate-limit hits (429s) in the last {hours}h across {} token(s).</p>\
         <table cellpadding='6' style='border-collapse:collapse'>\
         <tr><th align='right'>Hits</th><th align='left'>Account</th><th align='left'>Token</th></tr>{html_rows}</table>",
        rows.len()
    );
    let subject = format!("{app_name} - rate-limit digest ({total} hits / {hours}h)");
    (subject, text, html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        assert_eq!(escape_markup("<a href=\"x\">'&'</a>"), "&lt;a href=&#34;x&#34;&gt;&#39;&amp;&#39;&lt;/a&gt;");
        assert_eq!(escape_html("\"'"), "&quot;&#x27;");
    }

    #[test]
    fn text_part() {
        let b = Button { label: "Go".into(), url: "https://x".into() };
        assert_eq!(plain_text(&["a".into(), "b".into()], Some(&b), Some("n")), "a\n\nb\n\nGo: https://x\n\nn");
        assert_eq!(plain_text(&[], None, Some("")), "");
    }
}

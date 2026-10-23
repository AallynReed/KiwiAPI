//! `app/pageviews/classify.py`: the bot test, referrer -> (channel, source) and
//! User-Agent -> device / browser / OS labels, with the same tables.

use std::sync::LazyLock;

use regex::Regex;

use super::urlsplit::{parse_qs_first, urlsplit};
use crate::pystr;

static BOT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "(?i)bot|crawl|spider|slurp|archiver|fetch|scan|monitor|uptime|pingdom|lighthouse|pagespeed|\
         gtmetrix|headless|phantom|puppeteer|playwright|selenium|preview|externalhit|externalagent|\
         whatsapp|telegram|discord|skype|slack|read-aloud|inspectiontool|python|curl|wget|httpx|\
         okhttp|java/|go-http|axios|node-fetch|semrush|ahrefs|mj12|petal|bytespider|gptbot|\
         claude|perplexity|yandex|baidu",
    )
    .expect("static regex")
});

pub fn is_bot(ua: &str) -> bool {
    ua.is_empty() || !ua.starts_with("Mozilla/") || BOT_RE.is_match(ua)
}

pub const CLOUD_ASNS: [u64; 22] = [
    45090, 132203, 37963, 45102, 55990, 136907, 38365, 55967, 16509, 14618, 15169, 396982, 8075, 14061, 16276, 24940,
    63949, 20473, 31898, 51167, 12876, 209366,
];

const LABELS: [(&str, &str, &str); 30] = [
    ("google", "Google", "search"),
    ("bing", "Bing", "search"),
    ("duckduckgo", "DuckDuckGo", "search"),
    ("yahoo", "Yahoo", "search"),
    ("yandex", "Yandex", "search"),
    ("baidu", "Baidu", "search"),
    ("ecosia", "Ecosia", "search"),
    ("qwant", "Qwant", "search"),
    ("startpage", "Startpage", "search"),
    ("naver", "Naver", "search"),
    ("brave", "Brave Search", "search"),
    ("seznam", "Seznam", "search"),
    ("discord", "Discord", "social"),
    ("discordapp", "Discord", "social"),
    ("reddit", "Reddit", "social"),
    ("youtube", "YouTube", "social"),
    ("twitch", "Twitch", "social"),
    ("facebook", "Facebook", "social"),
    ("fb", "Facebook", "social"),
    ("instagram", "Instagram", "social"),
    ("tiktok", "TikTok", "social"),
    ("twitter", "X", "social"),
    ("steamcommunity", "Steam", "social"),
    ("steampowered", "Steam", "social"),
    ("vk", "VK", "social"),
    ("bsky", "Bluesky", "social"),
    ("threads", "Threads", "social"),
    ("kick", "Kick", "social"),
    ("pinterest", "Pinterest", "social"),
    ("linkedin", "LinkedIn", "social"),
];

const EXACT: [(&str, &str, &str); 19] = [
    ("t.co", "X", "social"),
    ("x.com", "X", "social"),
    ("youtu.be", "YouTube", "social"),
    ("lnkd.in", "LinkedIn", "social"),
    ("t.me", "Telegram", "social"),
    ("chatgpt.com", "ChatGPT", "ai"),
    ("chat.openai.com", "ChatGPT", "ai"),
    ("perplexity.ai", "Perplexity", "ai"),
    ("gemini.google.com", "Gemini", "ai"),
    ("claude.ai", "Claude", "ai"),
    ("copilot.microsoft.com", "Copilot", "ai"),
    ("chat.deepseek.com", "DeepSeek", "ai"),
    ("grok.com", "Grok", "ai"),
    ("you.com", "You.com", "ai"),
    ("phind.com", "Phind", "ai"),
    ("mail.google.com", "Gmail", "email"),
    ("outlook.live.com", "Outlook", "email"),
    ("outlook.office.com", "Outlook", "email"),
    ("mail.yahoo.com", "Yahoo Mail", "email"),
];

static UTM_MEDIUM: LazyLock<[(Regex, &'static str); 5]> = LazyLock::new(|| {
    let re = |s: &str| Regex::new(s).expect("static regex");
    [
        (re(r"^(cpc|ppc|cpm|paid.*|.*[_-]ads?|ads?|display|banner|sponsored)$"), "campaign"),
        (re(r"^(email|e-mail|newsletter|mail)$"), "email"),
        (re(r"^(social|sm|organic_social|community)$"), "social"),
        (re(r"^(organic|search)$"), "search"),
        (re(r"^(referral|link)$"), "referral"),
    ]
});

/// `_host(url)`: lowercased hostname without `www.`, `""` on a ValueError.
pub fn host_of(url: &str) -> String {
    let host = urlsplit(url).ok().and_then(|s| s.hostname()).unwrap_or_default().to_lowercase();
    host.strip_prefix("www.").map(str::to_owned).unwrap_or(host)
}

/// `_own_suffix()` for `app_url`.
pub fn own_suffix(app_url: &str) -> String {
    let host = host_of(app_url);
    if host.contains('.') {
        let labels: Vec<&str> = host.split('.').collect();
        labels[labels.len() - 2..].join(".")
    } else {
        host
    }
}

/// `is_own_host(host)`.
pub fn is_own_host(host: &str, suffix: &str) -> bool {
    if matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        return true;
    }
    !suffix.is_empty() && (host == suffix || host.ends_with(&format!(".{suffix}")))
}

/// `from_host(host)`: (source, channel).
pub fn from_host(host: &str) -> (String, &'static str) {
    if let Some((_, s, c)) = EXACT.iter().find(|(h, _, _)| *h == host) {
        return ((*s).to_owned(), c);
    }
    for label in host.split('.') {
        if let Some((_, s, c)) = LABELS.iter().find(|(l, _, _)| *l == label) {
            return ((*s).to_owned(), c);
        }
    }
    (pystr::prefix(host, 80).to_owned(), "referral")
}

/// `_clean(value, 60)`.
fn clean(value: &str) -> String {
    let kept: String = value.chars().filter(|&c| pystr::is_word(c) || " .:/-".contains(c)).collect();
    pystr::prefix(pystr::strip(&kept), 60).to_owned()
}

/// `classify(page_url, referrer)`: (channel, source).
pub fn classify(page_url: &str, referrer: &str, suffix: &str) -> (String, Option<String>) {
    let query = urlsplit(page_url).map(|s| s.query).unwrap_or_default();
    let utm_source = clean(&parse_qs_first(&query, "utm_source").unwrap_or_default()).to_lowercase();
    if !utm_source.is_empty() {
        let medium = clean(&parse_qs_first(&query, "utm_medium").unwrap_or_default()).to_lowercase();
        let (mut source, mut channel) = from_host(&utm_source);
        if source == pystr::prefix(&utm_source, 80) {
            source = utm_source.clone();
        }
        if !medium.is_empty()
            && let Some((_, mapped)) = UTM_MEDIUM.iter().find(|(re, _)| re.is_match(&medium))
        {
            channel = mapped;
        }
        return (channel.to_owned(), Some(source));
    }
    let host = if referrer.is_empty() { String::new() } else { host_of(referrer) };
    if host.is_empty() {
        return ("direct".to_owned(), None);
    }
    if is_own_host(&host, suffix) {
        return ("internal".to_owned(), None);
    }
    let (source, channel) = from_host(&host);
    (channel.to_owned(), Some(source))
}

/// `_TABLET`: `iPad|Tablet|PlayBook|Silk|Kindle|Android(?!.*Mobile)`.
fn tablet(ua: &str) -> bool {
    if ["iPad", "Tablet", "PlayBook", "Silk", "Kindle"].iter().any(|w| ua.contains(w)) {
        return true;
    }
    ua.match_indices("Android").any(|(i, _)| {
        let after = &ua[i + "Android".len()..];
        !after.split('\n').next().unwrap_or("").contains("Mobile")
    })
}

pub fn device(ua: &str, width: i64, touch: i64) -> &'static str {
    if tablet(ua) {
        return "tablet";
    }
    if ["Mobi", "iPhone", "iPod", "Android", "Opera Mini", "IEMobile"].iter().any(|w| ua.contains(w)) {
        return "mobile";
    }
    if ua.contains("Macintosh") && touch > 1 {
        return "tablet";
    }
    if 0 < width && width < 600 {
        return "mobile";
    }
    "desktop"
}

pub fn browser(ua: &str) -> &'static str {
    let any = |ws: &[&str]| ws.iter().any(|w| ua.contains(w));
    if any(&["FBAN", "FBAV", "FB_IAB", "FBIOS"]) {
        "Facebook app"
    } else if ua.contains("Instagram") {
        "Instagram app"
    } else if any(&["Edg/", "EdgA/", "EdgiOS/"]) {
        "Edge"
    } else if any(&["OPR/", "Opera"]) {
        "Opera"
    } else if ua.contains("YaBrowser") {
        "Yandex Browser"
    } else if ua.contains("SamsungBrowser") {
        "Samsung Internet"
    } else if any(&["Firefox", "FxiOS"]) {
        "Firefox"
    } else if any(&["CriOS", "Chrome/"]) {
        "Chrome"
    } else if ua.contains("Safari/") {
        "Safari"
    } else {
        "Other"
    }
}

pub fn os_name(ua: &str, touch: i64) -> &'static str {
    if ["iPhone", "iPad", "iPod"].iter().any(|w| ua.contains(w)) || (ua.contains("Macintosh") && touch > 1) {
        "iOS"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("CrOS") {
        "ChromeOS"
    } else if ua.contains("Mac OS") || ua.contains("Macintosh") {
        "macOS"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "Other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables() {
        let s = own_suffix("https://trove.aallyn.net");
        assert_eq!(s, "aallyn.net");
        assert!(is_own_host("api.aallyn.net", &s));
        assert!(!is_own_host("evilaallyn.net", &s));
        assert_eq!(classify("https://trove.aallyn.net/?utm_source=Discord&utm_medium=cpc", "", &s).0, "campaign");
        assert_eq!(
            classify("https://trove.aallyn.net/", "https://www.google.co.uk/", &s),
            ("search".into(), Some("Google".into()))
        );
        assert_eq!(classify("https://trove.aallyn.net/", "", &s), ("direct".into(), None));
        assert_eq!(device("Mozilla/5.0 (Linux; Android 14; Pixel) Mobile", 0, 0), "mobile");
        assert_eq!(device("Mozilla/5.0 (Linux; Android 14; Tab)", 0, 0), "tablet");
        assert!(is_bot("Mozilla/5.0 (compatible; Googlebot/2.1)"));
        assert!(!is_bot("Mozilla/5.0 (Windows NT 10.0) Chrome/120 Safari/537"));
    }
}

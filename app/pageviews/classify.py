"""Pure classifiers for a page-view beacon: bot test, referrer -> (channel, source),
and User-Agent -> device / browser / OS. Every output is a coarse label; the raw
User-Agent and referrer URL are never stored."""
from __future__ import annotations

import re
from urllib.parse import parse_qs, urlsplit

from app.core.config import settings

BOT_RE = re.compile(
    r"bot|crawl|spider|slurp|archiver|fetch|scan|monitor|uptime|pingdom|lighthouse|pagespeed|"
    r"gtmetrix|headless|phantom|puppeteer|playwright|selenium|preview|externalhit|externalagent|"
    r"whatsapp|telegram|discord|skype|slack|read-aloud|inspectiontool|python|curl|wget|httpx|"
    r"okhttp|java/|go-http|axios|node-fetch|semrush|ahrefs|mj12|petal|bytespider|gptbot|"
    r"claude|perplexity|yandex|baidu",
    re.I,
)


def is_bot(ua: str) -> bool:
    """Declared crawlers, link unfurlers, scripts and automation. Real browsers all
    send a ``Mozilla/`` UA, so anything else is a tool."""
    return not ua or not ua.startswith("Mozilla/") or bool(BOT_RE.search(ua))


# A browser on a cloud or crawler network is a scanner, not a player: Tencent, Alibaba,
# Huawei, Baidu, AWS, Google, Azure, DigitalOcean, OVH, Hetzner, Linode, Vultr, Oracle,
# Contabo, Scaleway, SEMrush (numbers checked against DB-IP ASN Lite).
CLOUD_ASNS = frozenset({
    45090, 132203, 37963, 45102, 55990, 136907, 38365, 55967, 16509, 14618, 15169,
    396982, 8075, 14061, 16276, 24940, 63949, 20473, 31898, 51167, 12876, 209366,
})


# --- referrer -> (channel, source) ---------------------------------------------

CHANNELS = ("direct", "search", "social", "ai", "referral", "email", "campaign")

# Host label -> (source name, channel). Matched against each dot-separated label, so
# google.co.uk, news.google.com and old.reddit.com all land on their brand.
_LABELS: dict[str, tuple[str, str]] = {
    "google": ("Google", "search"), "bing": ("Bing", "search"),
    "duckduckgo": ("DuckDuckGo", "search"), "yahoo": ("Yahoo", "search"),
    "yandex": ("Yandex", "search"), "baidu": ("Baidu", "search"),
    "ecosia": ("Ecosia", "search"), "qwant": ("Qwant", "search"),
    "startpage": ("Startpage", "search"), "naver": ("Naver", "search"),
    "brave": ("Brave Search", "search"), "seznam": ("Seznam", "search"),
    "discord": ("Discord", "social"), "discordapp": ("Discord", "social"),
    "reddit": ("Reddit", "social"), "youtube": ("YouTube", "social"),
    "twitch": ("Twitch", "social"), "facebook": ("Facebook", "social"),
    "fb": ("Facebook", "social"), "instagram": ("Instagram", "social"),
    "tiktok": ("TikTok", "social"), "twitter": ("X", "social"),
    "steamcommunity": ("Steam", "social"), "steampowered": ("Steam", "social"),
    "vk": ("VK", "social"), "bsky": ("Bluesky", "social"),
    "threads": ("Threads", "social"), "kick": ("Kick", "social"),
    "pinterest": ("Pinterest", "social"), "linkedin": ("LinkedIn", "social"),
}

# Whole hosts that a label match would get wrong or miss.
_EXACT: dict[str, tuple[str, str]] = {
    "t.co": ("X", "social"), "x.com": ("X", "social"), "youtu.be": ("YouTube", "social"),
    "lnkd.in": ("LinkedIn", "social"), "t.me": ("Telegram", "social"),
    "chatgpt.com": ("ChatGPT", "ai"), "chat.openai.com": ("ChatGPT", "ai"),
    "perplexity.ai": ("Perplexity", "ai"), "gemini.google.com": ("Gemini", "ai"),
    "claude.ai": ("Claude", "ai"), "copilot.microsoft.com": ("Copilot", "ai"),
    "chat.deepseek.com": ("DeepSeek", "ai"), "grok.com": ("Grok", "ai"),
    "you.com": ("You.com", "ai"), "phind.com": ("Phind", "ai"),
    "mail.google.com": ("Gmail", "email"), "outlook.live.com": ("Outlook", "email"),
    "outlook.office.com": ("Outlook", "email"), "mail.yahoo.com": ("Yahoo Mail", "email"),
}

_UTM_MEDIUM = (
    (re.compile(r"^(cpc|ppc|cpm|paid.*|.*[_-]ads?|ads?|display|banner|sponsored)$"), "campaign"),
    (re.compile(r"^(email|e-mail|newsletter|mail)$"), "email"),
    (re.compile(r"^(social|sm|organic_social|community)$"), "social"),
    (re.compile(r"^(organic|search)$"), "search"),
    (re.compile(r"^(referral|link)$"), "referral"),
)


def _host(url: str) -> str:
    try:
        host = (urlsplit(url).hostname or "").lower()
    except ValueError:
        return ""
    return host.removeprefix("www.")


def _own_suffix() -> str:
    host = _host(settings.app_url)
    return ".".join(host.split(".")[-2:]) if host.count(".") >= 1 else host


def is_own_host(host: str) -> bool:
    """This site, the API, the docs and the portal all count as one site."""
    if host in ("localhost", "127.0.0.1", "[::1]"):
        return True
    suffix = _own_suffix()
    return bool(suffix) and (host == suffix or host.endswith("." + suffix))


def from_host(host: str) -> tuple[str, str]:
    """(source, channel) for an external referring host."""
    if host in _EXACT:
        return _EXACT[host]
    for label in host.split("."):
        if label in _LABELS:
            return _LABELS[label]
    return host[:80], "referral"


def _clean(value: str, limit: int = 60) -> str:
    return re.sub(r"[^\w .:/-]", "", value).strip()[:limit]


def classify(page_url: str, referrer: str) -> tuple[str, str | None]:
    """The page view's (channel, source).

    ``internal`` means the visitor came from another page of this site; ``direct``
    means no referrer at all (typed, bookmarked, an app, or a privacy setting). A
    ``utm_source`` on the landing URL wins over the referrer, so a tagged link in a
    Discord announcement reads as Discord even though Discord sends no referrer."""
    try:
        query = parse_qs(urlsplit(page_url).query)
    except ValueError:
        query = {}
    utm_source = _clean((query.get("utm_source") or [""])[0]).lower()
    if utm_source:
        medium = _clean((query.get("utm_medium") or [""])[0]).lower()
        source, channel = from_host(utm_source)
        if source == utm_source[:80]:
            source = utm_source
        for pattern, mapped in _UTM_MEDIUM:
            if medium and pattern.match(medium):
                channel = mapped
                break
        return channel, source
    host = _host(referrer) if referrer else ""
    if not host:
        return "direct", None
    if is_own_host(host):
        return "internal", None
    source, channel = from_host(host)
    return channel, source


# --- User-Agent -> device / browser / OS -------------------------------------

_TABLET = re.compile(r"iPad|Tablet|PlayBook|Silk|Kindle|Android(?!.*Mobile)")
_MOBILE = re.compile(r"Mobi|iPhone|iPod|Android|Opera Mini|IEMobile")

_BROWSERS = (
    (re.compile(r"FBAN|FBAV|FB_IAB|FBIOS"), "Facebook app"),
    (re.compile(r"Instagram"), "Instagram app"),
    (re.compile(r"Edg(A|iOS)?/"), "Edge"),
    (re.compile(r"OPR/|Opera"), "Opera"),
    (re.compile(r"YaBrowser"), "Yandex Browser"),
    (re.compile(r"SamsungBrowser"), "Samsung Internet"),
    (re.compile(r"Firefox|FxiOS"), "Firefox"),
    (re.compile(r"CriOS|Chrome/"), "Chrome"),
    (re.compile(r"Safari/"), "Safari"),
)


def device(ua: str, width: int = 0, touch: int = 0) -> str:
    if _TABLET.search(ua):
        return "tablet"
    if _MOBILE.search(ua):
        return "mobile"
    if "Macintosh" in ua and touch > 1:
        return "tablet"        # iPadOS asks for the desktop site
    if 0 < width < 600:
        return "mobile"
    return "desktop"


def browser(ua: str) -> str:
    for pattern, name in _BROWSERS:
        if pattern.search(ua):
            return name
    return "Other"


def os_name(ua: str, touch: int = 0) -> str:
    if re.search(r"iPhone|iPad|iPod", ua) or ("Macintosh" in ua and touch > 1):
        return "iOS"
    if "Android" in ua:
        return "Android"
    if "Windows" in ua:
        return "Windows"
    if "CrOS" in ua:
        return "ChromeOS"
    if "Mac OS" in ua or "Macintosh" in ua:
        return "macOS"
    if "Linux" in ua:
        return "Linux"
    return "Other"

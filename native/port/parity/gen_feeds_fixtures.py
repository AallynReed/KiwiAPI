"""Golden fixtures for the feeds port, captured from the real Python code.

Two modes, both run with the production-pinned venv
(S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe):

    python native/port/parity/gen_feeds_fixtures.py golden
        -> native/kiwi-server/testdata/feeds/golden.json (feeds/events/httpx
           logic, the routes' response bodies) and kiwi-core's generated tables
           (native/kiwi-core/src/feeds/tables.rs).

    python native/port/parity/gen_feeds_fixtures.py html
        -> native/kiwi-server/testdata/feeds/html.json: html.parser +
           BeautifulSoup 4.12.3 trees.

The saved upstream payloads in testdata/feeds/raw/ were fetched once from
trovegame.com, trovesaurus.com and search.bilibili.com. The news sections
(unescape aside) are for the news loop, which stays in Python until the
events-bus wave; nothing in Rust reads them yet.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[3]
DATA = ROOT / "native" / "kiwi-server" / "testdata" / "feeds"
RAW = DATA / "raw"
UTC = timezone.utc

BILI_REWRITE = ("//www.bilibili.com/zh-Hans/video/", "//www.bilibili.com/video/")


def iso(d):
    return None if d is None else d.isoformat()


def err(e: BaseException) -> dict:
    return {"error": type(e).__name__, "msg": str(e)}


# --------------------------------------------------------------------------- html mode


def soup_dump(node):
    from bs4 import Tag

    if isinstance(node, Tag):
        return [node.name, [[k, v] for k, v in node.attrs.items()], [soup_dump(c) for c in node.contents]]
    return [type(node).__name__, str(node)]


def cards(html: str):
    from bs4 import BeautifulSoup

    soup = BeautifulSoup(html, "html.parser")
    out = []
    for card in soup.select(".bili-video-card"):
        a = card.select_one("a")
        t = card.select_one("h3.bili-video-card__info--tit")
        au = card.select_one(".bili-video-card__info--author")
        d = card.select_one(".bili-video-card__info--date")
        p = card.select_one("picture source")
        out.append({
            "href": a.get("href", "") if a else None,
            "title": t.get("title", "") if t else None,
            "author": au.text if au else None,
            "date": d.text if d else None,
            "srcset": p.get("srcset", "") if p else None,
        })
    return out


HTML_SNIPPETS = [
    "plain text only",
    "<p>a<b>bold</b>c</p>",
    "<div class='x  y\tz' id=main data-v-1>t</div>",
    "<a href=//www.bilibili.com/video/BV1/ class=\"\">link</a>",
    "<img src=a.png><source srcset=\"x\"><br/><p>after</p>",
    "<source x=1></source><p>s</p>",
    "<source><source/><p>nested?</p></source>",
    "<div><span>unclosed<p>para</div>tail",
    "</p>stray end<div>x</div></div></div>",
    "<!-- c1 --><!----><!--[--><!--]--><!--->x<!-->y",
    "<!--a--!>b<!-- -- >c-->d",
    "<!DOCTYPE html><html><head><title>T &amp; <b>x</b></title></head><body>b</body></html>",
    "<script>var a = '<div>'; if (a < b && c > d) {}</script><p>after script</p>",
    "<script>x</scripty>y</script >z",
    "<style>p{color:red}</style><textarea>&lt;a&gt; <b></textarea>",
    "<template>tpl text<b>in</b></template><rt>ruby</rt><rp>(</rp>",
    "&amp; &lt; &gt; &quot; &#39; &#x27; &#169; &copy &copyx &nbsp;&unknown; &a-b",
    "&#128; &#129; &#150; &#0; &#65; &#x1F600; &#1114112; &#x110000;",
    "a & b &# &#; &#x; &#xZ &",
    "<p title=\"&amp;&amp=x&copy;&copyx &#65&#x42;\">t</p>",
    "<p a=\"1\" a=\"2\" B=3 c d='' e = 'q' f= g =h>dup</p>",
    "<p x='unterminated>after</p>",
    "<p x= 'unterminated y=1>after</p>",
    "<p x='a'y=\"b\"z=c/>slashy</p>",
    "<a/b/c>weird</a>",
    "<A HREF='X'>Upper</A>",
    "<p>line1\n  <span>  </span>\n\n<span> </span></p>",
    "<pre>  \n  </pre><p>  \n  </p>",
    "<![CDATA[cdata text]]><p>x</p>",
    "<?php echo 1 ?><p>pi</p>",
    "<!weird decl><p>bogus</p>",
    "</>empty end</ 1>bogus end<p>z</p>",
    "<p>x</p><",
    "<p>x</p></",
    "<p>x</p></di",
    "<p>x</p><div",
    "<p>x</p><!--unclosed",
    "<p>x</p><!doctype",
    "<p>x</p><?unclosed",
    "<p>x</p><![CDATA[unclosed",
    "text &amp",
    "text &#12",
    "<div class=\"bili-video-card\"><a href=\"//www.bilibili.com/video/BV9/\"><picture><source srcset=\"//i0.hdslb.com/a.jpg@1w.avif\"></picture></a>"
    "<h3 class=\"bili-video-card__info--tit\" title=\"T &amp; t\">T</h3><span class=\"bili-video-card__info--author\"> au<!--c--> thor </span>"
    "<span class=\"bili-video-card__info--date\"> · 03-04</span><div class=\"bili-video-card\">inner</div></div>",
    "<xmp><b>raw</b></xmp><iframe><p>if</p></iframe><noembed>&amp;</noembed><plaintext><p>rest</p>",
]

HTML_ERRORS = [
    "<p>&#12a; then more</p>",
]


def run_html() -> dict:
    import bs4
    from bs4 import BeautifulSoup

    assert bs4.__version__ == "4.12.3", bs4.__file__
    page = (RAW / "bilibili_search.html").read_text(encoding="utf-8")
    out: dict = {"python": sys.version.split()[0], "trees": [], "errors": []}
    for snippet in HTML_SNIPPETS:
        soup = BeautifulSoup(snippet, "html.parser")
        out["trees"].append({"html": snippet, "tree": [soup_dump(c) for c in soup.contents]})
    for snippet in HTML_ERRORS:
        try:
            BeautifulSoup(snippet, "html.parser")
            out["errors"].append({"html": snippet, "error": None})
        except Exception as e:  # noqa: BLE001
            out["errors"].append({"html": snippet, "error": type(e).__name__})
    for name, html in (("raw", page), ("rewritten", page.replace(*BILI_REWRITE))):
        soup = BeautifulSoup(html, "html.parser")
        dumped = json.dumps([soup_dump(c) for c in soup.contents], ensure_ascii=False, separators=(",", ":"))
        out[f"page_{name}"] = {
            "tree_sha256": hashlib.sha256(dumped.encode("utf-8")).hexdigest(),
            "cards": cards(html),
        }
    return out


# --------------------------------------------------------------------------- golden mode


def write_entities() -> None:
    import html as html_mod
    from html.entities import html5

    from bs4.dammit import EntitySubstitution

    derived: dict[str, str] = {}
    for key, ch in sorted(html5.items()):
        derived.setdefault(key[:-1] if key.endswith(";") else key, ch)
    assert derived == EntitySubstitution.HTML_ENTITY_TO_CHARACTER, "bs4 entity table drifted"

    def lit(s: str) -> str:
        plain = (lambda c: " " <= c <= "~" and c not in '"\\')
        return '"' + "".join(c if plain(c) else f"\\u{{{ord(c):x}}}" for c in s) + '"'

    lines = [
        "//! Generated by native/port/parity/gen_feeds_fixtures.py from Python's",
        "//! `html.entities.html5`, `html._invalid_charrefs` / `_invalid_codepoints`",
        "//! and `unicodedata`. Do not edit.",
        "",
        "/// `html5`: entity name (with or without `;`) to text, sorted by name.",
        "#[rustfmt::skip]",
        "pub const HTML5: &[(&str, &str)] = &[",
    ]
    for key in sorted(html5):
        lines.append(f"    ({lit(key)}, {lit(html5[key])}),")
    lines.append("];")
    lines.append("")
    lines.append("/// `_invalid_charrefs`: code point to replacement text, sorted.")
    lines.append("#[rustfmt::skip]")
    lines.append("pub const INVALID_CHARREFS: &[(u32, &str)] = &[")
    for num in sorted(html_mod._invalid_charrefs):
        lines.append(f"    (0x{num:x}, {lit(html_mod._invalid_charrefs[num])}),")
    lines.append("];")
    lines.append("")
    lines.append("/// `_invalid_codepoints`, sorted.")
    lines.append("#[rustfmt::skip]")
    lines.append("pub const INVALID_CODEPOINTS: &[u32] = &[")
    cps = sorted(html_mod._invalid_codepoints)
    for i in range(0, len(cps), 8):
        lines.append("    " + " ".join(f"0x{c:x}," for c in cps[i:i + 8]))
    lines.append("];")
    import unicodedata

    zeros = [c for c in range(0x110000) if unicodedata.decimal(chr(c), None) == 0]
    assert all(unicodedata.decimal(chr(z + d), None) == d for z in zeros for d in range(10))
    lines.append("")
    lines.append(f"/// The `0` of every Unicode decimal digit run (`int()` accepts them), Unicode {unicodedata.unidata_version}.")
    lines.append("#[rustfmt::skip]")
    lines.append("pub const DECIMAL_ZEROS: &[u32] = &[")
    for i in range(0, len(zeros), 8):
        lines.append("    " + " ".join(f"0x{c:x}," for c in zeros[i:i + 8]))
    lines.append("];")
    path = ROOT / "native" / "kiwi-core" / "src" / "feeds" / "tables.rs"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")


UNESCAPE_CASES = [
    "", "no refs", "&amp;", "&amp", "&ampx", "&AMP;", "&amp;amp;", "&#38;", "&#x26;", "&#X26", "&#38", "&#0;",
    "&#13;", "&#128;", "&#129;", "&#x80;", "&#159;", "&#55296;", "&#xDFFF;", "&#1114111;", "&#1114112;",
    "&#99999999999999999999999;", "&#x0B;", "&#xFFFE;", "&#x1FFFF;", "&#65", "&#65x", "&notit;", "&notin;", "&noti",
    "&#;", "&#x;", "&", "a&b", "&nbsp;x", "&nbspx", "&AElig", "&Eacute;", "&eacute", "&;", "& amp;",
    "&abcdefghijklmnopqrstuvwxyzabcdefghij;", "&ltltlt", "&lt;&gt;&quot;", "&ccedil;&ccedil", "&\u00e9;",
    "&\u4e2d\u6587;", "x&#8217;s &#8230; &#160;", "&acE;", "&NotEqualTilde;", "&nGt;", "&ThickSpace;",
    "&amp\n;", "&quot&quot;", "&#x1F600;&#128512;",
]

STRIP_HTML_CASES = [
    "", "plain", "<p>Hello <b>world</b></p>", "a<br>b<BR/>c<br />d<br   />e", "  lots \n of \t space  ",
    "&lt;p&gt;escaped&lt;/p&gt;", "x&nbsp;y", "<img src='a'>tail", "a\u3000b\u00a0c\x1cd",
    "<p>Greetings, Trovians! &#8230; [&#8230;]</p>",
]

TRUNCATE_CASES = [
    ("short", 220),
    ("x" * 220, 220),
    ("x" * 221, 220),
    ("word " * 60, 220),
    ("a" * 230 + " tail", 220),
    ("one two three four", 10),
    ("one two three four", 9),
    ("\u4e2d" * 300, 220),
    ("abc def  ", 7),
]

RFC2822_CASES = [
    "Tue, 20 Oct 2026 15:37:59 +0000",
    "Tue, 20 Oct 2026 15:37:59 GMT",
    "Tue, 20 Oct 2026 15:37:59 UT",
    "20 Oct 2026 15:37:59 +0200",
    "Tue, 20 Oct 2026 15:37 +0000",
    "Tue, 20 Oct 26 15:37:59 +0000",
    "Tue, 20 Oct 70 15:37:59 +0000",
    "Tue, 20 Oct 2026 15:37:59 EST",
    "Tue, 20 Oct 2026 15:37:59 PDT",
    "Tue, 20 Oct 2026 15:37:59 -0530",
    "Tue, 20 Oct 2026 15:37:59",
    "Tue, 20 Oct 2026 24:00:00 +0000",
    "Tue, 31 Feb 2026 10:00:00 +0000",
    "Tue, 20 October 2026 15:37:59 +0000",
    "Tue,20 Oct 2026 15:37:59 +0000",
    "Oct 20 2026 15:37:59 +0000",
    "20 Oct 2026 15:37:59.5 +0000",
    "garbage",
    "",
    "Tue, 20 Oct 2026 15:37:59 +00",
    "Tue, 20 Oct 2026 15:37:59 XYZ",
    "Tue, 20-Oct-2026 15:37:59 +0000",
    "Tue, 20 Oct 2026 15.37.59 +0000",
    "Tue, 20 Oct 2026 1:2:3 +0000",
]


def run_news(out: dict) -> None:
    from email.utils import parsedate_to_datetime
    from html import unescape

    from app.trove import news

    out["unescape"] = [[s, unescape(s)] for s in UNESCAPE_CASES]
    out["strip_html"] = [[s, news._strip_html(s)] for s in STRIP_HTML_CASES]
    out["truncate"] = [[s, n, news._truncate(s, n)] for s, n in TRUNCATE_CASES]
    rows = []
    for s in RFC2822_CASES:
        try:
            d = parsedate_to_datetime(s.strip())
            if d.tzinfo is None:
                rows.append([s, {"naive": d.isoformat()}])
            else:
                rows.append([s, {"ok": d.astimezone(UTC).isoformat()}])
        except (TypeError, ValueError) as e:
            rows.append([s, {"error": type(e).__name__}])
    out["rfc2822"] = rows
    items = news.parse_feed((RAW / "news_feed.xml").read_text(encoding="utf-8"))
    out["news_feed"] = [{**it, "published_at": iso(it["published_at"])} for it in items]


EVENT_CASES = [
    {"id": 7, "name": "  Int id  ", "url": " https://x ", "startdate": 100, "enddate": "200",
     "category": "", "image": "", "icon": "  i  ", "lookup": None},
    {"id": 0, "name": "zero id", "startdate": "1", "enddate": "2"},
    {"id": "   ", "name": "blank id", "startdate": "1", "enddate": "2"},
    {"id": "a1", "startdate": " 12 ", "enddate": "1_000"},
    {"id": "a2", "startdate": "+5", "enddate": "-5"},
    {"id": "a3", "startdate": 1.5, "enddate": "2"},
    {"id": "a4", "startdate": "1.0", "enddate": "2"},
    {"id": "a5", "startdate": "abc", "enddate": "2"},
    {"id": "a6", "startdate": "1"},
    {"id": "a7", "startdate": True, "enddate": "2"},
    {"id": "a8", "startdate": None, "enddate": "2"},
    {"id": 1.25, "name": "float id", "startdate": "1", "enddate": "2"},
    {"id": True, "name": "bool id", "startdate": "1", "enddate": "2"},
    {"id": "a9", "name": None, "category": None, "startdate": "1", "enddate": "2", "image": " x "},
    {"id": "a10", "startdate": "1__0", "enddate": "2"},
    {"id": "a11", "startdate": "_1", "enddate": "2"},
    {"id": "a12", "startdate": "9223372036854775807", "enddate": "2"},
    {"id": "a13", "startdate": "\u0661\u0662", "enddate": "2"},
]


def run_events(out: dict) -> None:
    from app.trove import events

    real = json.loads((RAW / "trovesaurus_calendar.json").read_bytes())
    out["events_real"] = events.parse_events(real)
    out["events_cases"] = []
    for case in EVENT_CASES:
        out["events_cases"].append({"in": case, "out": events.parse_events([case])})
    out["events_errors"] = []
    for bad in ([{"id": "x", "name": 5, "startdate": "1", "enddate": "2"}], ["not a dict"],
                [{"id": "x", "url": ["l"], "startdate": "1", "enddate": "2"}]):
        try:
            events.parse_events(bad)
            out["events_errors"].append({"in": bad, "error": None})
        except Exception as e:  # noqa: BLE001
            out["events_errors"].append({"in": bad, "error": type(e).__name__})


TWITCH_CASES = [
    {"data": [
        {"user_login": "abc", "user_name": "ABC", "title": "Playing Trove", "viewer_count": 12, "game_name": "Trove",
         "started_at": "2026-10-20T10:00:00Z",
         "thumbnail_url": "https://static-cdn.jtvnw.net/previews-ttv/live_user_abc-{width}x{height}.jpg"},
        {"user_name": "OnlyName", "viewer_count": "7", "game_name": None},
        {"title": None, "viewer_count": 3.9, "thumbnail_url": ""},
        {"user_login": "", "user_name": "", "viewer_count": 0},
        {"user_login": "z", "viewer_count": True},
        "not a dict",
        {"user_login": "q", "viewer_count": None, "started_at": None, "game_name": ""},
    ]},
    [{"user_login": "listform", "viewer_count": 1}],
    {"nodata": True},
    None,
    {"data": []},
]

VIDEO_CASES = [
    [{"url": "u1", "title": "t", "channel": "c", "video_id": "v", "published_at": "p", "thumbnail_url": "th"},
     {"url": ""}, {"title": "no url"}, "str", {"url": "u2", "title": None, "channel": None},
     {"url": "u3", "extra": 1, "published_at": None}],
    {"not": "a list"},
    None,
]

HAS_TERM_CASES = [
    ("trove's summer", "trove"), ("introvert", "trove"), ("trove", "trove"), ("trovesaurus", "trove"),
    ("my_trove", "trove"), ("trove_x", "trove"), ("trove2", "trove"), ("2trove", "trove"),
    ("chaos chest!", "chaos chest"), ("chaos  chest", "chaos chest"), ("\u5b9d\u85cf\u4e16\u754ctrove", "trove"),
    ("\u5b9d\u85cf trove", "trove"), ("trove\u0301", "trove"), ("trove\u00b2", "trove"), ("\u00e9trove", "trove"),
    ("c++ rocks", "c++"), ("use c++", "c++"), ("a.b", "."), ("#shorts", "shorts"), ("yt shorts", "shorts"),
    ("aaa", "aa"), ("xaa aa", "aa"), ("trove\u2019s", "trove"), ("trove\u3000game", "trove"),
    ("\u0661trove", "trove"), ("\u2160trove", "trove"), ("trove\u00bd", "trove"), ("\u02b0trove", "trove"),
    ("\u0915\u093etrove", "trove"), ("", "trove"),
]

ISO_DURATION_CASES = [None, "", "PT45S", "PT1M", "PT1M1S", "PT1H2M3S", "PT0S", "P1DT2H", "PT", "PT1H", "pt1m",
                      "PT1M\n", "PT1.5S", "PT60S", "PT61S", "PT\u0661S", " PT1M", "PT1M "]

CSV_CASES = ["", " A, b ,,C ", "x", "  ,  ", "Dup,dup,DUP", "\u00c9cole, \u0130stanbul ", "a\u3000,b"]

BILI_DATE_NOW = "2026-10-21T12:34:56.789012+00:00"
BILI_DATE_CASES = [
    "03-04", "12-25", "10-21", "10-22", "02-29", "2024-02-29", "2023-02-29", "2026-1-5", "13-01", "1-2-3-4",
    "2026--1", "\u6628\u5929", "3\u5c0f\u65f6\u524d", " 15\u5206\u949f\u524d ", "x\u5c0f\u65f6\u524d",
    "\u5c0f\u65f6\u524d", "6\u670820\u65e5", "2\u5929\u524d", "", "   ", "-", "0-0", " 03 - 04 ",
    "100000000000\u5c0f\u65f6\u524d", "17000000\u5c0f\u65f6\u524d", "\u6628\u5929 3\u5c0f\u65f6\u524d",
    "+3\u5206\u949f\u524d", "1_0\u5206\u949f\u524d", "-5\u5206\u949f\u524d", "0001-01-01", "9999-12-31",
    "10000-01-01", "2026-10-21",
]


def run_feeds(out: dict) -> None:
    from app.trove import feeds

    out["twitch_normalize"] = []
    for case in TWITCH_CASES:
        try:
            out["twitch_normalize"].append({"in": case, "out": feeds._normalize_twitch(case)})
        except Exception as e:  # noqa: BLE001
            out["twitch_normalize"].append({"in": case, **err(e)})
    out["videos_normalize"] = [{"in": c, "out": feeds._normalize_videos(c)} for c in VIDEO_CASES]
    out["has_term"] = [[t, term, feeds._has_term(t, term)] for t, term in HAS_TERM_CASES]
    out["iso8601_seconds"] = [[v, feeds._iso8601_seconds(v)] for v in ISO_DURATION_CASES]
    out["csv_set"] = [[c, sorted(feeds._csv_set(c))] for c in CSV_CASES]
    now = datetime.fromisoformat(BILI_DATE_NOW)
    rows = []
    for s in BILI_DATE_CASES:
        try:
            rows.append([s, feeds._parse_bilibili_date(s, now).isoformat()])
        except Exception as e:  # noqa: BLE001
            rows.append([s, "error:" + type(e).__name__])
    out["bilibili_dates"] = {"now": BILI_DATE_NOW, "cases": rows}
    out["lower"] = [[s, s.lower()] for s in ["ABC", "\u0130", "\u03a3\u0391\u03a3", "\u1e9e", "\u2160", "\u00c9"]]


class FakeResponse:
    def __init__(self, url, payload=None, text=None, status=200):
        self.url = url
        self._payload = payload
        self.text = text
        self.status_code = status

    def json(self):
        return self._payload

    def raise_for_status(self):
        if self.status_code >= 400:
            import httpx

            raise httpx.HTTPStatusError("fake", request=None, response=None)


class FakeClient:
    routes: dict = {}
    calls: list = []

    def __init__(self, *args, **kwargs):
        FakeClient.calls.append({"client_kwargs": {k: v for k, v in kwargs.items() if k != "timeout"}})

    async def __aenter__(self):
        return self

    async def __aexit__(self, *exc):
        return False

    async def get(self, url, params=None, headers=None):
        FakeClient.calls.append({"url": str(url), "params": params, "headers": headers})
        return FakeResponse(url, **FakeClient.routes[str(url)])

    async def post(self, url, params=None):
        FakeClient.calls.append({"post": str(url), "params": params})
        return FakeResponse(url, **FakeClient.routes[str(url)])


def yt_item(vid, title, channel, published, desc="", live="none", thumbs=True):
    snip = {"publishedAt": published, "channelTitle": channel, "title": title, "description": desc,
            "liveBroadcastContent": live}
    if thumbs:
        snip["thumbnails"] = {"default": {"url": f"https://i.ytimg.com/vi/{vid}/default.jpg"},
                              "high": {"url": f"https://i.ytimg.com/vi/{vid}/hqdefault.jpg"}}
    return {"kind": "youtube#searchResult", "id": {"kind": "youtube#video", "videoId": vid}, "snippet": snip}


def yt_detail(vid, *, title=None, desc="", tags=None, cat="20", dur="PT10M", channel=None, published=None,
              thumbs=None):
    snip = {"description": desc, "categoryId": cat}
    if title is not None:
        snip["title"] = title
    if tags is not None:
        snip["tags"] = tags
    if channel is not None:
        snip["channelTitle"] = channel
    if published is not None:
        snip["publishedAt"] = published
    if thumbs is not None:
        snip["thumbnails"] = thumbs
    out = {"id": vid, "snippet": snip}
    if dur is not None:
        out["contentDetails"] = {"duration": dur}
    return out


def youtube_scenarios():
    search = {"items": [
        yt_item("v1", "Trove Delve guide", "ChanA", "2026-10-01T10:00:00Z"),
        yt_item("v2", "Trove builds", "ChanA", "2026-09-01T10:00:00Z"),
        yt_item("v3", "Trove farming", "ChanA", "2026-10-05T10:00:00Z"),
        yt_item("v4", "Trove stuff", "SCYUSHI", "2026-10-02T10:00:00Z"),
        yt_item("v5", "Trove live now", "ChanB", "2026-10-03T10:00:00Z", live="live"),
        yt_item("v6", "Trove clip", "ChanB", "2026-10-04T10:00:00Z"),
        yt_item("v7", "Trove fun", "ChanB", "2026-10-04T11:00:00Z", desc="#shorts"),
        yt_item("v8", "Trinket Trove episode", "ChanC", "2026-10-06T10:00:00Z"),
        yt_item("v9", "Introvert life", "ChanC", "2026-10-07T10:00:00Z"),
        yt_item("v10", "Trove's Summer", "ChanD", "2026-10-08T10:00:00Z"),
        yt_item("v11", "My Trove night", "ChanD", "2026-10-08T10:00:00Z"),
        {"id": {"kind": "youtube#channel", "channelId": "UCx"}, "snippet": {"title": "channel"}},
        yt_item("v12", "trove without details", "ChanE", "2026-10-09T10:00:00Z"),
        yt_item("v13", "Trove marathon", "ChanE", "2026-10-09T10:00:00Z"),
        yt_item("v1", "Trove Delve guide (dup)", "ChanA", "2026-10-01T10:00:00Z"),
        yt_item("v14", "Trove no thumbs", "ChanF", "2026-10-10T10:00:00Z", thumbs=False),
        {"id": {"videoId": "v15"}, "snippet": {"title": "Trove no date", "channelTitle": "ChanF"}},
        yt_item("v16", "\u5b9d\u85cf\u4e16\u754ctrove", "ChanG", "2026-10-11T10:00:00Z"),
        yt_item("v17", "TROVE UPPER", "ChanG", "2026-10-12T10:00:00Z"),
        yt_item("v18", "trove_underscore", "ChanG", "2026-10-13T10:00:00Z"),
        yt_item("v19", "Trove same time", "ChanH", "2026-10-08T10:00:00Z"),
    ]}
    videos = {"items": [
        yt_detail("v1", title="Trove Delve guide FULL", desc="delve run"),
        yt_detail("v2", desc="builds", cat="22", tags=["geode"]),
        yt_detail("v3", desc="farm"),
        yt_detail("v4"),
        yt_detail("v5"),
        yt_detail("v6", dur="PT45S"),
        yt_detail("v7"),
        yt_detail("v8"),
        yt_detail("v9"),
        yt_detail("v10", cat="22"),
        yt_detail("v11", cat="22", tags=["Chaos Chest", "x"]),
        yt_detail("v13", dur="P1DT2H", channel="ChanE renamed"),
        yt_detail("v14"),
        yt_detail("v15", published="2026-10-14T00:00:00Z",
                  thumbs={"high": {"url": "https://i.ytimg.com/vi/v15/hq.jpg"}}),
        yt_detail("v16"),
        yt_detail("v17", dur="PT1M"),
        yt_detail("v18"),
        yt_detail("v19", dur=None),
    ]}
    base = {
        "feeds_youtube_query": "Trove game",
        "feeds_youtube_excluded_channels": "scyushi,mikailstream",
        "feeds_youtube_excluded_title_terms": "trinket trove",
        "feeds_youtube_require_terms": "trove",
        "feeds_youtube_relevance_terms": "chaos chest,delve,geode",
        "feeds_youtube_video_category_id": "20",
        "feeds_video_cutoff_days": 90,
        "feeds_per_channel_max": 2,
        "feeds_max_items": 8,
    }
    open_cfg = {**base, "feeds_youtube_relevance_terms": "", "feeds_youtube_video_category_id": "",
                "feeds_youtube_require_terms": "", "feeds_youtube_excluded_title_terms": "",
                "feeds_per_channel_max": 3, "feeds_max_items": 50}
    return [
        {"name": "curated", "settings": base, "search": search, "videos": videos},
        {"name": "open", "settings": open_cfg, "search": search, "videos": videos},
        {"name": "empty_search", "settings": base, "search": {"items": []}, "videos": videos},
        {"name": "no_items_key", "settings": base, "search": {}, "videos": videos},
    ]


def bilibili_synthetic() -> str:
    def card(href, title, author, date, srcset, extra=""):
        parts = ['<div class="bili-video-card">']
        if href is not None:
            parts.append(f'<a href="{href}">')
        if srcset is not None:
            parts.append(f'<picture><source srcset="{srcset}" type="image/webp"></picture>')
        if href is not None:
            parts.append("</a>")
        if title is not None:
            parts.append(f'<h3 class="bili-video-card__info--tit" title="{title}">{title}</h3>')
        if author is not None:
            parts.append(f'<span class="bili-video-card__info--author">{author}</span>')
        if date is not None:
            parts.append(f'<span class="bili-video-card__info--date">{date}</span>')
        parts.append(extra + "</div>")
        return "".join(parts)

    cards_html = [
        card("//www.bilibili.com/video/BV1aa/", "A1", "Alice", " · 10-20", "//i0.hdslb.com/bfs/a1.jpg@672w.webp"),
        card("//www.bilibili.com/video/BV1ab", "A2", "Alice", " · 2026-09-01", "//i0.hdslb.com/bfs/a2.jpg"),
        card("//www.bilibili.com/video/BV1ac//", "A3", "Alice", " · \u6628\u5929", "https://i0.hdslb.com/a3.jpg@x"),
        card("//www.bilibili.com/video/BV1ad/", "A4", "Alice", "3\u5c0f\u65f6\u524d", "//i0.hdslb.com/a4.jpg"),
        card("//www.bilibili.com/video/BV1ba/", "B1 &amp; co", " Bob ", " · 2020-01-01", "//i0.hdslb.com/b1.jpg"),
        card("//www.bilibili.com/video/BV1bb/", None, None, None, None),
        card("https://www.bilibili.com/video/BV1x/", "abs", "C", "10-01", ""),
        card(None, "no link", "C", "10-01", ""),
        card("//www.bilibili.com/zh-Hans/video/BV1y/", "zh", "C", "10-01", ""),
        card("//www.bilibili.com/video/BV1bc/", "B2", "Bob", "12-31", "//i0.hdslb.com/b2.jpg"),
        card("//www.bilibili.com/video/BV1bd/", "B3", "Bob", "15\u5206\u949f\u524d", "//i0.hdslb.com/b3.jpg"),
        card("//www.bilibili.com/video/BV1be/", "B4", "Bob", "6\u670820\u65e5", "//i0.hdslb.com/b4.jpg"),
        card("//www.bilibili.com/video/BV1ca/", "C1", "Carol", "02-29", "//i0.hdslb.com/c1.jpg"),
        card("//www.bilibili.com/video/BV1cb/", "C2", "Carol", "10-21", "//i0.hdslb.com/c2.jpg"),
        card("//www.bilibili.com/video/BV1cc/", "C3", "Carol", "10-22", "//i0.hdslb.com/c3.jpg"),
    ]
    return "<html><body>" + "".join(cards_html) + "</body></html>"


def run_fetchers(out: dict) -> None:
    from app.trove import feeds

    now = datetime(2026, 10, 21, 12, 34, 56, 789012, tzinfo=UTC)
    orig_client = feeds.httpx.AsyncClient
    orig_get = feeds.runtime_config.get_setting
    orig_now = feeds.utcnow
    orig_key = feeds.settings.yt_api_key
    try:
        feeds.httpx.AsyncClient = FakeClient
        feeds.utcnow = lambda: now
        feeds.settings.yt_api_key = "KEY"
        out["youtube"] = []
        for sc in youtube_scenarios():
            settings = sc["settings"]

            async def get_setting(key, _s=settings):
                return _s[key]

            feeds.runtime_config.get_setting = get_setting
            FakeClient.routes = {
                feeds._YOUTUBE_SEARCH_URL: {"payload": sc["search"]},
                feeds._YOUTUBE_VIDEOS_URL: {"payload": sc["videos"]},
            }
            FakeClient.calls = []
            try:
                result = asyncio.run(feeds._fetch_youtube())
                out["youtube"].append({**sc, "now": now.isoformat(), "calls": FakeClient.calls, "out": result})
            except Exception as e:  # noqa: BLE001
                out["youtube"].append({**sc, "now": now.isoformat(), "calls": FakeClient.calls, **err(e)})

        page = (RAW / "bilibili_search.html").read_text(encoding="utf-8")
        bili_settings = {"feeds_bilibili_keyword": "\u5b9d\u85cf\u4e16\u754ctrove", "feeds_video_cutoff_days": 90,
                         "feeds_per_channel_max": 3, "feeds_max_items": 10}

        async def bili_get(key):
            return bili_settings[key]

        feeds.runtime_config.get_setting = bili_get
        out["bilibili"] = []
        for name, html, cfg in (
            ("raw", page, {}),
            ("rewritten", page.replace(*BILI_REWRITE), {}),
            ("rewritten_wide", page.replace(*BILI_REWRITE), {"feeds_per_channel_max": 20, "feeds_max_items": 50}),
            ("synthetic", bilibili_synthetic(), {}),
            ("synthetic_short_cutoff", bilibili_synthetic(), {"feeds_video_cutoff_days": 1, "feeds_max_items": 3}),
        ):
            bili_settings.update({"feeds_video_cutoff_days": 90, "feeds_per_channel_max": 3, "feeds_max_items": 10})
            bili_settings.update(cfg)
            FakeClient.routes = {feeds._BILIBILI_SEARCH_URL: {"text": html}}
            FakeClient.calls = []
            result = asyncio.run(feeds._fetch_bilibili())
            entry = {"name": name, "settings": dict(bili_settings), "now": now.isoformat(), "out": result}
            if name.startswith("synthetic"):
                entry["html"] = html
            entry["calls"] = FakeClient.calls
            out["bilibili"].append(entry)
    finally:
        feeds.httpx.AsyncClient = orig_client
        feeds.runtime_config.get_setting = orig_get
        feeds.utcnow = orig_now
        feeds.settings.yt_api_key = orig_key


HDSLB_URLS = [
    "https://i0.hdslb.com/bfs/archive/abc.jpg",
    "https://i0.hdslb.com/bfs/archive/abc.jpg@672w_378h_1c_!web-search-common-cover.webp",
    "HTTPS://I0.HDSLB.COM/Path/A.JPG?Q=1#Frag",
    "https://hdslb.com/x.jpg",
    "https://evilhdslb.com/x.jpg",
    "https://hdslb.com.evil.com/x.jpg",
    "http://i0.hdslb.com/x.jpg",
    "//i0.hdslb.com/x.jpg",
    "i0.hdslb.com/x.jpg",
    "",
    "https://",
    "https:///x",
    "https://i0.hdslb.com",
    "https://i0.hdslb.com:443/x.jpg",
    "https://i0.hdslb.com:8443/x.jpg",
    "https://i0.hdslb.com:/x.jpg",
    "https://i0.hdslb.com:abc/x.jpg",
    "https://i0.hdslb.com: 443/x.jpg",
    "https://i0.hdslb.com:4_43/x.jpg",
    "https://user:pass@i0.hdslb.com/x.jpg",
    "https://evil.com@i0.hdslb.com/x.jpg",
    "https://i0.hdslb.com@evil.com/x.jpg",
    "https://i0.hdslb.com/a b/c d.jpg?q=a b&r=\"x\"#f g",
    "https://i0.hdslb.com/%zz/%41%e4.jpg",
    "https://i0.hdslb.com/\u4e2d\u6587.jpg?\u4e2d=\u6587",
    "https://i0.hdslb.com/a/./b/../c.jpg",
    "https://i0.hdslb.com/../../etc/passwd",
    "https://i0.hdslb.com/x\x00.jpg",
    "https://i0.hdslb.com/x\x7f.jpg",
    "https://i0.hdslb.com/x\t.jpg",
    "https://i0.hdslb.com/x{}|\\^`[].jpg",
    "https://I0.HDSLB.COM/x.jpg",
    "https://i0.hdslb.com./x.jpg",
    "https://127.0.0.1/x.jpg",
    "https://01.2.3.4/x.jpg",
    "https://256.1.1.1/x.jpg",
    "https://[::1]/x.jpg",
    "https://[::1/x.jpg",
    "https://[zz]/x.jpg",
    "https://i0 .hdslb.com/x.jpg",
    "https://i0%2ehdslb.com/x.jpg",
    "https://i0.hdslb.com\\@evil.com/",
    "ftp://i0.hdslb.com/x.jpg",
    "javascript:alert(1)",
    "https:i0.hdslb.com/x.jpg",
    ":https://i0.hdslb.com/x",
    "https://i0.hdslb.com/x?",
    "https://i0.hdslb.com/x#",
    "https://i0.hdslb.com?x=1",
    "https://xn--i0-zka.hdslb.com/x.jpg",
    "https://\u00e4.hdslb.com/x.jpg",
    "https://\u00c4.HDSLB.com/x.jpg",
    "https://i0.hdslb.com/" + "a" * 70000,
]


def run_httpx(out: dict) -> None:
    import httpx

    from app.trove import feeds

    rows = []
    for u in HDSLB_URLS:
        try:
            t = feeds._hdslb_target(u)
            rows.append({"in": u, "ok": str(t), "raw_path": t.raw_path.decode("ascii"), "host": t.host,
                         "port": t.port, "userinfo": t.userinfo.decode("ascii")})
        except Exception as e:  # noqa: BLE001
            rows.append({"in": u, **err(e)})
    out["hdslb_target"] = rows

    status_rows = []
    for status, reason, location, url in (
        (404, b"Not Found", None, "https://i0.hdslb.com/x.jpg"),
        (404, b"Nope", None, "https://i0.hdslb.com/x.jpg?a=1#f"),
        (403, b"Forbidden", None, "https://i0.hdslb.com/x.jpg"),
        (500, b"Internal Server Error", None, "https://i0.hdslb.com/x.jpg"),
        (503, b"", None, "https://i0.hdslb.com/x.jpg"),
        (301, b"Moved Permanently", "https://evil.example/", "https://i0.hdslb.com/x.jpg"),
        (302, b"Found", None, "https://i0.hdslb.com/x.jpg"),
        (304, b"Not Modified", "/y", "https://i0.hdslb.com/x.jpg"),
        (307, b"Temporary Redirect", "/y", "https://i0.hdslb.com/x.jpg"),
        (308, b"Permanent Redirect", "/y", "https://i0.hdslb.com/x.jpg"),
        (303, b"See Other", "/y", "https://i0.hdslb.com/x.jpg"),
        (300, b"Multiple Choices", "/y", "https://i0.hdslb.com/x.jpg"),
        (418, b"I'm a teapot", None, "https://i0.hdslb.com/x.jpg"),
        (101, b"Switching Protocols", None, "https://i0.hdslb.com/x.jpg"),
        (199, b"", None, "https://i0.hdslb.com/x.jpg"),
        (600, b"Weird", None, "https://i0.hdslb.com/x.jpg"),
        (200, b"OK", None, "https://i0.hdslb.com/x.jpg"),
        (204, b"No Content", None, "https://i0.hdslb.com/x.jpg"),
        (299, b"", None, "https://i0.hdslb.com/x.jpg"),
    ):
        headers = {"Location": location} if location else {}
        resp = httpx.Response(status, headers=headers, request=httpx.Request("GET", url),
                              extensions={"reason_phrase": reason})
        try:
            resp.raise_for_status()
            status_rows.append({"status": status, "reason": reason.decode(), "location": location, "url": url,
                                "message": None})
        except httpx.HTTPStatusError as e:
            status_rows.append({"status": status, "reason": reason.decode(), "location": location, "url": url,
                                "message": str(e)})
    out["status_errors"] = status_rows


def news_doc(i, title, url, cats, published, **kw):
    return SimpleNamespace(title=title, url=url, author=kw.get("author", "Team Trove"),
                           summary=kw.get("summary", f"s{i}"), category=cats[0] if cats else "News",
                           categories=list(cats), image=kw.get("image"), published_at=published)


def run_routes(out: dict) -> None:
    from pydantic import TypeAdapter

    from app.trove import feeds, news
    from app.trove import router as trove_router
    from app.trove.schemas import TroveEventList, TroveNewsHistory, TroveNewsList, TwitchStreams, Videos

    t0 = datetime(2026, 10, 20, 15, 37, 59, 123000, tzinfo=UTC)
    docs = [
        news_doc(1, "Shadow's Eve Contest", "https://t/1", ["Community", "Events"], t0),
        news_doc(2, "shadow's eve contest ", "https://t/2", ["Events", "Shop Offers"], t0 - timedelta(hours=1)),
        news_doc(3, "  SHADOW'S EVE CONTEST", "https://t/3", ["News"], t0 - timedelta(hours=2), image="https://img"),
        news_doc(4, "", "https://t/4", [], t0 - timedelta(hours=3), author="Someone"),
        news_doc(5, "", "https://t/4", ["Dup URL"], t0 - timedelta(hours=4)),
        news_doc(6, "Patch Notes \u00c9t\u00e9", "https://t/6", ["Patch"], datetime(2026, 10, 1, tzinfo=UTC)),
        news_doc(7, "PATCH NOTES \u00c9T\u00c9", "https://t/7", ["Patch", "News"], None),
        news_doc(8, "Other", "https://t/8", ["A"], datetime(2026, 9, 1, 0, 0, 0, 1000, tzinfo=UTC)),
        news_doc(9, "Third", "https://t/9", ["B"], datetime(2026, 8, 1, tzinfo=UTC)),
    ]

    class FakeFind:
        def __init__(self, rows):
            self.rows = rows

        def sort(self, key):
            assert key == "-published_at"
            return self

        def limit(self, n):
            self.n = n
            return self

        async def to_list(self):
            import copy

            return copy.deepcopy(self.rows[: self.n])

    orig_find = news.TroveNews.find
    rows = []
    try:
        for limit in (1, 2, 3, 4, 20):
            news.TroveNews.find = lambda *a, **k: FakeFind(docs)
            result = asyncio.run(trove_router.list_news(ctx=None, limit=limit))
            body = TypeAdapter(TroveNewsList).dump_json(result).decode()
            rows.append({"limit": limit, "body": body})
    finally:
        news.TroveNews.find = orig_find
    out["news_docs"] = [{**vars(d), "published_at": iso(d.published_at)} for d in docs]
    out["news_latest"] = rows

    hist = TroveNewsHistory(items=[trove_router._news_item(d) for d in docs[:3]], count=3, total=42)
    out["news_history"] = {"docs": 3, "total": 42, "body": TypeAdapter(TroveNewsHistory).dump_json(hist).decode()}

    now = 1_800_000_000
    evs = [
        SimpleNamespace(event_id="1", name="Up", url="https://u", category="Giveaway", image=None, icon="i",
                        lookup="giveaway=1", starts_at=now + 10, ends_at=now + 100),
        SimpleNamespace(event_id="2", name="On", url="", category="Event", image="im", icon=None, lookup=None,
                        starts_at=now - 10, ends_at=now + 5),
        SimpleNamespace(event_id="3", name="Ended", url="x", category="Event", image=None, icon=None, lookup=None,
                        starts_at=now - 100, ends_at=now),
        SimpleNamespace(event_id="4", name="Edge start", url="x", category="E", image=None, icon=None, lookup=None,
                        starts_at=now, ends_at=now),
    ]
    lst = TroveEventList(items=[trove_router._event_item(e, now) for e in evs], count=len(evs))
    out["events_items"] = {"now": now, "events": [vars(e) for e in evs],
                           "body": TypeAdapter(TroveEventList).dump_json(lst).decode()}

    fetched = datetime(2026, 10, 21, 1, 2, 3, 456000, tzinfo=UTC)
    tw_items = feeds._normalize_twitch(TWITCH_CASES[0])
    tw = TwitchStreams(items=[trove_router.TwitchStream(**i) for i in tw_items], count=len(tw_items),
                       fetched_at=fetched)
    vids = [{"title": "t", "url": "u", "channel": "c", "video_id": "v", "published_at": "2026-10-21T12:34:56.789012Z",
             "thumbnail_url": None}, {"title": "", "url": "u2", "channel": "", "video_id": None,
                                      "published_at": None, "thumbnail_url": "x"}]
    vv = Videos(items=[trove_router.Video(**i) for i in vids], count=2, fetched_at=None)
    out["feed_bodies"] = {
        "fetched_at": fetched.isoformat(),
        "twitch_items": tw_items,
        "twitch_body": TypeAdapter(TwitchStreams).dump_json(tw).decode(),
        "video_items": vids,
        "videos_body": TypeAdapter(Videos).dump_json(vv).decode(),
        "empty_body": TypeAdapter(Videos).dump_json(Videos(items=[], count=0, fetched_at=None)).decode(),
    }


def run_golden() -> dict:
    sys.path.insert(0, str(ROOT))
    os.chdir(ROOT)
    write_entities()
    out: dict = {"python": sys.version.split()[0]}
    run_news(out)
    run_events(out)
    run_feeds(out)
    run_fetchers(out)
    run_httpx(out)
    run_routes(out)
    return out


def main() -> None:
    mode = sys.argv[1] if len(sys.argv) > 1 else ""
    if mode == "golden":
        data, name = run_golden(), "golden.json"
    elif mode == "html":
        data, name = run_html(), "html.json"
    else:
        sys.exit("usage: gen_feeds_fixtures.py golden|html")
    (DATA / name).write_text(json.dumps(data, ensure_ascii=False, indent=1) + "\n", encoding="utf-8", newline="\n")
    print(f"wrote {DATA / name}")


if __name__ == "__main__":
    main()

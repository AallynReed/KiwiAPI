"""Extra golden cases for the news loop port, from the real Python code.

    TZ=UTC0 S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_news_fixtures.py
        -> native/kiwi-server/testdata/feeds/news_cases.json

TZ=UTC0 matters: a naive pubDate (-0000, unknown zone) goes through
.astimezone(), which reads it in the host zone, and the container runs UTC.

The saved feed (testdata/feeds/raw/news_feed.xml) and the first RFC 2822
cases are in golden.json (gen_feeds_fixtures.py); these add ElementTree
corner cases and more dates.
"""

from __future__ import annotations

import json
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "feeds" / "news_cases.json"
UTC = timezone.utc

NS = 'xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:media="http://search.yahoo.com/mrss/"'

FEEDS = [
    f"<rss {NS}><channel><item><link> https://a/1 </link><title> A &amp;amp; B </title>"
    "<dc:creator> Ann </dc:creator><description>&lt;p&gt;Hi &amp;amp; bye&lt;/p&gt;</description>"
    "<category> One </category><category>  </category><category>Two &amp;amp; Three</category>"
    "<pubDate>Tue, 20 Oct 2026 15:37:59 -0000</pubDate>"
    '<media:content url=""/><media:thumbnail url="https://img/t.png"/></item></channel></rss>',
    f"<rss {NS}><channel><item><title>no link</title></item><item><link></link></item>"
    "<item><link>x<!-- c -->y<![CDATA[<z>]]><b>inner</b>tail</link><pubDate>garbage</pubDate></item>"
    "</channel><channel><item><link>second-channel</link><media:content url='u1'/>"
    "<media:content url='u2'/></item></channel></rss>",
    f"<rss {NS}><channel><item><link>l</link><description><![CDATA[<p>Big "
    + ("word " * 80)
    + "</p>]]></description><pubDate>1 Jan 0001 00:30:00 +0100</pubDate></item></channel></rss>",
    "<rss><channel>" + "".join(f"<item><link>u{i}</link></item>" for i in range(35)) + "</channel></rss>",
    '<rss xmlns="urn:x"><channel><item><link>ns-default</link></item></channel></rss>',
    "<!DOCTYPE rss [<!ENTITY t \"Trove\">]><rss><channel><item><link>e</link><title>&t;!</title>"
    "<pubDate>31 Dec 9999 23:00:00 -0200</pubDate></item></channel></rss>",
    "<rss><channel><item><link>bad</link><title>&nbsp;</title></item></channel></rss>",
    "<rss><channel><item><link>a\r\nb</link><dc:creator xmlns:dc='urn:other'>x</dc:creator></item></channel></rss>",
    "not xml at all",
]

DATES = [
    "Tue, 20 Oct 2026 15:37:59 -0000",
    "Tue, 20 Oct -5 15:37:59 +0000",
    "Tue, 20 Oct 68 15:37:59 +0000",
    "Tue, 20 Oct 69 15:37:59 +0000",
    "Tue, 20 Oct 2026 15:37:59 +2400",
    "Tue, 20 Oct 2026 15:37:59 +2359",
    "Tue, 20 Oct 2026 15:37:59 +0099",
    "Tue, 20 Oct 2026 15:37:59 -9999999999999999999999",
    "Tue, 20 Oct 2026 15:37:59 +",
    "Tue, 20 Oct 2026 15:37:59+0100",
    "Tue, 20 Oct 2026 15:37:59-0100",
    "2026 Oct 20 15:37:59 +0000",
    "20 Oct 15:37:59 2026 +0000",
    "Tuesday, 20 Oct 2026 15:37:59 +0000",
    "mon 20 Oct 2026 15:37:59 +0000",
    "20 Oct, 2026, 15:37:59, +0000",
    "20 Oct 2026, 15:37:59 +0000",
    "20-Oct-2026 15:37:59 +0000",
    "20 Oct 2026 15:37:59 +0000 extra words",
    "29 Feb 2024 00:00:00 +0000",
    "29 Feb 2023 00:00:00 +0000",
    "1 Jan 0 00:00:00 +0000",
    "1 Jan 10000 00:00:00 +0000",
    "\u0662\u0660 Oct 2026 15:37:59 +0000",
    "20 OCT 2026 15:37:59 gmt",
    "20 Oct 2026 15:37:59 z",
    "20 Oct 2026 15:37 +0000",
    "20 Oct 2026 15 +0000",
    "20 Oct 2026 15:37:59:01 +0000",
    "20 Oct 2026 15.37 +0000",
    "20 Oct 2026 15.37.59.1 +0000",
    "   ",
    "20\u3000Oct 2026 15:37:59 +0000",
    "20 Oct 2026 -1:37:59 +0000",
]


def main() -> None:
    from email.utils import parsedate_to_datetime

    from app.trove import news

    probe = datetime(2026, 1, 1)
    assert probe.astimezone(UTC).replace(tzinfo=None) == probe, "run with TZ=UTC0 (the container's zone)"
    feeds = []
    for xml in FEEDS:
        try:
            items = news.parse_feed(xml)
            feeds.append({"xml": xml, "items": [
                {**it, "published_at": None if it["published_at"] is None else int(it["published_at"].timestamp())}
                for it in items
            ]})
        except Exception as e:  # noqa: BLE001
            feeds.append({"xml": xml, "error": type(e).__name__})
    dates = []
    for s in DATES:
        try:
            d = parsedate_to_datetime(s.strip())
            if d.tzinfo is None:
                dates.append([s, {"naive": d.isoformat()}])
            else:
                dates.append([s, {"ok": int(d.astimezone(UTC).timestamp())}])
        except (TypeError, ValueError) as e:
            dates.append([s, {"error": type(e).__name__}])
        except Exception as e:  # noqa: BLE001
            dates.append([s, {"raised": type(e).__name__}])
    out = {"python": sys.version.split()[0], "feeds": feeds, "dates": dates}
    OUT.write_text(json.dumps(out, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()

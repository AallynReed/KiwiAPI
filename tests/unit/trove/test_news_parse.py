from datetime import timezone

from app.trove.news import parse_feed

_FEED = """<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"
     xmlns:dc="http://purl.org/dc/elements/1.1/"
     xmlns:media="http://search.yahoo.com/mrss/">
  <channel>
    <title>Trove</title>
    <item>
      <title>Patch Notes &amp; More</title>
      <link>https://trovegame.com/news/patch-1</link>
      <dc:creator>Team Trove</dc:creator>
      <pubDate>Mon, 01 Jan 2024 12:00:00 +0000</pubDate>
      <description>&lt;p&gt;Lots of &lt;b&gt;changes&lt;/b&gt; today.&lt;/p&gt;</description>
      <category>Update</category>
      <category>Patch</category>
      <media:content url="https://img.example/x.png"/>
    </item>
    <item>
      <title>No Link Item</title>
      <link></link>
    </item>
  </channel>
</rss>"""


def test_parse_feed_extracts_item():
    items = parse_feed(_FEED)
    assert len(items) == 1  # the link-less item is skipped
    it = items[0]
    assert it["title"] == "Patch Notes & More"
    assert it["url"] == "https://trovegame.com/news/patch-1"
    assert it["author"] == "Team Trove"
    assert it["summary"] == "Lots of changes today."  # html stripped
    assert it["category"] == "Update" and it["categories"] == ["Update", "Patch"]
    assert it["image"] == "https://img.example/x.png"
    assert it["published_at"].tzinfo == timezone.utc
    assert it["published_at"].year == 2024


def test_parse_feed_empty_channel():
    assert parse_feed("<rss><channel></channel></rss>") == []

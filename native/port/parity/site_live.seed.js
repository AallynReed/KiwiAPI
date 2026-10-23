// site_live twin seed: public clubs, BTT releases + changelog, calendar events,
// news and the video feeds (copied to B by mongodump).
const NOW = Date.now(), DAY = 86400000;
const at = (d) => new Date(NOW + d * DAY);
const secs = (d) => NumberInt(Math.floor((NOW + d * DAY) / 1000));
const oid = (s) => ObjectId(s);

db.clubs.insertMany([
  {_id: oid("65f100000000000000000001"), guild_id: NumberLong(1), name: "  alpha ", public: true,
   description: "First", banner_url: null, avatar_url: "https://x/a.png", discord_url: null,
   website_url: "https://alpha.example", role_links: {}, updated_by: null, created_at: at(-9), updated_at: at(-9)},
  {_id: oid("65f100000000000000000002"), guild_id: NumberLong(1), name: "Delta", public: true,
   description: null, banner_url: "https://x/d.png", avatar_url: null, discord_url: "https://discord.gg/d",
   website_url: null, role_links: {}, updated_by: null, created_at: at(-8), updated_at: at(-8)},
  {_id: oid("65f100000000000000000003"), guild_id: NumberLong(2), name: "zulu Ünï", public: true,
   description: "Unranked", banner_url: null, avatar_url: null, discord_url: null, website_url: null,
   role_links: {}, updated_by: null, created_at: at(-7), updated_at: at(-7)},
  {_id: oid("65f100000000000000000004"), guild_id: NumberLong(2), name: "Hidden", public: false,
   description: null, banner_url: null, avatar_url: null, discord_url: null, website_url: null,
   role_links: {}, updated_by: null, created_at: at(-6), updated_at: at(-6)},
  {_id: oid("65f100000000000000000005"), guild_id: NumberLong(3), name: "Bravo", public: true,
   description: null, banner_url: null, avatar_url: null, discord_url: null, website_url: null,
   role_links: {}, updated_by: null, created_at: at(-5), updated_at: at(-5)},
]);

const asset = (name, url, size, ct, dl) => ({name, url, size: NumberInt(size), content_type: ct, download_count: NumberInt(dl)});
db.btt_releases.insertMany([
  {_id: oid("65f200000000000000000001"), release_id: NumberLong(9001), tag_name: "v2.0.0-beta.1", name: "Beta Ünï",
   body: "notes", html_url: "https://github.com/x/r/9001", prerelease: true, published_at: at(-2),
   assets: [asset("BTT-2.0.0.msi", "https://dl/1", 10, "application/x-msi", 3)], fetched_at: at(-1)},
  {_id: oid("65f200000000000000000002"), release_id: NumberLong(9000), tag_name: "v1.9.0", name: "",
   body: "", html_url: "https://github.com/x/r/9000", prerelease: false, published_at: at(-10),
   assets: [asset("btt.AppImage", "https://dl/2", 20, null, 0), asset("btt_1.9.0_amd64.deb", "https://dl/3", 21, "x", 7),
            {name: "notes.txt", url: "https://dl/4", size: NumberInt(1), content_type: "text/plain", download_count: NumberInt(1), extra: 1.5}],
   fetched_at: at(-1)},
  {_id: oid("65f200000000000000000003"), release_id: NumberLong(8999), tag_name: "v1.8.0", name: "Setup",
   body: "x", html_url: "https://github.com/x/r/8999", prerelease: false, published_at: at(-40),
   assets: [asset("BTT-Setup-1.8.0.exe", "https://dl/5", 30, "a", 9), asset("btt-1.8.0.apk", "https://dl/6", 31, "b", 2)],
   fetched_at: at(-1)},
]);
db.btt_changelogs.insertOne({
  _id: oid("65f300000000000000000001"), repo: "AallynReed/BetterTroveTools", rate_limited: false, fetched_at: at(-1),
  groups: [
    {version: "Unreleased", commits: [{sha: "abc1234def", short_sha: "abc1234", message: "feat(ui): x", type: "feat", url: "https://c/1"}]},
    {version: "v1.9.0", commits: [{sha: "def5678abc", short_sha: "def5678", message: "fix: y", type: "fix", url: ""}]},
  ],
});

db.trove_events.insertMany([
  {_id: oid("65f400000000000000000001"), event_id: "e-on", name: "Ongoing Event", url: "https://t/e1", category: "Event",
   image: null, icon: "i.png", lookup: "l", starts_at: secs(-1), ends_at: secs(2), first_seen_at: at(-3), updated_at: at(-1)},
  {_id: oid("65f400000000000000000002"), event_id: "e-up", name: "Upcoming Sale", url: "", category: "Sale",
   image: "img.png", icon: null, lookup: null, starts_at: secs(3), ends_at: secs(5), first_seen_at: at(-3), updated_at: at(-1)},
  {_id: oid("65f400000000000000000003"), event_id: "e-past", name: "Past", url: "", category: "Event",
   image: null, icon: null, lookup: null, starts_at: secs(-30), ends_at: secs(-29), first_seen_at: at(-40), updated_at: at(-29)},
]);

db.trove_news.insertMany([
  {_id: oid("65f500000000000000000001"), url: "https://trovegame.com/n/1", title: "Patch Notes", author: "Team Trove",
   summary: "s", category: "News", categories: ["News"], image: null, published_at: at(-1), first_seen_at: at(-1), updated_at: at(-1)},
  {_id: oid("65f500000000000000000002"), url: "https://trovegame.com/n/2", title: "patch notes ", author: "Team Trove",
   summary: "", category: "Patch", categories: ["Patch"], image: null, published_at: at(-1.5), first_seen_at: at(-1), updated_at: at(-1)},
  {_id: oid("65f500000000000000000003"), url: "https://trovegame.com/n/3", title: "Sälé", author: "A",
   summary: "", category: "Shop", categories: [], image: "https://i/3.png", published_at: at(-2), first_seen_at: at(-2), updated_at: at(-2)},
]);

db.feed_cache.insertMany([
  {_id: oid("65f600000000000000000001"), feed: "youtube", fetched_at: at(-0.1), items: [
    {title: "Vid", url: "https://y/1", channel: "C", video_id: "1", published_at: "2026-10-20", thumbnail_url: null, views: NumberInt(12), ratio: 0.5},
    {title: "Ünï", url: "https://y/2", channel: "C2", nested: {a: [NumberInt(1), 2.25, null, true]}},
  ]},
  {_id: oid("65f600000000000000000002"), feed: "twitch", fetched_at: at(-0.2), items: []},
]);

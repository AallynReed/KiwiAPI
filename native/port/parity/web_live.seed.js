// web_live twin seed: what the live pages' first paints read - BTT releases, calendar
// events, news, the video feeds, mod stats and giveaways (copied to B by mongodump).
const NOW = Date.now(), DAY = 86400000;
const at = (d) => new Date(NOW + d * DAY);
const secs = (d) => NumberInt(Math.floor((NOW + d * DAY) / 1000));
const oid = (s) => ObjectId(s);

const asset = (name, url, size) => ({name, url, size: NumberInt(size), content_type: null, download_count: NumberInt(1)});
db.btt_releases.insertMany([
  {_id: oid("66a200000000000000000001"), release_id: NumberLong(9101), tag_name: "v2.1.0-beta.2", name: "Beta <Ünï>",
   body: "**Fixed** the [gem builder](https://x.example) and `code` " + "word ".repeat(60), html_url: "https://github.com/x/r/9101",
   prerelease: true, published_at: at(-2), assets: [asset("BTT-2.1.0.msi", "https://dl/1", 10)], fetched_at: at(-1)},
  {_id: oid("66a200000000000000000002"), release_id: NumberLong(9100), tag_name: "v2.0.0", name: "",
   body: "", html_url: "https://github.com/x/r/9100", prerelease: false, published_at: at(-10), assets: [], fetched_at: at(-1)},
]);

db.trove_events.insertMany([
  {_id: oid("66a400000000000000000001"), event_id: "w-on", name: "Ongoing & Event", url: "https://t/e1", category: "Event",
   image: null, icon: null, lookup: null, starts_at: secs(-1), ends_at: secs(2), first_seen_at: at(-3), updated_at: at(-1)},
  {_id: oid("66a400000000000000000002"), event_id: "w-up", name: "Upcoming <Sale>", url: "", category: "Sale",
   image: null, icon: null, lookup: null, starts_at: secs(3), ends_at: secs(5), first_seen_at: at(-3), updated_at: at(-1)},
]);

db.trove_news.insertMany([
  {_id: oid("66a500000000000000000001"), url: "https://trovegame.com/n/w1", title: "Patch <Notes>", author: "Team Trove",
   summary: "s", category: "News", categories: ["News"], image: null, published_at: at(-1), first_seen_at: at(-1), updated_at: at(-1)},
  {_id: oid("66a500000000000000000002"), url: "https://trovegame.com/n/w2", title: "Store Sale", author: "Team Trove",
   summary: "", category: "Shop Offers", categories: ["Shop Offers"], image: null, published_at: at(-1.5),
   first_seen_at: at(-1), updated_at: at(-1)},
]);

db.feed_cache.insertMany([
  {_id: oid("66a600000000000000000001"), feed: "youtube", fetched_at: at(-0.1), items: [
    {title: "Vid & more", url: "https://y/1", channel: "C", video_id: "1", published_at: "2026-10-20T10:00:00Z",
     thumbnail_url: "https://i.example/{width}x{height}.jpg"},
    {title: "Ünï", url: "https://y/2", channel: null, published_at: null},
  ]},
  {_id: oid("66a600000000000000000002"), feed: "twitch", fetched_at: at(-0.2), items: [
    {title: "Live now", url: "https://twitch.example/a", channel: "A", thumbnail_url: "https://t.example/{width}-{height}.jpg"},
  ]},
]);

const day = (d) => new Date(NOW - d * DAY).toISOString().slice(0, 10);
const row = (title, slug) => ({title, handle: "aallyn", slug,
  hub: {top_release: NumberInt(3), downloads_7d: NumberInt(1), stars: NumberInt(0)}, trovesaurus: null, steam: null});
db.mod_stats_snapshots.insertOne({_id: ObjectId("673000000000000000000101"), day: day(0), taken_at: new Date(NOW),
  mods: [row("Zakros UI - Chat", "chat"), row("Zakros BP - Ships", "ships"), row("Zakros FX - <Glow>", "glow"),
         row("Loose", "loose"), row("Zakros UI - alpha", "alpha")]});

const gw = (n, title, prize, starts, ends, status, winner, entries) => ({
  _id: oid("66a3000000000000000000" + n), title, description: "A **prize** <b>draw</b>", prize_name: prize,
  starts_at: at(starts), ends_at: at(ends), status, vault_item_id: null, prize_code_id: null, winner_user_id: null,
  winner_username: winner, drawn_at: winner ? at(ends) : null, entry_count: NumberInt(entries),
  created_at: at(starts - 1), updated_at: at(starts - 1)});
db.giveaways.insertMany([
  gw("01", "Weekly draw", "Sunset Sail", -1, 3, "open", null, 1234),
  gw("02", "Last week", "", -9, -2, "drawn", "Win & Ner", 0),
]);

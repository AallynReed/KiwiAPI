// Snapshots as the loop stores them. Today's was just taken, so both loops (A's
// Python, B's kiwi-server) find it fresh and skip the platforms entirely.
const now = new Date();
const day = (d) => new Date(now.getTime() - d * 86400000).toISOString().slice(0, 10);
const row = (title, slug, top, ts, st) => ({
  title, handle: "aallyn", slug,
  hub: {top_release: NumberInt(top), downloads_7d: NumberInt(3), stars: NumberInt(1)},
  trovesaurus: ts, steam: st,
});
const mods = [
  row("Zakros BP - Ships", "ships", 0, {}, null),
  row("Zakros UI - Chat", "chat", 1200, {id: NumberInt(11), downloads: NumberInt(99), likes: NumberInt(1)},
      {id: NumberInt(2001), subscriptions: NumberInt(10), lifetime_subscriptions: NumberInt(25), favorites: NumberInt(3),
       views: NumberInt(400)}),
  row("Zakros UI - Clubs", "clubs", 7, null, null),
];
db.mod_stats_snapshots.insertMany([
  {_id: ObjectId("673000000000000000000001"), day: day(0), taken_at: now, mods},
  {_id: ObjectId("673000000000000000000002"), day: day(1), taken_at: new Date(now.getTime() - 86400000),
   mods: mods.slice(0, 2)},
  {_id: ObjectId("673000000000000000000003"), day: day(3), taken_at: new Date(now.getTime() - 3 * 86400000),
   mods: [{title: "Old shape", hub: {top_release: 2.5}, trovesaurus: {downloads: 0.1}, steam: null}]},
]);

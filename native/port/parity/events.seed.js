// Every event source with something to announce, around whatever "now" the run hits.
const now = Math.floor(Date.now() / 1000);
const names = ["Cursed Vale", "RAMPAGE ALERT!", "Collection Challenge", "Racing Challenge", "Target Challenge", "Ünïcode \"Dungeon\""];
const challenges = [];
for (let a = Math.floor(now / 1800) * 1800 - 7200, i = 0; a < now + 172800; a += 1800, i++) {
  challenges.push({window_anchor: a, window_ends_at: a + 1200, name: names[i % names.length],
    is_friday_window: new Date((a - 39600) * 1000).getUTCDay() === 5, captured_at: new Date((a + 61) * 1000 + 777)});
}
db.challenge_captures.insertMany(challenges);

const base = Date.UTC(2023, 6, 18, 11) / 1000;
const week = base + Math.floor((now - base) / 604800) * 604800;
db.chaos_chest_captures.insertMany([
  {week_anchor: week, name: "Parity Chest", captured_at: new Date((week + 300) * 1000 + 123)},
  {week_anchor: week + 604800, name: "Next Parity Chest", captured_at: new Date((week + 604800 + 300) * 1000)},
]);

const reset = Math.floor((now - 39600) / 86400) * 86400 + 39600;
db.luxion_appearances.insertOne({started_at: reset - 86400, first_seen_at: new Date((reset - 86400 + 3600) * 1000 + 5),
  last_seen_at: new Date((now - 60) * 1000)});

db.trove_news.insertOne({url: "https://trovegame.com/parity", title: "Parity patch notes Ünï", author: "Team Trove",
  summary: "", category: "News", categories: ["News"], image: null, published_at: ISODate("2026-10-20T15:30:00.250Z"),
  first_seen_at: ISODate("2026-10-20T16:00:00Z"), updated_at: ISODate("2026-10-20T16:00:00Z")});

db.giveaways.insertOne({_id: ObjectId("65f0c0ffee0000000000abcd"), title: "Parity \"giveaway\"", description: null,
  prize_name: "Dragon", starts_at: ISODate("2026-10-01T00:00:00Z"), ends_at: ISODate("2099-01-01T00:00:00Z"),
  status: "open", vault_item_id: null, prize_code_id: null, winner_user_id: null, winner_username: null, drawn_at: null,
  entry_count: 0, created_at: ISODate("2026-10-01T12:00:00.999Z"), updated_at: ISODate("2026-10-01T12:00:00.999Z")});

db.update_versions.insertMany([
  {branch: "live-us", ordinal: 7, version_tag: "a20.7", status: "complete", captured_at: ISODate("2026-10-21T10:00:00Z"),
   completed_at: ISODate("2026-10-21T10:05:00.456Z"), content_path: "", motd: "", files_added: 3, files_modified: 40,
   files_removed: 1, bytes_added: NumberLong("5000000000")},
  {branch: "live-us", ordinal: 8, version_tag: "a20.8", status: "in_progress", captured_at: ISODate("2026-10-22T10:00:00Z"),
   completed_at: null, content_path: "", motd: "", files_added: 0, files_modified: 0, files_removed: 0, bytes_added: 0},
]);

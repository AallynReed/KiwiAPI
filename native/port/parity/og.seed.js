// What the board and banners draw, around whatever "now" the run hits.
const now = Math.floor(Date.now() / 1000);
const names = ["Cursed Vale", "RAMPAGE ALERT!", "Ünïcode \"Dungeon\" of the Very Long Name That Must Shrink"];
const challenges = [];
for (let a = Math.floor(now / 1800) * 1800 - 7200, i = 0; a < now + 172800; a += 1800, i++) {
  challenges.push({window_anchor: a, window_ends_at: a + 1200, name: names[i % names.length],
    is_friday_window: new Date((a - 39600) * 1000).getUTCDay() === 5, captured_at: new Date((a + 61) * 1000 + 777)});
}
db.challenge_captures.insertMany(challenges);

const base = Date.UTC(2023, 6, 18, 11) / 1000;
const week = base + Math.floor((now - base) / 604800) * 604800;
db.chaos_chest_captures.insertOne({week_anchor: week, name: "Parity Chest Ultimate Shadow Hunter Style",
  captured_at: new Date((week + 300) * 1000 + 123)});

const reset = Math.floor((now - 39600) / 86400) * 86400 + 39600;
db.luxion_appearances.insertOne({started_at: reset - 86400, first_seen_at: new Date((reset - 86400 + 3600) * 1000 + 5),
  last_seen_at: new Date((now - 60) * 1000)});

db.update_versions.insertMany([
  {branch: "live-us", ordinal: 1186, version_tag: "older", status: "complete", captured_at: new Date((now - 90000) * 1000),
    completed_at: new Date((now - 89000) * 1000), content_path: "", motd: "", files_added: 1, files_modified: 2,
    files_removed: 3, bytes_added: 4},
  {branch: "live-us", ordinal: 1187, version_tag: "live-us 2026.10.21 (#1187)", status: "complete",
    captured_at: new Date((now - 3600) * 1000), completed_at: new Date((now - 3000) * 1000), content_path: "",
    motd: "", files_added: 12, files_modified: 3456, files_removed: 0, bytes_added: 99},
  {branch: "live-us", ordinal: 1188, version_tag: "in flight", status: "in_progress",
    captured_at: new Date((now - 60) * 1000), completed_at: null, content_path: "", motd: "", files_added: 0,
    files_modified: 0, files_removed: 0, bytes_added: 0},
  {branch: "pts", ordinal: 9999, version_tag: "pts only", status: "complete", captured_at: new Date(now * 1000),
    completed_at: new Date(now * 1000), content_path: "", motd: "", files_added: 5, files_modified: 5,
    files_removed: 5, bytes_added: 5},
]);

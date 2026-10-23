// Custom art requests in every state the review queue shows, shaped as Beanie
// writes them. Their pictures are not on disk in either stack.
const pic = (n) => ({sha: n.repeat(64), content_type: "image/png", width: NumberInt(8), height: NumberInt(8)});
const req = (id, o) => Object.assign({
  _id: ObjectId(id), kind: "club", name: "Seed", email: "seed@aallyn.net", note: null, pictures: {pfp: pic("a")},
  status: "pending", reason: null, batch: null, log: null, versions: {}, steam_pending: false,
  created_at: ISODate("2026-10-01T10:00:00.123Z"), decided_at: null, released_at: null,
}, o);
db.art_requests.insertMany([
  req("672000000000000000000001", {name: "Seed Failed A", status: "failed", batch: "parity-batch",
    log: "$ git fetch origin\nFAILED exit 1: git fetch origin", versions: {"Zakros UI - Chat": "1.0.4"},
    decided_at: ISODate("2026-10-02T08:00:00Z"), created_at: ISODate("2026-10-01T10:00:00.123Z")}),
  req("672000000000000000000002", {name: "Seed Failed B", kind: "player", status: "failed", batch: "parity-batch",
    log: "FAILED", decided_at: ISODate("2026-10-02T08:00:01Z"), created_at: ISODate("2026-10-01T10:00:01.456Z")}),
  req("672000000000000000000003", {name: "Seed Failed Other", status: "failed", batch: "other-batch",
    created_at: ISODate("2026-10-01T10:00:02Z")}),
  req("672000000000000000000004", {name: "Seed Released", pictures: {pfp: pic("b"), banner: {sha: "c".repeat(64),
    content_type: "image/jpeg", width: NumberInt(60), height: NumberInt(20)}}, status: "released", steam_pending: true,
    versions: {"Zakros UI - Chat": "1.0.5", "Zakros UI - Nameplate": "2.1", "Zakros UI - Clubs": "0.3"},
    decided_at: ISODate("2026-10-03T08:00:00Z"), released_at: ISODate("2026-10-04T09:30:00.250Z"),
    created_at: ISODate("2026-10-01T11:00:00Z")}),
  req("672000000000000000000005", {name: "Seed Denied", kind: "player", status: "denied", reason: "Not a picture of you.",
    decided_at: ISODate("2026-10-03T08:00:00Z"), created_at: ISODate("2026-10-01T12:00:00Z")}),
  req("672000000000000000000006", {name: "Seed Old Pending", note: "an old note", created_at: ISODate("2026-09-01T00:00:00Z")}),
]);

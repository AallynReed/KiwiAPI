// Mods Hub creator links + issues twin seed: four site users (the creator holds
// creator token "kiwi_creator_ParityTwinSeedTokenNumberOne00"), three mods and
// one open issue with a reply. The specs sign in with fixed site JWTs minted
// for these ids against the sandbox SECRET_KEY.
const T0 = ISODate("2026-09-01T12:00:00.250Z"), T1 = ISODate("2026-09-02T08:15:00Z");
const CREATOR = ObjectId("65f1000000000000000000b1"), PLAYER = ObjectId("65f1000000000000000000b2");
const COLLAB = ObjectId("65f1000000000000000000b3"), STRANGER = ObjectId("65f1000000000000000000b4");
const user = (_id, username, extra) => Object.assign({
  _id, username, discord_handle: username, notify_email: null, display_name: null, is_active: true,
  is_verified: true, is_deleted: false, discord_id: null, discord_avatar: null, token_version: NumberInt(0),
  claimed_trove_name: null, claimed_trove_display: null, claimed_at: null, claim_verified: false,
  claim_baseline: {}, claim_verified_at: null, creator_token_hash: null, creator_token_prefix: null,
  creator_token_at: null, mod_issues_seen_at: null, created_at: T0, updated_at: T0, last_login_at: null,
}, extra);
db.site_users.insertMany([
  user(CREATOR, "parity_creator", {display_name: "Parity Creator",
    creator_token_hash: "df6e90daa0e46a605f0c43ae25dd911dc70e1a2d7cf954830b81fedb449f676a",
    creator_token_prefix: "kiwi_creator_ParityTw", creator_token_at: T0}),
  user(PLAYER, "parity_player", {display_name: "Parity Player", mod_issues_seen_at: T0}),
  user(COLLAB, "parity_collab"),
  user(STRANGER, "parity_stranger"),
]);

const ALPHA = ObjectId("65f1000000000000000000c1"), DRAFT = ObjectId("65f1000000000000000000c2");
const CLOSED = ObjectId("65f1000000000000000000c3");
const project = (_id, slug, title, extra) => Object.assign({
  _id, slug, title, summary: "", description: "", tags: [], owner_id: CREATOR, owner_username: "parity_creator",
  owner_handle: "parity_creator", collaborators: [], is_stray: false, issues_enabled: true,
  issue_seq: NumberInt(0), open_issue_count: NumberInt(0), visibility: "public", taken_down: false,
  is_remix: false, created_at: T0, updated_at: T0,
}, extra);
db.mod_projects.insertMany([
  project(ALPHA, "parity-alpha", "Parity Alpha", {collaborators: [{user_id: COLLAB, username: "parity_collab"}],
    issue_seq: NumberInt(1), open_issue_count: NumberInt(1)}),
  project(DRAFT, "parity-draft", "Parity Draft", {visibility: "draft"}),
  project(CLOSED, "parity-closed", "Parity Closed", {issues_enabled: false}),
]);

const ISSUE = ObjectId("65f1000000000000000000d1");
db.mod_issues.insertOne({
  _id: ISSUE, project_id: ALPHA, project_slug: "parity-alpha", project_handle: "parity_creator",
  project_title: "Parity Alpha", number: NumberInt(1), kind: "issue", title: "Seeded bug", body: "It **breaks**.",
  author_id: PLAYER, author_username: "Parity Player", status: "open", closed_at: null, closed_by_id: null,
  comment_count: NumberInt(1), participant_ids: [COLLAB, CREATOR, PLAYER], created_at: T0, updated_at: T1,
  last_activity_at: T1, last_activity_by: CREATOR,
});
db.mod_issue_events.insertOne({
  _id: ObjectId("65f1000000000000000000e1"), issue_id: ISSUE, project_id: ALPHA, kind: "comment",
  body: "Looking into it.", author_id: CREATOR, author_username: "Parity Creator", created_at: T1,
});

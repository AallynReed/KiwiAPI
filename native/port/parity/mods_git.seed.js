// Mods Hub git twin seed: a creator (signed in with fixed sandbox site JWTs), a
// collaborator, a stranger and a disabled account; their git tokens (raw values in
// mods_git.writes.json); four mods shaped as Beanie writes them; the parity API
// account connected to the creator; and a small live-us file index for placement.
const T0 = ISODate("2026-01-01T00:00:00Z");
const OWNER = ObjectId("65f0000000000000000c0001"), COLLAB = ObjectId("65f0000000000000000c0002");
const STRANGER = ObjectId("65f0000000000000000c0003"), GONE = ObjectId("65f0000000000000000c0004");
const ADMIN = db.users.findOne({email: "parity@aallyn.net"})._id;

function user(id, username, extra) {
  return Object.assign({
    _id: id, username: username, discord_handle: username, notify_email: null, display_name: null,
    is_active: true, is_verified: true, is_deleted: false, discord_id: null, discord_avatar: null,
    token_version: NumberInt(0), claimed_trove_name: null, claimed_trove_display: null, claimed_at: null,
    claim_verified: false, claim_baseline: {}, claim_verified_at: null, creator_token_hash: null,
    creator_token_prefix: null, creator_token_at: null, mod_issues_seen_at: null,
    created_at: T0, updated_at: T0, last_login_at: null,
  }, extra);
}

db.site_users.insertMany([
  user(OWNER, "paritygit", {display_name: "Parity Git"}),
  user(COLLAB, "paritycollab"),
  user(STRANGER, "paritystranger"),
  user(GONE, "paritygone", {is_active: false}),
]);

function project(id, slug, extra) {
  return Object.assign({
    _id: ObjectId(id), slug: slug, title: slug, summary: "", description: "", readme_text: "", warnings: "",
    title_i18n: {}, summary_i18n: {}, description_i18n: {}, readme_i18n: {}, warnings_i18n: {}, tags: [],
    owner_id: OWNER, owner_username: "Parity Git", owner_handle: "paritygit", collaborators: [],
    is_stray: false, stray_status: null, author: "", source: null, source_id: null, source_url: null,
    source_author_id: null, source_file_id: null, source_likes: NumberInt(0), uploaded_on_behalf: false,
    issues_enabled: true, issue_seq: NumberInt(0), open_issue_count: NumberInt(0), visibility: "public",
    is_beta: false, mode: "files", source_visibility: "public", default_branch: "main", banner_sha: null,
    preview_shas: [], discord_url: null, website_url: null, donation_urls: [], download_count: NumberInt(0),
    star_count: NumberInt(0), downloads_7d: NumberInt(0), popularity_score: 0.0, hidden_release_branches: [],
    branch_order: [], forked_from_id: null, forked_from_slug: null, forked_from_handle: null,
    forked_from_title: null, forked_from_owner: null, inspired_by_slug: null, inspired_by_handle: null,
    inspired_by_title: null, inspired_by_owner: null, fork_count: NumberInt(0), is_remix: false,
    remix_status: "pending", remix_requested_at: null, remix_resolved_at: null, remix_resolved_by: null,
    taken_down: false, takedown_reason: null, created_at: T0, updated_at: T0, last_release_at: null,
  }, extra);
}

db.mod_projects.insertMany([
  project("65f0000000000000000c0101", "git-files",
          {collaborators: [{user_id: COLLAB, username: "paritycollab"}]}),
  project("65f0000000000000000c0102", "git-private", {source_visibility: "private"}),
  project("65f0000000000000000c0103", "git-releases", {mode: "releases"}),
  project("65f0000000000000000c0104", "git-draft", {visibility: "draft"}),
]);

function token(id, owner, hash, prefix, name, revoked) {
  return {_id: ObjectId(id), site_user_id: owner, token_hash: hash, prefix: prefix, name: name,
          revoked: revoked, created_at: T0, last_used_at: null};
}

db.mod_git_tokens.insertMany([
  token("65f0000000000000000c0201", OWNER, "6b3c20da04aef456e8331d9d29a5ed1b6b63d3c13c94f9b2963fde125f58c64d",
        "kgit_parity-", "owner laptop", false),
  token("65f0000000000000000c0202", COLLAB, "be8c44439e3b0a4504c250bab31a3526701d46e764423869dd7ac5252e15d730",
        "kgit_parity-", "collab", false),
  token("65f0000000000000000c0203", GONE, "d5c69c385267db49ee38e1bc26eb9c1abfda7129de3777b422c3cdd3fdebee35",
        "kgit_parity-", "gone", false),
  token("65f0000000000000000c0204", OWNER, "cb451a71a79403b491048ce06eefe8e2c787b7cd1b3b9cdcc105338b7c94aa47",
        "kgit_parity-", "old revoked", true),
]);

db.mod_creator_links.insertOne({_id: ObjectId("65f0000000000000000c0301"), site_user_id: OWNER, api_user_id: ADMIN,
  label: "parity", all_projects: true, project_ids: [], revoked: false, revoked_at: null,
  revoked_by_rotation: false, created_at: T0, last_used_at: null, request_count: NumberInt(0)});

db.update_branches.insertOne({branch: "live-us", content_path: "", current_version: "parity",
  current_ordinal: NumberInt(7), last_probe_at: null, status: "idle", last_error: null});
db.update_state.insertMany([
  {branch: "live-us", path: "ui/hud/a.txt"},
  {branch: "live-us", path: "particles/fx/p.pkfx"},
  {branch: "live-us", path: "textures/t.dds"},
  {branch: "live-us", path: "particles/other/p.pkfx"},
]);

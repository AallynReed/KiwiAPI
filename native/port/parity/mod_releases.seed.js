// A verified creator with one public files-mode mod, connected to the admin's API account,
// so the parity token cuts releases through get_mod_write_user's API path on both stacks.
const t0 = new Date("2026-01-02T03:04:05.123Z");
const C = ObjectId("65ae00000000000000000c01"), P = ObjectId("65ae00000000000000000f01");
const Q = ObjectId("65ae00000000000000000f02"), L = ObjectId("65ae00000000000000000e01");
const admin = db.users.findOne({is_superuser: true});

db.site_users.deleteMany({_id: C});
db.site_users.insertOne({_id: C, username: "parity_creator", discord_handle: "ParityCreator", notify_email: null,
  display_name: "Parity Creator", is_active: true, is_verified: true, is_deleted: false, discord_id: null,
  discord_avatar: null, token_version: NumberInt(0), claim_baseline: {}, creator_token_hash: null,
  creator_token_prefix: null, creator_token_at: null, mod_issues_seen_at: null, created_at: t0, updated_at: t0,
  last_login_at: null});
db.mod_creator_links.deleteMany({_id: L});
db.mod_creator_links.insertOne({_id: L, site_user_id: C, api_user_id: admin._id, label: "parity", all_projects: true,
  project_ids: [], revoked: false, revoked_at: null, revoked_by_rotation: false, created_at: t0, last_used_at: null,
  request_count: NumberInt(0)});
db.mod_projects.deleteMany({_id: {$in: [P, Q]}});
db.mod_projects.insertMany([
  {_id: P, slug: "parity-mod", title: "Parity Mod", summary: "A mod for parity runs", description: "", readme_text: "",
   warnings: "", title_i18n: {}, summary_i18n: {}, description_i18n: {}, readme_i18n: {}, warnings_i18n: {},
   tags: ["gui", "radar"], owner_id: C, owner_username: "Parity Creator", owner_handle: "parity_creator",
   collaborators: [], is_stray: false, stray_status: null, author: "", source: null, source_id: null,
   source_url: null, source_author_id: null, source_file_id: null, source_likes: NumberInt(0),
   uploaded_on_behalf: false, issues_enabled: true, issue_seq: NumberInt(0), open_issue_count: NumberInt(0),
   visibility: "public", is_beta: false, mode: "files", source_visibility: "public", default_branch: "main",
   banner_sha: null, preview_shas: [], discord_url: null, website_url: null, donation_urls: [],
   download_count: NumberInt(0), star_count: NumberInt(0), downloads_7d: NumberInt(0), popularity_score: 0.5,
   hidden_release_branches: ["alt"], branch_order: [], forked_from_id: null, forked_from_slug: null,
   forked_from_handle: null, forked_from_title: null, forked_from_owner: null, inspired_by_slug: null,
   inspired_by_handle: null, inspired_by_title: null, inspired_by_owner: null, fork_count: NumberInt(0),
   is_remix: false, remix_status: "pending", remix_requested_at: null, remix_resolved_at: null,
   remix_resolved_by: null, taken_down: false, takedown_reason: null, created_at: t0, updated_at: t0,
   last_release_at: null},
  {_id: Q, slug: "parity-draft", title: "Parity Draft", owner_id: C, owner_username: "Parity Creator",
   owner_handle: "parity_creator", visibility: "draft", mode: "releases", uploaded_on_behalf: true,
   created_at: t0, updated_at: t0},
]);
db.mod_releases.deleteMany({project_id: {$in: [P, Q]}});
db.mod_download_events.deleteMany({project_id: {$in: [P, Q]}});

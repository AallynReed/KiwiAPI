// Mods Hub / Modpacks moderation twin seed. The stray import is seeded running so no start reaches the network.
const t0 = ISODate("2025-10-09T08:53:20.123Z");
const SRC = ["trove", "saurus"].join("");
const at = (s) => ISODate(s);
const id = (n) => ObjectId("65ae" + n.toString(16).padStart(20, "0"));
const OWNER = id(1), CLAIMANT = id(2), NEWOWNER = id(3), QUIET = id(4);
const P_PUB = id(0x10), P_DRAFT = id(0x11), P_WEB = id(0x12), S_PEND = id(0x13), S_APPR = id(0x14),
      S_REJ = id(0x15), N_CLASH = id(0x16), RX_T = id(0x17), RX_O = id(0x18);
const PACK1 = id(0x30), PACK2 = id(0x31), PROF1 = id(0x40);

function siteUser(_id, username, extra) {
  return Object.assign({_id, username, discord_handle: "", notify_email: null, display_name: null, is_active: true,
    is_verified: true, is_deleted: false, discord_id: null, discord_avatar: null, token_version: NumberInt(0),
    claimed_trove_name: null, claimed_trove_display: null, claimed_at: null, claim_verified: false,
    claim_baseline: {}, claim_verified_at: null, creator_token_hash: null, creator_token_prefix: null,
    creator_token_at: null, mod_issues_seen_at: null, created_at: t0, updated_at: t0, last_login_at: null}, extra);
}

function project(_id, slug, title, extra) {
  return Object.assign({_id, slug, title, summary: "", owner_id: OWNER, owner_username: "Owner Name",
    owner_handle: "mmowner", visibility: "public", created_at: at("2025-06-15T15:06:40Z"),
    updated_at: at("2025-10-20T22:40:00Z")}, extra);
}

db.site_users.insertMany([
  siteUser(OWNER, "mmowner", {display_name: "Owner Name", notify_email: "mmowner@kiwi-mail.net"}),
  siteUser(CLAIMANT, "mmclaimant", {notify_email: "mmclaimant@kiwi-mail.net"}),
  siteUser(NEWOWNER, "mmnewowner", {display_name: "New Owner"}),
  siteUser(QUIET, "mmquiet", {display_name: ""}),
]);

db.mod_projects.insertMany([
  project(P_PUB, "mm-kiwi-sword", "Kiwi Sword", {summary: "A sword", tags: ["weapon", "sword"], banner_sha: "e".repeat(64),
    preview_shas: ["f".repeat(64), "e".repeat(64)], download_count: NumberLong(42), star_count: NumberInt(3),
    downloads_7d: NumberInt(7), popularity_score: 0.1234567, is_beta: true,
    title_i18n: {fr: "Épée Kiwi", de: "Kiwi-Schwert"}, fork_count: NumberInt(1),
    last_release_at: at("2025-09-27T19:06:40.500Z"), updated_at: at("2025-10-21T22:40:00.001Z")}),
  project(P_DRAFT, "mm-draft-mod", "Draft Mod", {visibility: "draft",
    collaborators: [{user_id: CLAIMANT, username: "mmclaimant", extra: NumberInt(1)}],
    inspired_by_slug: "mm-kiwi-sword", inspired_by_handle: "mmowner", inspired_by_title: "Kiwi Sword",
    inspired_by_owner: "Owner Name", popularity_score: NumberInt(1), updated_at: at("2025-10-15T03:46:40Z")}),
  project(P_WEB, "mm-web-only", "Web Only", {visibility: "public_website_only", popularity_score: 0.5,
    uploaded_on_behalf: true, author: "Someone Else", updated_at: at("2025-10-03T14:00:00Z")}),
  project(S_PEND, "mm-lost-mod", "Lost Mod", {owner_id: null, owner_username: "Ghost", owner_handle: "stray",
    is_stray: true, stray_status: "pending", visibility: "draft", author: "Ghost", mode: "releases",
    source: SRC, source_id: "mm101", updated_at: at("2025-09-16T05:20:00Z")}),
  project(S_APPR, "mm-clash-mod", "MM Clash Mod", {owner_id: null, owner_username: "Wraith", owner_handle: "stray",
    is_stray: true, stray_status: "approved", author: "Wraith", mode: "releases", updated_at: at("2025-09-04T15:33:20Z")}),
  project(S_REJ, "mm-nope-mod", "Nope Mod", {owner_id: null, owner_username: "Shade", owner_handle: "stray",
    is_stray: true, stray_status: "rejected", visibility: "draft", author: "Shade", updated_at: at("2025-08-24T01:46:40Z")}),
  project(N_CLASH, "mm-clash-mod", "Theirs", {owner_id: NEWOWNER, owner_username: "New Owner",
    owner_handle: "mmnewowner", updated_at: at("2025-08-12T12:00:00Z")}),
  project(RX_T, "mm-remix-target", "Remix Target", {owner_id: QUIET, owner_username: "mmquiet", owner_handle: "mmquiet",
    updated_at: at("2025-07-31T22:13:20Z")}),
  project(RX_O, "mm-remix-original", "Remix Original", {fork_count: NumberLong(2), updated_at: at("2025-07-20T08:26:40Z")}),
]);

db.mod_releases.insertMany([
  {_id: id(0x80), project_id: P_PUB, owner_id: OWNER, tag: "v1", tmod_sha: "a".repeat(64), prior_tmod_shas: ["b".repeat(64), ""],
   tmod_size: NumberInt(1000), status: "published", created_at: t0, updated_at: t0, published_at: t0, announced_at: t0},
  {_id: id(0x81), project_id: S_PEND, owner_id: null, tag: "3", tmod_sha: "d".repeat(64), tmod_size: NumberInt(10),
   status: "published", created_at: t0, updated_at: t0, published_at: t0, announced_at: null},
  {_id: id(0x82), project_id: P_PUB, owner_id: OWNER, tag: "v2", branch: "beta", tmod_sha: "c".repeat(64),
   tmod_size: NumberLong(500), status: "draft", created_at: t0, updated_at: t0},
  {_id: id(0x83), project_id: P_DRAFT, owner_id: OWNER, tag: "v0", tmod_sha: "c".repeat(64), tmod_size: NumberInt(5),
   status: "published", created_at: t0, updated_at: t0},
  {_id: id(0x84), project_id: S_APPR, owner_id: null, tag: "1", tmod_sha: "9".repeat(64), tmod_size: NumberInt(7),
   status: "published", created_at: t0, updated_at: t0, published_at: t0},
]);

db.mod_images.insertMany([
  {_id: id(0x90), sha: "e".repeat(64), content_type: "image/png", byte_size: NumberInt(2048), owner_id: OWNER,
   width: NumberInt(10), height: NumberInt(10), variants: {}, created_at: t0},
  {_id: id(0x91), sha: "f".repeat(64), content_type: "image/webp", byte_size: NumberLong(300), owner_id: OWNER,
   width: null, height: null, variants: {"200": "a".repeat(64)}, created_at: t0},
]);

db.mod_profiles.insertOne({_id: PROF1, site_user_id: OWNER, handle: "mmowner", display_name: "", tagline: "hi",
  created_at: t0, updated_at: t0});

db.content_reports.insertMany([
  {_id: id(0x50), target_type: "mod", target_id: P_PUB, target_label: "Kiwi Sword", target_url: "/mods/mmowner/mm-kiwi-sword",
   reason: "stolen", resolved: false, created_at: at("2025-10-22T10:26:40Z")},
  {_id: id(0x51), target_type: "profile", target_id: PROF1, target_label: "mmowner", target_url: "/mods/mmowner",
   reason: "spam", resolved: false, created_at: at("2025-10-23T14:13:20Z")},
  {_id: id(0x52), target_type: "modpack", target_id: PACK1, target_label: "Pack", target_url: "/modpacks/mmowner/mm-pack",
   reason: "old", resolved: true, created_at: at("2025-10-20T22:40:00Z")},
  {_id: id(0x53), target_type: "modpack", target_id: PACK1, target_label: "Pack", target_url: "/modpacks/mmowner/mm-pack",
   reason: "broken", resolved: false, created_at: at("2025-10-24T00:00:00Z")},
]);

db.mod_claim_requests.insertMany([
  {_id: id(0x60), project_id: S_PEND, project_slug: "mm-lost-mod", project_title: "Lost Mod", claimant_id: CLAIMANT,
   claimant_username: "mmclaimant", message: "mine", status: "pending", resolved_by: null, resolved_at: null,
   created_at: at("2025-10-24T10:00:00Z")},
  {_id: id(0x61), project_id: S_PEND, project_slug: "mm-lost-mod", claimant_id: QUIET, claimant_username: "mmquiet",
   status: "pending", created_at: at("2025-10-23T06:13:20Z")},
  {_id: id(0x62), project_id: S_REJ, project_slug: "mm-nope-mod", project_title: "Nope Mod", claimant_id: CLAIMANT,
   claimant_username: "mmclaimant", status: "approved", resolved_by: null, resolved_at: at("2025-10-21T09:00:00.500Z"),
   created_at: at("2025-10-09T08:53:20Z")},
  {_id: id(0x63), project_id: S_APPR, project_slug: "mm-clash-mod", claimant_id: CLAIMANT, claimant_username: "mmclaimant",
   status: "pending", created_at: at("2025-10-22T00:00:00Z")},
]);

db.mod_remix_claims.insertMany([
  {_id: id(0x70), target_id: RX_T, target_slug: "mm-remix-target", target_title: "Remix Target", original_id: RX_O,
   original_slug: "mm-remix-original", original_title: "Remix Original", claimant_id: OWNER, claimant_username: "mmowner",
   message: "it's mine", status: "pending", created_at: at("2025-10-25T13:46:40Z")},
  {_id: id(0x71), target_id: RX_T, original_id: P_PUB, claimant_id: OWNER, claimant_username: "mmowner",
   status: "pending", created_at: at("2025-10-24T10:00:00Z")},
]);

db.mod_taken_down_artifacts.insertOne({_id: id(0xA0), sha256: "d".repeat(64), md5: null, project_id: S_REJ,
  title: "Nope Mod", owner_handle: "stray", created_at: t0});
db.mod_stars.insertOne({_id: id(0xB0), project_id: P_DRAFT, site_user_id: CLAIMANT, created_at: t0});
db.mod_issues.insertOne({_id: id(0xB1), project_id: P_DRAFT, number: NumberInt(1), title: "bug", author_id: CLAIMANT});
db.mod_issue_events.insertOne({_id: id(0xB2), issue_id: id(0xB1), project_id: P_DRAFT, author_id: CLAIMANT});

db.modpack_projects.insertMany([
  {_id: PACK1, slug: "mm-pack", title: "Kiwi Pack", summary: "best", tags: ["pvp"], owner_id: OWNER,
   owner_username: "Owner Name", owner_handle: "mmowner", visibility: "public",
   variants: [{name: "default", label: "", entries: [
       {project_id: P_PUB, handle: "mmowner", slug: "mm-kiwi-sword", title: "Kiwi Sword", junk: NumberInt(1)},
       {custom_sha: "c".repeat(64), custom_filename: "X.tmod", title: "X"}]},
     {name: "lite", label: "Lite", entries: []}],
   default_variant: "lite", created_at: t0, updated_at: at("2025-10-20T22:40:00Z")},
  {_id: PACK2, slug: "mm-pack-2", title: "Second Pack", owner_id: QUIET, owner_username: "mmquiet", visibility: "draft",
   preview_shas: ["f".repeat(64)], created_at: t0, updated_at: t0},
]);
db.modpack_stars.insertOne({_id: id(0xC0), modpack_id: PACK2, site_user_id: OWNER, created_at: t0});

db.stray_import_state.deleteMany({key: SRC});
db.stray_import_state.insertOne({_id: id(0xD0), key: SRC, running: true, phase: "importing",
  total: NumberInt(12), processed: NumberInt(3), imported: NumberInt(2), updated: NumberInt(1),
  pending_added: NumberInt(0), failed: NumberInt(0), last_error: null, started_at: new Date(Date.now() - 5 * 60000),
  finished_at: null, updated_at: t0});

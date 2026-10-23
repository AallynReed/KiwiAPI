// Twin sandbox seed for the Mods Hub catalog: three site users (signed in with
// fixed sandbox site JWTs), a spread of projects, releases, a profile, a star,
// recent download events and an orphan taken-down artifact, shaped as Beanie
// writes them. No git repos or CAS blobs: the specs never read either.
const T0 = ISODate("2026-01-05T10:00:00.250Z"), T1 = ISODate("2026-03-01T08:30:00Z"), T2 = ISODate("2026-06-01T12:00:00.500Z");
const OWNER = ObjectId("65f0000000000000000000c1"), COLLAB = ObjectId("65f0000000000000000000c2"),
      STRANGER = ObjectId("65f0000000000000000000c3");
const user = (_id, username, display_name, discord_id, discord_avatar) => ({
  _id, username, discord_handle: username, notify_email: null, display_name, is_active: true,
  is_verified: true, is_deleted: false, discord_id, discord_avatar, token_version: NumberInt(0),
  claimed_trove_name: null, claimed_trove_display: null, claimed_at: null, claim_verified: false,
  claim_baseline: {}, created_at: T0, updated_at: T0,
});
db.site_users.insertMany([
  user(OWNER, "paritymodder", "Parity Modder", NumberLong("312345678901234567"), "a_abc123"),
  user(COLLAB, "paritycollab", null, NumberLong("412345678901234567"), null),
  user(STRANGER, "paritystranger", "  ", null, null),
]);

const project = (o) => Object.assign({
  summary: "", description: "", readme_text: "", warnings: "", title_i18n: {}, summary_i18n: {},
  description_i18n: {}, readme_i18n: {}, warnings_i18n: {}, tags: [], owner_id: OWNER,
  owner_username: "Parity Modder", owner_handle: "paritymodder", collaborators: [], is_stray: false,
  stray_status: null, author: "", source: null, source_id: null, source_url: null, source_author_id: null,
  source_file_id: null, source_likes: NumberInt(0), uploaded_on_behalf: false, issues_enabled: true,
  issue_seq: NumberInt(0), open_issue_count: NumberInt(0), visibility: "public", is_beta: false, mode: "files",
  source_visibility: "public", default_branch: "main", banner_sha: null, preview_shas: [], discord_url: null,
  website_url: null, donation_urls: [], download_count: NumberInt(0), star_count: NumberInt(0),
  downloads_7d: NumberInt(0), popularity_score: 0.0, hidden_release_branches: [], branch_order: [],
  forked_from_id: null, forked_from_slug: null, forked_from_handle: null, forked_from_title: null,
  forked_from_owner: null, inspired_by_slug: null, inspired_by_handle: null, inspired_by_title: null,
  inspired_by_owner: null, fork_count: NumberInt(0), is_remix: false, remix_status: "pending",
  remix_requested_at: null, remix_resolved_at: null, remix_resolved_by: null, taken_down: false,
  takedown_reason: null, created_at: T0, updated_at: T1, last_release_at: null,
}, o);
const P1 = ObjectId("66f0000000000000000000d1"), P2 = ObjectId("66f0000000000000000000d2"),
      P3 = ObjectId("66f0000000000000000000d3"), P4 = ObjectId("66f0000000000000000000d4"),
      P5 = ObjectId("66f0000000000000000000d5"), R1 = ObjectId("66f0000000000000000000d6"),
      R2 = ObjectId("66f0000000000000000000d7"), S1 = ObjectId("66f0000000000000000000d8");
db.mod_projects.insertMany([
  project({_id: P1, slug: "dragon-pack", title: "Dragon Pack", summary: "Dragons, re-skinned",
    description: "# Dragons\nAll of them.", warnings: "Back up first<br>Beta build",
    title_i18n: {fr: "Pack de dragons"}, tags: ["dragons", "gui", "custom tag"],
    collaborators: [{user_id: COLLAB, username: "paritycollab"}], banner_sha: "b".repeat(64),
    preview_shas: ["c".repeat(64), "d".repeat(64)], discord_url: "https://discord.gg/parity",
    donation_urls: ["https://ko-fi.com/parity"], download_count: NumberInt(10), star_count: NumberInt(1),
    hidden_release_branches: ["alt"], branch_order: ["main", "alt"], fork_count: NumberInt(2), is_beta: true,
    last_release_at: T2, updated_at: T2}),
  project({_id: P2, slug: "secret-draft", title: "Secret Draft", visibility: "draft", tags: ["wings"]}),
  project({_id: P3, slug: "gone", title: "Gone Mod", taken_down: true, takedown_reason: "Spam",
    download_count: NumberInt(99)}),
  project({_id: P4, slug: "unlisted-one", title: "Unlisted One", visibility: "unlisted", mode: "releases",
    tags: ["Radar"], download_count: NumberInt(3), star_count: NumberInt(5), last_release_at: T1}),
  project({_id: P5, slug: "web-only", title: "Web Only", visibility: "public_website_only", tags: ["gui"],
    summary: "website tier"}),
  project({_id: R1, slug: "dragon-remix", title: "Dragon Pack", owner_id: STRANGER, owner_username: "paritystranger",
    owner_handle: "paritystranger", tags: ["dragons"], forked_from_id: P1, forked_from_slug: "dragon-pack",
    forked_from_handle: "paritymodder", forked_from_title: "Dragon Pack", forked_from_owner: "Parity Modder",
    is_remix: true, remix_requested_at: ISODate("2026-01-01T00:00:00Z"), updated_at: T0}),
  project({_id: R2, slug: "fresh-remix", title: "Fresh Remix", owner_id: STRANGER, owner_username: "paritystranger",
    owner_handle: "paritystranger", forked_from_id: P1, forked_from_slug: "dragon-pack",
    forked_from_handle: "paritymodder", forked_from_title: "Dragon Pack", forked_from_owner: "Parity Modder",
    is_remix: true, remix_requested_at: ISODate("2099-01-01T00:00:00Z")}),
  project({_id: S1, slug: "old-stray", title: "Old Stray", owner_id: null, owner_username: "",
    owner_handle: "stray", is_stray: true, stray_status: "approved", author: "Old Author", source: "x",
    source_id: "1", tags: ["fishing"], download_count: NumberInt(7)}),
]);

const release = (o) => Object.assign({
  owner_id: OWNER, branch: "main", title: "", changelog: "", title_i18n: {}, changelog_i18n: {},
  source_commit_sha: null, release_format: "tmod", prior_tmod_shas: [], tmod_size: NumberInt(1000),
  tmod_filename: "mod.tmod", tmod_properties: {}, banner_sha: null, download_count: NumberInt(0),
  status: "published", created_at: T1, updated_at: T1, published_at: T1, announced_at: T1, silent: false,
}, o);
db.mod_releases.insertMany([
  release({_id: ObjectId("66f1000000000000000000e1"), project_id: P1, tag: "v1.0", title: "First",
    tmod_sha: "1".repeat(64), prior_tmod_shas: ["2".repeat(64)], tmod_properties: {title: "Dragon Pack", author: "Parity", modVersion: "1.0"},
    tmod_filename: "dragon-pack-v1.0.tmod", download_count: NumberInt(6)}),
  release({_id: ObjectId("66f1000000000000000000e2"), project_id: P1, tag: "v1.1", title: "Second", changelog: "- fixes",
    tmod_sha: "6".repeat(64), tmod_size: NumberInt(1500), tmod_properties: {title: "  Dragon: Pack?  ", modVersion: "1.1"},
    created_at: T2, updated_at: T2, published_at: T2, announced_at: T2, download_count: NumberInt(4)}),
  release({_id: ObjectId("66f1000000000000000000e3"), project_id: P1, tag: "v0.9", branch: "alt", tmod_sha: "7".repeat(64),
    created_at: T1, published_at: T1, announced_at: null}),
  release({_id: ObjectId("66f1000000000000000000e4"), project_id: P1, tag: "v2.0", status: "draft", tmod_sha: "8".repeat(64),
    published_at: null, announced_at: null, created_at: T2}),
  release({_id: ObjectId("66f1000000000000000000e5"), project_id: P3, tag: "v1", tmod_sha: "3".repeat(64)}),
  release({_id: ObjectId("66f1000000000000000000e6"), project_id: P4, tag: "r1", release_format: "zip",
    tmod_filename: "unlisted-one-r1.zip", tmod_sha: "4".repeat(64)}),
]);
db.mod_taken_down_artifacts.insertOne({_id: ObjectId("66f2000000000000000000f1"), sha256: "5".repeat(64),
  md5: "", project_id: null, title: "Deleted", owner_handle: "someone", created_at: T0});
db.mod_profiles.insertOne({_id: ObjectId("66f3000000000000000000f1"), site_user_id: OWNER, handle: "paritymodder",
  display_name: "", tagline: "Makes dragons", readme: "About me", tagline_i18n: {fr: "Fait des dragons"},
  readme_i18n: {}, avatar_sha: null, banner_sha: null, discord_url: null, website_url: "https://parity.example",
  donation_urls: [], mod_order: ["unlisted-one", "dragon-pack"], featured_slug: "dragon-pack", taken_down: false,
  takedown_reason: null, created_at: T0, updated_at: T1});
db.mod_stars.insertOne({_id: ObjectId("66f4000000000000000000f1"), project_id: P1, site_user_id: COLLAB, created_at: T1});
const recent = new Date(Date.now() - 3600 * 1000);
db.mod_download_events.insertMany([
  {project_id: P1, release_id: ObjectId("66f1000000000000000000e2"), created_at: recent},
  {project_id: P1, release_id: ObjectId("66f1000000000000000000e2"), created_at: recent},
  {project_id: P4, release_id: ObjectId("66f1000000000000000000e6"), created_at: recent},
]);

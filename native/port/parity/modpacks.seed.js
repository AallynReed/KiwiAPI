// Site users signed in with the fixed sandbox JWTs of gen_modpacks_parity_data.py,
// hub mods and releases a pack can reference, and packs in every visibility.
// The hub release's tmod_sha is the upload_hub.bin build: its blob only lands in
// each stack's store when the spec uploads it.
const T0 = ISODate("2026-09-01T12:00:00.250Z"), T1 = ISODate("2026-09-02T08:30:00Z");
const T2 = ISODate("2026-09-03T09:15:42.120Z"), T3 = ISODate("2026-09-04T18:00:00.999Z");
const OWNER = ObjectId("65c100000000000000000001"), COLLAB = ObjectId("65c100000000000000000002");
const OTHER = ObjectId("65c100000000000000000003"), MODDER = ObjectId("65c100000000000000000004");
const HUB_SHA = "b3f353888e329a59b365afd9e14e51db3c292d4b03a7d9b37b437e0e47aeb348";
const CUSTOM_SHA = "ecb4a45c103297d427557de5f12483c24158182b5f9301dda406d2dcde30f50d";

const user = (id, username, display, email) => ({
  _id: id, username, discord_handle: username, notify_email: email, display_name: display,
  is_active: true, is_verified: true, is_deleted: false, discord_id: null, discord_avatar: null,
  token_version: NumberInt(0), claimed_trove_name: null, claimed_trove_display: null, claimed_at: null,
  claim_verified: false, claim_baseline: {}, created_at: T0, updated_at: T0,
});
db.site_users.insertMany([
  user(OWNER, "parityowner", "Parity Owner", "owner@parity.test"),
  user(COLLAB, "paritycollab", null, null),
  user(OTHER, "parityother", "Other Ünïcode", null),
  user(MODDER, "paritymodder", "Parity Modder", null),
]);

const mod = (id, o) => Object.assign({
  _id: id, summary: "", description: "", warnings: "", tags: [], owner_id: MODDER,
  owner_username: "Parity Modder", owner_handle: "paritymodder", collaborators: [], is_stray: false,
  visibility: "public", is_beta: false, mode: "files", default_branch: "main", banner_sha: null,
  preview_shas: [], download_count: NumberInt(0), star_count: NumberInt(0), taken_down: false,
  takedown_reason: null, created_at: T0, updated_at: T0, last_release_at: T1,
}, o);
const M1 = ObjectId("65c200000000000000000001"), M2 = ObjectId("65c200000000000000000002");
const M3 = ObjectId("65c200000000000000000003"), M4 = ObjectId("65c200000000000000000004");
const M5 = ObjectId("65c200000000000000000005"), M6 = ObjectId("65c200000000000000000006");
db.mod_projects.insertMany([
  mod(M1, {slug: "parity-hub-mod", title: "Parity Hub Mod"}),
  mod(M2, {slug: "beta-mod", title: "Zeta Beta Mod", visibility: "unlisted", is_beta: true}),
  mod(M3, {slug: "secret-mod", title: "Secret Mod", visibility: "draft"}),
  mod(M4, {slug: "gone-mod", title: "Gone Mod", taken_down: true, takedown_reason: "dmca"}),
  mod(M5, {slug: "lost-mod", title: "éclair Lost Mod", owner_id: null, owner_username: "", owner_handle: "",
           is_stray: true, stray_status: "approved", author: "Someone"}),
  mod(M6, {slug: "shared-files", title: "Shared Files Mod", visibility: "public_website_only"}),
]);

const rel = (o) => Object.assign({
  owner_id: MODDER, branch: "main", title: "", changelog: "", source_commit_sha: null, release_format: "tmod",
  prior_tmod_shas: [], tmod_size: NumberInt(100), tmod_filename: "mod.tmod", tmod_properties: {},
  banner_sha: null, download_count: NumberInt(0), status: "published", created_at: T0, updated_at: T0,
  published_at: T1, announced_at: null, silent: false,
}, o);
db.mod_releases.insertMany([
  rel({_id: ObjectId("65c300000000000000000001"), project_id: M1, tag: "v1", tmod_sha: HUB_SHA,
       tmod_filename: "Parity Hub Mod.tmod", tmod_properties: {title: "Parity Hub Mod", author: "Modder"},
       published_at: T2}),
  rel({_id: ObjectId("65c300000000000000000002"), project_id: M1, tag: "v0", tmod_sha: "0".repeat(64),
       tmod_properties: {title: "Parity Hub Mod"}, published_at: T1}),
  rel({_id: ObjectId("65c300000000000000000003"), project_id: M1, tag: "v2-zip", release_format: "zip",
       tmod_sha: "1".repeat(64), tmod_filename: "parity-hub-mod-v2-zip.zip", published_at: T3}),
  rel({_id: ObjectId("65c300000000000000000004"), project_id: M1, tag: "v3", status: "draft",
       tmod_sha: "2".repeat(64), published_at: null}),
  rel({_id: ObjectId("65c300000000000000000005"), project_id: M2, tag: "b1", branch: "dev",
       tmod_sha: "3".repeat(64), tmod_properties: {title: "Zeta: Beta?"}, published_at: T1}),
  rel({_id: ObjectId("65c300000000000000000006"), project_id: M5, tag: "s1", owner_id: null,
       tmod_sha: "4".repeat(64), published_at: T1}),
]);

const entry = (o) => Object.assign({
  project_id: null, handle: "", slug: "", title: "", branch: "main", version_locked: false, locked_tag: null,
  custom_sha: null, custom_filename: null, author: "",
}, o);
const pack = (o) => Object.assign({
  summary: "", description: "", warnings: "", tags: [], owner_id: OWNER, owner_username: "Parity Owner",
  owner_handle: "parityowner", collaborators: [], visibility: "public", banner_sha: null, preview_shas: [],
  discord_url: null, website_url: null, donation_urls: [], download_count: NumberInt(0),
  star_count: NumberInt(0), variants: [{name: "default", label: "Default", entries: []}],
  default_variant: "default", taken_down: false, takedown_reason: null, created_at: T0, updated_at: T0,
}, o);
db.modpack_projects.insertMany([
  pack({_id: ObjectId("65c400000000000000000001"), slug: "seeded-public", title: "Seeded Public Pack",
        summary: "All the parity mods", description: "# Read me", warnings: "Back up first<br>Beta inside",
        tags: ["ui", "qol"], collaborators: [{user_id: COLLAB, username: "paritycollab"}],
        banner_sha: "a".repeat(64), preview_shas: ["b".repeat(64), "c".repeat(64)],
        discord_url: "https://discord.gg/parity", donation_urls: ["https://ko-fi.com/parity"],
        download_count: NumberInt(7), star_count: NumberInt(2), updated_at: T3,
        variants: [
          {name: "default", label: "Default", entries: [
            entry({project_id: M1, handle: "oldhandle", slug: "old-slug", title: "Old Title"}),
            entry({project_id: M2, handle: "paritymodder", slug: "beta-mod", title: "Zeta Beta Mod", branch: "dev",
                   version_locked: true, locked_tag: "b1"}),
            entry({project_id: M3, handle: "paritymodder", slug: "secret-mod", title: "Secret Mod"}),
            entry({project_id: ObjectId("65c2000000000000000000ff"), handle: "ghost", slug: "ghost", title: "ghost mod"}),
            entry({custom_sha: CUSTOM_SHA, custom_filename: "Parity Custom.tmod", title: "Parity Custom",
                   author: "Tester"}),
            entry({project_id: M1, handle: "paritymodder", slug: "parity-hub-mod", title: "Parity Hub Mod",
                   branch: "main", version_locked: true, locked_tag: "v2-zip"}),
          ]},
          {name: "lite", label: "", entries: [
            entry({project_id: M1, handle: "paritymodder", slug: "parity-hub-mod", title: "Parity Hub Mod",
                   version_locked: true, locked_tag: "v0"}),
          ]},
        ]}),
  pack({_id: ObjectId("65c400000000000000000002"), slug: "unlisted-pack", title: "unlisted pack",
        visibility: "unlisted", owner_handle: "drifted-old-name", updated_at: T1}),
  pack({_id: ObjectId("65c400000000000000000003"), slug: "draft-pack", title: "Draft Pack", visibility: "draft",
        updated_at: T2}),
  pack({_id: ObjectId("65c400000000000000000004"), slug: "taken-pack", title: "Taken Pack", taken_down: true,
        takedown_reason: "Broken links", updated_at: T0}),
  {_id: ObjectId("65c400000000000000000005"), slug: "legacy", title: "Légacy Pack", owner_id: OTHER,
   owner_username: "Other Ünïcode", visibility: "public", created_at: T1, updated_at: T1},
  pack({_id: ObjectId("65c400000000000000000006"), slug: "collab-pack", title: "Collab Owned", owner_id: OTHER,
        owner_username: "Other Ünïcode", owner_handle: "parityother", tags: ["qol"],
        collaborators: [{user_id: OWNER, username: "parityowner"}], download_count: NumberInt(40),
        updated_at: T2, variants: [{name: "default", label: "Default", entries: [
          entry({project_id: M5, handle: "stray", slug: "lost-mod", title: "éclair Lost Mod"}),
        ]}]}),
]);
db.modpack_stars.insertOne({_id: ObjectId("65c500000000000000000001"), modpack_id: ObjectId("65c400000000000000000001"),
                            site_user_id: COLLAB, created_at: T0});
db.content_reports.insertMany([
  {_id: ObjectId("65c600000000000000000001"), target_type: "modpack", target_id: ObjectId("65c400000000000000000006"),
   target_label: "Collab Owned", target_url: "/modpacks/parityother/collab-pack", reason: "spam", resolved: false,
   created_at: T0},
  {_id: ObjectId("65c600000000000000000002"), target_type: "mod", target_id: ObjectId("65c400000000000000000006"),
   target_label: "x", target_url: "/mods/x/y", reason: "other kind", resolved: false, created_at: T0},
]);

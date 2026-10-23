//! The indexes every Beanie `Document` declares, in `DOCUMENT_MODELS` order,
//! each exactly as pymongo's `IndexModel.document` sends it (option order
//! included). Beanie sends a model's whole list on every boot.

use mongodb::bson::{Document, doc};

pub fn beanie() -> Vec<(&'static str, Vec<Document>)> {
    vec![
        (
            "users",
            vec![
                doc! {"unique": true, "name": "email_1", "key": {"email": 1}},
                doc! {"unique": true, "partialFilterExpression": {"github_id": {"$type": "number"}}, "name": "github_id_1", "key": {"github_id": 1}},
            ],
        ),
        (
            "sessions",
            vec![
                doc! {"unique": true, "name": "refresh_token_hash_1", "key": {"refresh_token_hash": 1}},
                doc! {"name": "user_id_1_last_used_at_-1", "key": {"user_id": 1, "last_used_at": -1}},
            ],
        ),
        (
            "site_users",
            vec![
                doc! {"unique": true, "name": "username_1", "key": {"username": 1}},
                doc! {"unique": true, "partialFilterExpression": {"discord_id": {"$type": "number"}}, "name": "discord_id_1", "key": {"discord_id": 1}},
                doc! {"unique": true, "partialFilterExpression": {"claimed_trove_name": {"$type": "string"}}, "name": "claimed_trove_name_1", "key": {"claimed_trove_name": 1}},
                doc! {"unique": true, "partialFilterExpression": {"creator_token_hash": {"$type": "string"}}, "name": "creator_token_hash_1", "key": {"creator_token_hash": 1}},
            ],
        ),
        (
            "site_sessions",
            vec![
                doc! {"unique": true, "name": "refresh_token_hash_1", "key": {"refresh_token_hash": 1}},
                doc! {"name": "site_user_id_1_last_used_at_-1", "key": {"site_user_id": 1, "last_used_at": -1}},
            ],
        ),
        (
            "username_change_requests",
            vec![
                doc! {"name": "status_1_created_at_-1", "key": {"status": 1, "created_at": -1}},
                doc! {"name": "site_user_id_1_created_at_-1", "key": {"site_user_id": 1, "created_at": -1}},
            ],
        ),
        (
            "api_tokens",
            vec![
                doc! {"unique": true, "name": "hashed_token_1", "key": {"hashed_token": 1}},
                doc! {"name": "user_id_1", "key": {"user_id": 1}},
                doc! {"name": "revoked_1_last_used_at_1", "key": {"revoked": 1, "last_used_at": 1}},
                doc! {"name": "revoked_1_expires_at_1", "key": {"revoked": 1, "expires_at": 1}},
            ],
        ),
        (
            "usage_events",
            vec![
                doc! {"name": "user_id_1_created_at_-1", "key": {"user_id": 1, "created_at": -1}},
                doc! {"name": "token_id_1_created_at_-1", "key": {"token_id": 1, "created_at": -1}},
                doc! {"name": "status_code_1_created_at_-1", "key": {"status_code": 1, "created_at": -1}},
            ],
        ),
        (
            "page_views",
            vec![
                doc! {"name": "path_1_created_at_-1", "key": {"path": 1, "created_at": -1}},
                doc! {"unique": true, "partialFilterExpression": {"pid": {"$type": "string"}}, "name": "pid_1", "key": {"pid": 1}},
            ],
        ),
        ("email_outbox", vec![doc! {"name": "status_1_next_attempt_at_1", "key": {"status": 1, "next_attempt_at": 1}}]),
        (
            "trove_news",
            vec![
                doc! {"unique": true, "name": "url_1", "key": {"url": 1}},
                doc! {"name": "published_at_-1", "key": {"published_at": -1}},
            ],
        ),
        ("feed_cache", vec![doc! {"unique": true, "name": "feed_1", "key": {"feed": 1}}]),
        (
            "trove_events",
            vec![
                doc! {"unique": true, "name": "event_id_1", "key": {"event_id": 1}},
                doc! {"name": "category_1", "key": {"category": 1}},
                doc! {"name": "starts_at_1", "key": {"starts_at": 1}},
                doc! {"name": "ends_at_1", "key": {"ends_at": 1}},
            ],
        ),
        ("delve_rotations", vec![doc! {"unique": true, "name": "week_1", "key": {"week": 1}}]),
        (
            "btt_releases",
            vec![
                doc! {"unique": true, "name": "release_id_1", "key": {"release_id": 1}},
                doc! {"name": "prerelease_1_published_at_-1", "key": {"prerelease": 1, "published_at": -1}},
            ],
        ),
        ("btt_changelogs", vec![doc! {"unique": true, "name": "repo_1", "key": {"repo": 1}}]),
        ("update_branches", vec![doc! {"unique": true, "name": "branch_1", "key": {"branch": 1}}]),
        (
            "update_versions",
            vec![
                doc! {"unique": true, "name": "branch_1_ordinal_1", "key": {"branch": 1, "ordinal": 1}},
                doc! {"name": "branch_1_version_tag_1", "key": {"branch": 1, "version_tag": 1}},
                doc! {"name": "branch_1_status_1", "key": {"branch": 1, "status": 1}},
            ],
        ),
        (
            "update_changes",
            vec![
                doc! {"name": "branch_1_path_1_ordinal_1", "key": {"branch": 1, "path": 1, "ordinal": 1}},
                doc! {"name": "content_sha256_1", "key": {"content_sha256": 1}},
            ],
        ),
        (
            "update_state",
            vec![
                doc! {"unique": true, "name": "branch_1_path_1", "key": {"branch": 1, "path": 1}},
                doc! {"name": "branch_1_archive_1", "key": {"branch": 1, "archive": 1}},
            ],
        ),
        ("update_manifest", vec![doc! {"unique": true, "name": "branch_1_path_1", "key": {"branch": 1, "path": 1}}]),
        (
            "mod_projects",
            vec![
                doc! {"unique": true, "name": "owner_id_1_slug_1", "key": {"owner_id": 1, "slug": 1}},
                doc! {"name": "owner_handle_1_slug_1", "key": {"owner_handle": 1, "slug": 1}},
                doc! {"name": "owner_id_1_updated_at_-1", "key": {"owner_id": 1, "updated_at": -1}},
                doc! {"name": "collaborators.user_id_1", "key": {"collaborators.user_id": 1}},
                doc! {"name": "visibility_1_updated_at_-1", "key": {"visibility": 1, "updated_at": -1}},
                doc! {"name": "visibility_1_last_release_at_-1", "key": {"visibility": 1, "last_release_at": -1}},
                doc! {"name": "visibility_1_download_count_-1", "key": {"visibility": 1, "download_count": -1}},
                doc! {"name": "visibility_1_star_count_-1", "key": {"visibility": 1, "star_count": -1}},
                doc! {"name": "visibility_1_popularity_score_-1", "key": {"visibility": 1, "popularity_score": -1}},
                doc! {"name": "tags_1", "key": {"tags": 1}},
                doc! {"name": "forked_from_id_1", "key": {"forked_from_id": 1}},
                doc! {"name": "forked_from_id_1_is_remix_1", "key": {"forked_from_id": 1, "is_remix": 1}},
                doc! {"unique": true, "partialFilterExpression": {"source": {"$type": "string"}}, "name": "source_1_source_id_1", "key": {"source": 1, "source_id": 1}},
                doc! {"name": "is_stray_1_stray_status_1_updated_at_-1", "key": {"is_stray": 1, "stray_status": 1, "updated_at": -1}},
                doc! {"name": "mod_project_text", "key": {"title": "text", "summary": "text", "tags": "text"}},
            ],
        ),
        (
            "mod_releases",
            vec![
                doc! {"unique": true, "name": "project_id_1_branch_1_tag_1", "key": {"project_id": 1, "branch": 1, "tag": 1}},
                doc! {"name": "project_id_1_published_at_-1", "key": {"project_id": 1, "published_at": -1}},
                doc! {"name": "tmod_sha_1", "key": {"tmod_sha": 1}},
                doc! {"sparse": true, "name": "prior_tmod_shas_1", "key": {"prior_tmod_shas": 1}},
            ],
        ),
        ("mod_images", vec![doc! {"unique": true, "name": "sha_1", "key": {"sha": 1}}]),
        (
            "content_reports",
            vec![
                doc! {"name": "resolved_1_created_at_-1", "key": {"resolved": 1, "created_at": -1}},
                doc! {"name": "target_type_1_target_id_1", "key": {"target_type": 1, "target_id": 1}},
            ],
        ),
        (
            "mod_git_tokens",
            vec![
                doc! {"unique": true, "name": "token_hash_1", "key": {"token_hash": 1}},
                doc! {"name": "site_user_id_1_created_at_-1", "key": {"site_user_id": 1, "created_at": -1}},
            ],
        ),
        (
            "mod_stars",
            vec![
                doc! {"unique": true, "name": "project_id_1_site_user_id_1", "key": {"project_id": 1, "site_user_id": 1}},
                doc! {"name": "site_user_id_1_created_at_-1", "key": {"site_user_id": 1, "created_at": -1}},
            ],
        ),
        (
            "mod_download_events",
            vec![doc! {"name": "project_id_1_created_at_-1", "key": {"project_id": 1, "created_at": -1}}],
        ),
        (
            "mod_profiles",
            vec![
                doc! {"unique": true, "name": "site_user_id_1", "key": {"site_user_id": 1}},
                doc! {"name": "handle_1", "key": {"handle": 1}},
            ],
        ),
        (
            "mod_issues",
            vec![
                doc! {"unique": true, "name": "project_id_1_number_1", "key": {"project_id": 1, "number": 1}},
                doc! {"name": "project_id_1_status_1_last_activity_at_-1", "key": {"project_id": 1, "status": 1, "last_activity_at": -1}},
                doc! {"name": "participant_ids_1_last_activity_at_-1", "key": {"participant_ids": 1, "last_activity_at": -1}},
                doc! {"name": "author_id_1_created_at_-1", "key": {"author_id": 1, "created_at": -1}},
            ],
        ),
        ("mod_issue_events", vec![doc! {"name": "issue_id_1_created_at_1", "key": {"issue_id": 1, "created_at": 1}}]),
        (
            "mod_claim_requests",
            vec![
                doc! {"name": "status_1_created_at_-1", "key": {"status": 1, "created_at": -1}},
                doc! {"name": "project_id_1", "key": {"project_id": 1}},
                doc! {"name": "claimant_id_1_created_at_-1", "key": {"claimant_id": 1, "created_at": -1}},
            ],
        ),
        ("stray_import_state", vec![doc! {"unique": true, "name": "key_1", "key": {"key": 1}}]),
        (
            "mod_remix_claims",
            vec![
                doc! {"name": "status_1_created_at_-1", "key": {"status": 1, "created_at": -1}},
                doc! {"name": "target_id_1", "key": {"target_id": 1}},
                doc! {"name": "claimant_id_1_created_at_-1", "key": {"claimant_id": 1, "created_at": -1}},
            ],
        ),
        (
            "mod_taken_down_artifacts",
            vec![
                doc! {"unique": true, "name": "sha256_1", "key": {"sha256": 1}},
                doc! {"name": "project_id_1", "key": {"project_id": 1}},
            ],
        ),
        (
            "mod_creator_links",
            vec![
                doc! {"unique": true, "name": "site_user_id_1_api_user_id_1", "key": {"site_user_id": 1, "api_user_id": 1}},
                doc! {"name": "api_user_id_1_revoked_1", "key": {"api_user_id": 1, "revoked": 1}},
            ],
        ),
        (
            "modpack_projects",
            vec![
                doc! {"unique": true, "name": "owner_id_1_slug_1", "key": {"owner_id": 1, "slug": 1}},
                doc! {"name": "owner_handle_1_slug_1", "key": {"owner_handle": 1, "slug": 1}},
                doc! {"name": "owner_id_1_updated_at_-1", "key": {"owner_id": 1, "updated_at": -1}},
                doc! {"name": "collaborators.user_id_1", "key": {"collaborators.user_id": 1}},
                doc! {"name": "visibility_1_updated_at_-1", "key": {"visibility": 1, "updated_at": -1}},
                doc! {"name": "visibility_1_download_count_-1", "key": {"visibility": 1, "download_count": -1}},
                doc! {"name": "visibility_1_star_count_-1", "key": {"visibility": 1, "star_count": -1}},
                doc! {"name": "tags_1", "key": {"tags": 1}},
                doc! {"name": "variants.entries.project_id_1", "key": {"variants.entries.project_id": 1}},
                doc! {"name": "modpack_project_text", "key": {"title": "text", "summary": "text", "tags": "text"}},
            ],
        ),
        (
            "modpack_stars",
            vec![
                doc! {"unique": true, "name": "modpack_id_1_site_user_id_1", "key": {"modpack_id": 1, "site_user_id": 1}},
                doc! {"name": "site_user_id_1_created_at_-1", "key": {"site_user_id": 1, "created_at": -1}},
            ],
        ),
        (
            "blueprint_cache",
            vec![
                doc! {"unique": true, "name": "key_1", "key": {"key": 1}},
                doc! {"name": "created_at_1", "key": {"created_at": 1}},
            ],
        ),
        ("market_interest_items", vec![doc! {"unique": true, "name": "name_1", "key": {"name": 1}}]),
        (
            "market_item_categories",
            vec![
                doc! {"unique": true, "name": "name_1", "key": {"name": 1}},
                doc! {"name": "order_1", "key": {"order": 1}},
            ],
        ),
        (
            "store_products",
            vec![
                doc! {"unique": true, "name": "code_1", "key": {"code": 1}},
                doc! {"name": "last_seen_-1", "key": {"last_seen": -1}},
                doc! {"name": "categories_1", "key": {"categories": 1}},
            ],
        ),
        ("store_categories", vec![doc! {"unique": true, "name": "index_1", "key": {"index": 1}}]),
        ("store_state", vec![doc! {"unique": true, "name": "key_1", "key": {"key": 1}}]),
        ("chaos_chest_captures", vec![doc! {"unique": true, "name": "week_anchor_-1", "key": {"week_anchor": -1}}]),
        ("challenge_captures", vec![doc! {"unique": true, "name": "window_anchor_-1", "key": {"window_anchor": -1}}]),
        ("luxion_appearances", vec![doc! {"unique": true, "name": "started_at_-1", "key": {"started_at": -1}}]),
        ("feedback_entries", vec![doc! {"name": "read_1_created_at_-1", "key": {"read": 1, "created_at": -1}}]),
        ("trove_status_events", vec![doc! {"name": "env_1_started_at_-1", "key": {"env": 1, "started_at": -1}}]),
        ("runtime_config", vec![doc! {"unique": true, "name": "key_1", "key": {"key": 1}}]),
        (
            "ingest_log",
            vec![
                doc! {"expireAfterSeconds": 2592000, "name": "timestamp_1", "key": {"timestamp": 1}},
                doc! {"name": "endpoint_1_timestamp_-1", "key": {"endpoint": 1, "timestamp": -1}},
            ],
        ),
        (
            "decoder_runs",
            vec![
                doc! {"name": "name_1_started_at_-1", "key": {"name": 1, "started_at": -1}},
                doc! {"expireAfterSeconds": 7776000, "name": "started_at_1", "key": {"started_at": 1}},
            ],
        ),
        ("vault_items", vec![doc! {"name": "created_at_-1", "key": {"created_at": -1}}]),
        (
            "giveaways",
            vec![
                doc! {"name": "status_1_ends_at_1", "key": {"status": 1, "ends_at": 1}},
                doc! {"name": "starts_at_-1", "key": {"starts_at": -1}},
            ],
        ),
        (
            "giveaway_entries",
            vec![
                doc! {"unique": true, "name": "giveaway_id_1_site_user_id_1", "key": {"giveaway_id": 1, "site_user_id": 1}},
            ],
        ),
        (
            "prize_codes",
            vec![
                doc! {"name": "vault_item_id_1_status_1", "key": {"vault_item_id": 1, "status": 1}},
                doc! {"name": "giveaway_id_1", "key": {"giveaway_id": 1}},
            ],
        ),
        (
            "supporters",
            vec![
                doc! {"unique": true, "name": "name_1", "key": {"name": 1}},
                doc! {"name": "created_at_1", "key": {"created_at": 1}},
            ],
        ),
        ("guild_configs", vec![doc! {"unique": true, "name": "guild_id_1", "key": {"guild_id": 1}}]),
        (
            "tracked_announcements",
            vec![
                doc! {"name": "guild_id_1_kind_1", "key": {"guild_id": 1, "kind": 1}},
                doc! {"name": "expires_at_1", "key": {"expires_at": 1}},
            ],
        ),
        (
            "clubs",
            vec![doc! {"name": "guild_id_1", "key": {"guild_id": 1}}, doc! {"name": "public_1", "key": {"public": 1}}],
        ),
        (
            "site_webhooks",
            vec![
                doc! {"name": "owner_id_1_created_at_-1", "key": {"owner_id": 1, "created_at": -1}},
                doc! {"name": "active_1", "key": {"active": 1}},
            ],
        ),
        (
            "dm_subscriptions",
            vec![
                doc! {"name": "owner_id_1_created_at_-1", "key": {"owner_id": 1, "created_at": -1}},
                doc! {"name": "active_1", "key": {"active": 1}},
            ],
        ),
        ("image_designs", vec![doc! {"name": "owner_id_1_created_at_-1", "key": {"owner_id": 1, "created_at": -1}}]),
        (
            "file_drops",
            vec![
                doc! {"unique": true, "name": "slug_1", "key": {"slug": 1}},
                doc! {"name": "created_at_-1", "key": {"created_at": -1}},
            ],
        ),
        ("drop_uploads", vec![doc! {"name": "drop_id_1_uploaded_at_-1", "key": {"drop_id": 1, "uploaded_at": -1}}]),
        (
            "art_requests",
            vec![
                doc! {"name": "status_1_created_at_-1", "key": {"status": 1, "created_at": -1}},
                doc! {"name": "batch_1", "key": {"batch": 1}},
            ],
        ),
        ("mod_stats_snapshots", vec![doc! {"unique": true, "name": "day_1", "key": {"day": 1}}]),
        ("btt_mod_blocklist", vec![doc! {"unique": true, "name": "kind_1_value_1", "key": {"kind": 1, "value": 1}}]),
        (
            "steam_workshop_items",
            vec![
                doc! {"unique": true, "name": "published_file_id_1", "key": {"published_file_id": 1}},
                doc! {"name": "subscriptions_-1", "key": {"subscriptions": -1}},
            ],
        ),
    ]
}

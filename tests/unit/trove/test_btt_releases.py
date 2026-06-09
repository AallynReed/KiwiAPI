"""Pure tests for the BTT releases relay: platform detection (extension priority),
the walk-back logic that finds the latest release with a matching asset, and the
GitHub-payload normalization. The fetch + Mongo storage are integration-tested.
"""

from datetime import datetime, timezone

from app.trove import btt_releases as btt

UTC = timezone.utc


def _asset(name: str, **extra) -> dict:
    return {"name": name, "url": f"https://x/{name}",
            "size": 1, "content_type": None, "download_count": 0, **extra}


def _release(tag: str, assets: list[dict], prerelease: bool = False) -> dict:
    return {"tag_name": tag, "assets": assets, "prerelease": prerelease}


# --- platform detection -----------------------------------------------------

def test_asset_priority_matches_per_platform():
    assert btt._asset_priority("BTT-1.0.0.msi", "windows") == 0  # msi wins
    assert btt._asset_priority("BTT-1.0.0.exe", "windows") == 1  # exe is second
    assert btt._asset_priority("BTT-1.0.0.AppImage", "linux") == 0
    assert btt._asset_priority("BTT-1.0.0.deb", "linux") == 1
    assert btt._asset_priority("BTT-1.0.0.apk", "android") == 0
    assert btt._asset_priority("BTT-1.0.0.msi", "linux") is None
    assert btt._asset_priority("README", "windows") is None
    # Case-insensitive matching (uploads vary).
    assert btt._asset_priority("BTT.MSI", "windows") == 0


def test_assets_for_platform_filters_and_sorts_by_priority():
    assets = [
        _asset("BTT-1.0.0.exe"),
        _asset("BTT-1.0.0.msi"),
        _asset("BTT-1.0.0.deb"),
        _asset("BTT-1.0.0.apk"),
        _asset("README.txt"),
    ]
    windows = btt.assets_for_platform(assets, "windows")
    assert [a["name"] for a in windows] == ["BTT-1.0.0.msi", "BTT-1.0.0.exe"]
    assert btt.assets_for_platform(assets, "linux")[0]["name"] == "BTT-1.0.0.deb"
    assert [a["name"] for a in btt.assets_for_platform(assets, "android")] == ["BTT-1.0.0.apk"]
    # No matching asset for the unknown-extension list -> empty list.
    assert btt.assets_for_platform([_asset("README.txt")], "windows") == []


# --- walk-back: newest release without the platform's asset is skipped ------

def test_walk_latest_finds_newest_with_matching_asset():
    releases = [
        _release("v3", [_asset("BTT-3.apk"), _asset("BTT-3.deb")]),  # no windows
        _release("v2", [_asset("BTT-2.msi"), _asset("BTT-2.deb")]),  # windows here!
        _release("v1", [_asset("BTT-1.exe")]),
    ]
    found = btt.walk_latest(releases, "windows")
    assert found is not None
    release, assets = found
    assert release["tag_name"] == "v2"
    assert [a["name"] for a in assets] == ["BTT-2.msi"]


def test_walk_latest_returns_none_when_no_platform_match():
    releases = [_release("v1", [_asset("README.txt")])]
    assert btt.walk_latest(releases, "android") is None


def test_walk_latest_skips_release_with_empty_assets():
    # Common race: a fresh tag is published but the CI build hasn't uploaded its
    # binaries yet, so the latest release has assets=[]. Walk back to the
    # previous release that actually shipped a build. (This is exactly what the
    # "go to second-latest when latest has no binaries" UX needs.)
    releases = [
        _release("v3", []),                                  # latest, tag-only, no binaries yet
        _release("v2", [_asset("BTT-2.msi"), _asset("BTT-2.apk")]),
        _release("v1", [_asset("BTT-1.msi")]),
    ]
    for platform in ("windows", "android"):
        release, _assets = btt.walk_latest(releases, platform)
        assert release["tag_name"] == "v2", f"{platform} should walk back to v2"
    # Linux had no asset on either v3 OR v2, so it walks to v1 - wait, v1
    # ships .msi, not Linux. So Linux walks past all three and returns None.
    assert btt.walk_latest(releases, "linux") is None


def test_walk_latest_uses_newest_when_it_has_the_platform():
    releases = [
        _release("v3", [_asset("BTT-3.msi"), _asset("BTT-3.exe")]),
        _release("v2", [_asset("BTT-2.msi")]),
    ]
    release, assets = btt.walk_latest(releases, "windows")
    assert release["tag_name"] == "v3"
    # Both msi and exe are matched, msi sorts first.
    assert [a["name"] for a in assets] == ["BTT-3.msi", "BTT-3.exe"]


# --- GitHub payload normalization -------------------------------------------

def _gh(**overrides) -> dict:
    base = {
        "id": 100, "tag_name": "v1.0.0", "name": "Release 1",
        "body": "Notes", "html_url": "https://github.com/x/y/releases/tag/v1.0.0",
        "prerelease": False, "draft": False,
        "published_at": "2026-06-05T12:00:00Z",
        "assets": [{"name": "BTT.msi", "browser_download_url": "https://dl/msi",
                    "size": 1234, "content_type": "application/octet-stream",
                    "download_count": 7}],
    }
    base.update(overrides)
    return base


def test_normalize_release_keeps_required_fields():
    n = btt.normalize_release(_gh())
    assert n["release_id"] == 100
    assert n["tag_name"] == "v1.0.0"
    assert n["prerelease"] is False
    assert n["published_at"] == datetime(2026, 6, 5, 12, 0, tzinfo=UTC)
    assert n["assets"][0]["url"] == "https://dl/msi" and n["assets"][0]["size"] == 1234


def test_normalize_release_skips_drafts_and_invalid():
    assert btt.normalize_release(_gh(draft=True)) is None
    assert btt.normalize_release({"id": 1}) is None             # missing required
    assert btt.normalize_release(_gh(published_at=None)) is None
    assert btt.normalize_release("not a dict") is None


# --- version comparison (drives the /check endpoint) -----------------------

def test_parse_version_handles_common_shapes():
    assert btt.parse_version("v1.2.3") == ((1, 2, 3), "")
    assert btt.parse_version("1.2.3") == ((1, 2, 3), "")
    assert btt.parse_version("v1.2.3-beta.1") == ((1, 2, 3), "-beta.1")
    assert btt.parse_version("  v1.0  ") == ((1, 0), "")
    assert btt.parse_version("nightly") is None
    assert btt.parse_version("") is None
    assert btt.parse_version("v") is None


def test_compare_versions_numeric_ordering():
    assert btt.compare_versions("v1.2.3", "v1.2.4") == -1
    assert btt.compare_versions("v2.0.0", "v1.99.99") == 1
    assert btt.compare_versions("v1.2.3", "v1.2.3") == 0
    # Trailing zeros don't change ordering (v1.2 == v1.2.0).
    assert btt.compare_versions("v1.2", "v1.2.0") == 0
    assert btt.compare_versions("v1.2", "v1.2.1") == -1


def test_compare_versions_release_outranks_prerelease():
    # Same numeric core: a release ("") is GREATER than a prerelease.
    assert btt.compare_versions("v1.2.3", "v1.2.3-beta.1") == 1
    assert btt.compare_versions("v1.2.3-beta.1", "v1.2.3") == -1
    # Among prereleases, compare suffix strings (rough but predictable).
    assert btt.compare_versions("v1.2.3-beta.1", "v1.2.3-beta.2") == -1


def test_compare_versions_returns_none_on_garbage():
    assert btt.compare_versions("v1.2.3", "nightly") is None
    assert btt.compare_versions("rolling", "v1.2.3") is None
    assert btt.compare_versions("", "v1.0.0") is None


# --- changelog: conventional-commit parse + tag grouping --------------------

def test_parse_conventional_prefix():
    assert btt.parse_conventional_prefix("feat: add update check") == "feat"
    assert btt.parse_conventional_prefix("fix(api): handle nulls") == "fix"
    assert btt.parse_conventional_prefix("Feat: capitalized") == "feat"  # lowercased
    assert btt.parse_conventional_prefix("docs(readme): tweak") == "docs"
    assert btt.parse_conventional_prefix("Merge pull request #42 from x") is None
    assert btt.parse_conventional_prefix("") is None
    assert btt.parse_conventional_prefix("just a sentence") is None


def _commit(sha: str, message: str) -> dict:
    return {"sha": sha, "commit": {"message": message},
            "html_url": f"https://github.com/x/y/commit/{sha}"}


def _tag(name: str, sha: str) -> dict:
    return {"name": name, "commit": {"sha": sha}}


def test_build_changelog_groups_unreleased_then_tags():
    tags = [_tag("v1.1.0", "bbbbbbb"), _tag("v1.0.0", "ddddddd")]
    commits = [
        _commit("aaaaaaa", "feat: post-v1.1 work"),  # since v1.1 -> Unreleased
        _commit("bbbbbbb", "release v1.1.0"),         # tagged v1.1.0 - starts v1.1.0 group
        _commit("ccccccc", "fix: a bug fixed in 1.1"),
        _commit("ddddddd", "v1.0.0 release"),         # tagged v1.0.0 - starts v1.0.0 group
        _commit("eeeeeee", "initial commit"),
    ]
    groups = btt.build_changelog_groups(tags, commits)
    assert [g["version"] for g in groups] == ["Unreleased", "v1.1.0", "v1.0.0"]
    assert [c["short_sha"] for c in groups[0]["commits"]] == ["aaaaaaa"]
    assert [c["short_sha"] for c in groups[1]["commits"]] == ["bbbbbbb", "ccccccc"]
    assert [c["short_sha"] for c in groups[2]["commits"]] == ["ddddddd", "eeeeeee"]
    # Conventional-commit type passed through.
    assert groups[0]["commits"][0]["type"] == "feat"
    assert groups[1]["commits"][1]["type"] == "fix"
    assert groups[2]["commits"][1]["type"] is None  # "initial commit"


def test_build_changelog_groups_drops_empty_groups():
    # No commits since the latest tag - the "Unreleased" group should NOT appear.
    tags = [_tag("v1.0.0", "aaaaaaa")]
    commits = [_commit("aaaaaaa", "release v1.0.0"), _commit("bbbbbbb", "feat: start")]
    groups = btt.build_changelog_groups(tags, commits)
    assert [g["version"] for g in groups] == ["v1.0.0"]
    assert len(groups[0]["commits"]) == 2


def test_build_changelog_groups_message_first_line_only():
    commits = [_commit("aaaaaaa", "feat: headline\n\nlong body\nmore body")]
    groups = btt.build_changelog_groups([], commits)
    assert groups[0]["commits"][0]["message"] == "feat: headline"


def test_build_changelog_groups_defensive_on_garbage():
    # Junk shapes mixed in shouldn't crash; they're just skipped.
    assert btt.build_changelog_groups(None, None) == []
    assert btt.build_changelog_groups([{"junk": 1}], [{"missing": "sha"}]) == []
    assert btt.build_changelog_groups([], ["not a dict"]) == []


def test_normalize_release_drops_malformed_assets():
    payload = _gh(assets=[
        {"name": "ok.msi", "browser_download_url": "https://dl/ok"},
        {"name": "", "browser_download_url": "https://dl/empty"},      # empty name
        {"name": "no-url.msi"},                                         # no url
        "not a dict",                                                   # type mismatch
    ])
    n = btt.normalize_release(payload)
    assert [a["name"] for a in n["assets"]] == ["ok.msi"]

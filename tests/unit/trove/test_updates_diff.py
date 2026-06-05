from app.trove.updates.diff import (
    archive_index_of,
    classify_manifest_diff,
    diff_logical,
    join_logical,
    parent_dir,
)


def test_classify_manifest_diff():
    old = {
        "Trove.exe": {"sha1": "a", "size": 1},
        "ui/index.tfi": {"sha1": "t", "size": 1},
        "ui/archive0.tfa": {"sha1": "x", "size": 1},
    }
    new = [
        {"path": "Trove.exe", "sha1": "a", "size": 1},          # unchanged
        {"path": "ui/index.tfi", "sha1": "T2", "size": 2},      # changed
        {"path": "ui/archive0.tfa", "sha1": "X2", "size": 2},   # changed
        {"path": "NewLoose.dll", "sha1": "n", "size": 3},       # added loose
    ]
    plan = classify_manifest_diff(old, new)
    assert plan["any"] is True
    assert plan["changed_loose"] == ["NewLoose.dll"]
    assert plan["removed_loose"] == []
    assert plan["dir_work"]["ui"]["tfi_path"] == "ui/index.tfi"
    assert plan["dir_work"]["ui"]["changed_archives"] == [0]


def test_classify_no_change():
    old = {"a.exe": {"sha1": "x", "size": 1}}
    plan = classify_manifest_diff(old, [{"path": "a.exe", "sha1": "x", "size": 1}])
    assert plan["any"] is False
    assert plan["dir_work"] == {}


def test_classify_dir_removed():
    old = {"ui/index.tfi": {"sha1": "t", "size": 1}, "ui/archive0.tfa": {"sha1": "x", "size": 1}}
    plan = classify_manifest_diff(old, [])  # whole manifest gone
    assert "ui" in plan["dir_work"]
    assert plan["dir_work"]["ui"]["tfi_path"] is None  # signals "directory removed"


def test_diff_logical():
    old = {"a": 1, "b": 2, "c": 3}
    new = {"a": 1, "b": 99, "d": 4}
    d = diff_logical(old, new)
    assert d["added"] == ["d"] and d["modified"] == ["b"] and d["removed"] == ["c"]


def test_path_helpers():
    assert archive_index_of("ui/archive12.tfa") == 12
    assert archive_index_of("ui/index.tfi") is None
    assert parent_dir("ui/sub/x.bin") == "ui/sub"
    assert parent_dir("Trove.exe") == ""
    assert join_logical("ui", "a/b.bin") == "ui/a/b.bin"
    assert join_logical("", "x") == "x"

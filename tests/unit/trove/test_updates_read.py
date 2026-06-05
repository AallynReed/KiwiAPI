from app.trove.updates.read import directory_listing

_TREE = [
    {"path": "Trove_x64.exe", "size": 100},
    {"path": "prefabs/collections/pet/wolf.binfab", "size": 10},
    {"path": "prefabs/collections/pet/cat.binfab", "size": 20},
    {"path": "prefabs/collections/mount/horse.binfab", "size": 30},
    {"path": "prefabs/item/sword.binfab", "size": 40},
    {"path": "ui/icon.png", "size": 5},
]


def test_root_listing():
    out = directory_listing(_TREE, "")
    names = [(e["name"], e["is_dir"]) for e in out]
    # Directories first (prefabs, ui), then files (Trove_x64.exe), each alphabetical.
    assert names == [("prefabs", True), ("ui", True), ("Trove_x64.exe", False)]
    prefabs = next(e for e in out if e["name"] == "prefabs")
    assert prefabs["path"] == "prefabs/" and prefabs["file_count"] == 4 and prefabs["size"] == 100


def test_subdirectory_listing():
    out = directory_listing(_TREE, "prefabs/collections/")
    by = {e["name"]: e for e in out}
    assert set(by) == {"pet", "mount"}
    assert by["pet"]["is_dir"] and by["pet"]["file_count"] == 2 and by["pet"]["size"] == 30
    assert by["pet"]["path"] == "prefabs/collections/pet/"


def test_leaf_directory_lists_files():
    out = directory_listing(_TREE, "prefabs/collections/pet/")
    assert [(e["name"], e["is_dir"], e["size"]) for e in out] == [
        ("cat.binfab", False, 20), ("wolf.binfab", False, 10),
    ]
    assert out[0]["path"] == "prefabs/collections/pet/cat.binfab"


def test_unknown_prefix_is_empty():
    assert directory_listing(_TREE, "nope/") == []

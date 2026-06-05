import pytest

from app.trove.updates import cdn

_POINTER = (
    "STABLE-103-418-A-335805|content/patchkiwi-live-us01|"
    "May 21 15:16, STABLE-103-418-A-335805|"
    "http://alpha.triongames.com/installer/trove/trove-alpha-motd.html|"
    "kiwi_STABLE-103-418-A-335805_Patch|*"
)

_MANIFEST = (
    "version STABLE-103-415-A-335760\r\n"
    "CrashHandler.exe:535853c2de3cc8da34f376c53850f66d00232723:2376192\r\n"
    "Trove_x64.exe:c7483e6f11530c393d138d76cb10392537faa16f:21809664\r\n"
    "audio\\archive0.tfa:84f78fad45833a3735d15bff5f66bb19f5eb79e1:4039807\r\n"
    "ui\\archive0.tfa:b5b7875b6fb3925e6b0c4a9644efacf31cb4accd:4194655\r\n"
)


def test_parse_pointer():
    p = cdn.parse_pointer(_POINTER)
    assert p["version"] == "STABLE-103-418-A-335805"
    # The "content/" prefix is stripped so the URL templates don't double it.
    assert p["content_path"] == "patchkiwi-live-us01"
    assert p["motd"].endswith("trove-alpha-motd.html")


def test_parse_pointer_rejects_garbage():
    with pytest.raises(cdn.CdnError):
        cdn.parse_pointer("")
    with pytest.raises(cdn.CdnError):
        cdn.parse_pointer("only-one-field")


def test_parse_manifest():
    version, entries = cdn.parse_manifest(_MANIFEST)
    assert version == "STABLE-103-415-A-335760"
    assert len(entries) == 4
    by_path = {e["path"]: e for e in entries}
    assert "ui/archive0.tfa" in by_path  # backslash -> posix
    assert by_path["ui/archive0.tfa"]["sha1"] == "b5b7875b6fb3925e6b0c4a9644efacf31cb4accd"
    assert by_path["ui/archive0.tfa"]["size"] == 4194655
    assert by_path["Trove_x64.exe"]["size"] == 21809664


def test_parse_manifest_skips_bad_lines_keeps_good():
    text = "version V1\r\nGood.exe:abc:123\r\ngarbage-no-colons\r\nBad.exe:def:notanint\r\n"
    version, entries = cdn.parse_manifest(text)
    assert version == "V1"
    assert [e["path"] for e in entries] == ["Good.exe"]  # only the valid line survives


def test_parse_manifest_requires_version_line():
    with pytest.raises(cdn.CdnError):
        cdn.parse_manifest("Trove_x64.exe:abc:1\r\n")


def test_url_builders_reproduce_double_slash():
    base, prefix = "http://trove-update.dyn.triongames.com", "/kiwi-live-client-patch/"
    assert cdn.pointer_url(base, prefix, "kiwi-live-us.txt") == (
        "http://trove-update.dyn.triongames.com/kiwi-live-client-patch//public/kiwi-live-us.txt"
    )
    assert cdn.manifest_url(base, prefix, "patchkiwi-live-us01", "STABLE-1") == (
        "http://trove-update.dyn.triongames.com/kiwi-live-client-patch/"
        "/content/patchkiwi-live-us01/STABLE-1.manifest"
    )
    url = cdn.file_url(base, prefix, "patchkiwi-live-us01", "ui/archive0.tfa", "deadbeef")
    assert url.endswith("/content/patchkiwi-live-us01/recovery/ui/archive0.tfa?sha1=deadbeef")


def test_branches_map():
    assert cdn.BRANCHES == {"live-us": "kiwi-live-us.txt", "pts": "kiwi-pts.txt"}

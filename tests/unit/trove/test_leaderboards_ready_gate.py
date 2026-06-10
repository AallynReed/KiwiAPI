"""Unit tests for the published-anchor gate that holds the leaderboards page
at the latest FULLY-PROCESSED capture.

``_cap_to_ready`` drops anchors newer than the ``ready`` pointer, so a freshly
ingested capture stays hidden until the warmer publishes it (set_ready_anchor)
- the page switches to a new snapshot only once entries + cheaters + activity
are all cached for it.
"""
from __future__ import annotations

from app.trove.leaderboards.cache import _cap_to_ready


def test_no_ready_pointer_passes_through():
    # No Redis / before the first publish -> raw list, unchanged.
    assert _cap_to_ready([30, 20, 10], None) == [30, 20, 10]


def test_caps_to_ready():
    # 30 isn't published yet -> hidden; page stays at 20.
    assert _cap_to_ready([30, 20, 10], 20) == [20, 10]


def test_exact_ready_is_included():
    assert _cap_to_ready([30, 20, 10], 30) == [30, 20, 10]


def test_ready_newer_than_all_keeps_all():
    assert _cap_to_ready([30, 20, 10], 99) == [30, 20, 10]


def test_ready_older_than_all_empties():
    assert _cap_to_ready([30, 20, 10], 5) == []

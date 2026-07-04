"""Styles: equipment appearances (hats / faces / hair / weapons / banners).

Verified against the live archive: styles are the ``equipment/`` appearance prefabs
(``equipment/adventure/helm_clubits_01``, ``equipment/banner/.../banner_*``, …) - the
catalogue is ``collections/collection_equipmentappearance.binfab``. Each prefab carries
an identity component (name/category="Equipment"/description) and names the model
blueprint; the prefab path itself is the stable equipment id.

Mastery is the documented EquipmentAppearance base (1) unless a ``meta/multipliers.binfab``
row scales it - resolved through the same authoritative mastery path the rest of the
codex uses (``mastery.mastery_for`` with the ``equipment/`` base), so an unbacked style
still shows its base, never a guessed value. Geode mastery stays opt-in (None unless the
style is listed in ``geode_multipliers.binfab``).

The equipment SLOT (Hat/Face/Weapon/Banner) isn't reliably encoded in the path, so
``style_family`` is best-effort over the stem tokens and degrades to "" (the entry still
shows, just ungrouped). Pure + stdlib-only.
"""

from __future__ import annotations

STYLE_ROOT = "equipment/"          # logical path prefix under prefabs/ (verified)

# Stem token -> display family. Scanned over the lowercased stem; first hit wins. The
# weapon tokens cover the in-game weapon style classes (sword/gun/staff/bow/…).
_FAMILY_TOKENS: tuple[tuple[str, str], ...] = (
    ("banner", "Banner"),
    ("helm", "Hat"),
    ("hat", "Hat"),
    ("face", "Face"),
    ("hair", "Hair"),
    ("mask", "Face"),
    ("weapon", "Weapon"),
    ("sword", "Weapon"),
    ("staff", "Weapon"),
    ("bow", "Weapon"),
    ("gun", "Weapon"),
    ("pistol", "Weapon"),
    ("spear", "Weapon"),
    ("fist", "Weapon"),
    ("axe", "Weapon"),
    ("lance", "Weapon"),
)


def equipment_id(rel: str) -> str:
    """The style's stable equipment id - its prefab stem (no dir, no .binfab)."""
    return rel.replace("\\", "/").rsplit("/", 1)[-1].removesuffix(".binfab")


def style_family(rel: str) -> str:
    """Best-effort equipment slot family from the stem (Hat/Face/Weapon/Banner), or ""."""
    stem = equipment_id(rel).lower()
    for token, label in _FAMILY_TOKENS:
        if token in stem:
            return label
    return ""


def style_identity(rel: str) -> dict:
    """The preserved identity for a style: the prefab path is both the source id and the
    resolved equipment id (no alias to reconcile - the appearance prefab IS the id)."""
    eid = equipment_id(rel)
    return {
        "source_id": rel,
        "equipment_ref": eid,
        "resolved_id": rel,
        "confidence": "high",
    }

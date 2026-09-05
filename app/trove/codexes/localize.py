"""Resolve the `$…` localization keys the bonus extractors leave behind.

The decoders store raw keys (`$Stat_MovementSpeed`, `$EquipmentSlot_Mount`, the
ability `$prefabs_…_description`). The real display text lives in the archived
`languages/<locale>/` string tables we already merge into the locale map, so we
look the key up there first and fall back to a built-in English label (ported from
BTT) when a table doesn't carry it. Pure - the locale map is passed in.
"""

from __future__ import annotations

from app.trove.codexes.binfab import clean_localized_text

# `$Stat_…` key -> built-in fallback label (the in-game display names; BTT's table).
STAT_NAMES: dict[str, str] = {
    "$Stat_PhysicalDamage": "Physical Damage",
    "$Stat_SpellDamage": "Magic Damage",
    "$Stat_MaxHealth": "Maximum Health",
    "$Stat_MaxEnergy": "Maximum Energy",
    "$Stat_HealthRegen_controller": "Health Regen",
    "$Stat_EnergyRegen_controller": "Energy Regen",
    "$Stat_Stability": "Stability",
    "$Stat_CriticalHitChance": "Critical Hit",
    "$Stat_MovementSpeed": "Movement Speed",
    "$Stat_Jump": "Jump",
    "$Stat_Superstition": "Superstition",
    "$Stat_IncomingDamageMod": "Incoming Damage",
    "$Stat_OutgoingDamageMod": "Damage",
    "$Stat_MagicFind": "Magic Find",
    "$Stat_Mining": "Lasermancy",
    "$Stat_AttackSpeed": "Attack Speed",
    "$Stat_MaxFlasks": "Flask Capacity",
    "$Stat_CraftingSpeed": "Crafting Speed",
    "$Stat_CooldownSpeed": "Cooldown Speed",
    "$Stat_Acceleration": "Acceleration",
    "$Stat_TurningRate": "Turning Rate",
    "$Stat_ExperienceBoost": "Experience Boost",
    "$Stat_CriticalHitDamage": "Critical Damage",
    "$Stat_BattleFactor": "Battle Factor",
    "$Stat_ActionTimeMod": "Action Time Mod",
    "$Stat_PowerRank": "Power Rank",
    "$Stat_Glide": "Glide",
    "$Stat_ShootProjectileSpeedMultiplier": "Projectile Speed",
    "$Stat_RedGemStatBoost": "Red Gem Stat Boost",
    "$Stat_BlueGemStatBoost": "Blue Gem Stat Boost",
    "$Stat_YellowGemStatBoost": "Yellow Gem Stat Boost",
    "$Stat_DoubleHitChance": "Double Hit",
    "$Stat_JackpotExperience": "Jackpot Experience",
    "$Stat_GemEfficiency": "Gem Efficiency",
    "$Stat_AdventurineGainBoost": "Adventurine",
    "$Stat_ClubExperienceBoost": "Club Experience Boost",
    "$Stat_MaxExploration": "Maximum Exploration",
    "$Stat_JumpSpeedMultiplier": "Jump Speed",
    "$Stat_GreenGemStatBoost": "Green Gem Stat Boost",
    "$Stat_Light": "Light",
    "$Stat_Dark": "Dark",
    "$Stat_OpalGemStatBoost": "Opal Gem Stat Boost",
    "$Stat_MaxNCharge": "Maximum N-Charge",
    "$Stat_NChargeRegen": "N-Charge Regen",
    "$Stat_MaxArmor": "Maximum Armor",
}

SLOT_NAMES: dict[str, str] = {
    "$EquipmentSlot_Mount": "Mount",
    "$EquipmentSlot_Wings": "Wings",
    "$EquipmentSlot_Boat": "Boat",
}


def _from_loc(loc_map: dict[str, str], key: str | None) -> str:
    if not key:
        return ""
    text = loc_map.get(key)
    return clean_localized_text(text) if text else ""


def _humanize(key: str) -> str:
    """Last-ditch readable label from a `$Stat_…`/`$EquipmentSlot_…` key."""
    body = str(key or "").lstrip("$")
    body = body.split("_", 1)[-1] if "_" in body else body   # drop the Stat_/EquipmentSlot_ prefix
    body = body.replace("_controller", "")
    # split camelCase -> words
    out = []
    for i, ch in enumerate(body):
        if ch.isupper() and i and not body[i - 1].isupper():
            out.append(" ")
        out.append(ch)
    return "".join(out).replace("_", " ").strip().title()


def _bonus_base(key: str) -> str | None:
    """The stat a `…Bonus` key is a bonus ON, or None if this isn't one. Guarded on the
    base being a stat we know, so a real key that happened to end in `Bonus` is safe."""
    base, sep, tail = str(key or "").partition("_controller")
    if not base.endswith("Bonus"):
        return None
    base = base[:-len("Bonus")] + sep + tail
    return base if base in STAT_NAMES else None


def resolve_stat_name(loc_map: dict[str, str], key: str) -> str:
    """Display name for a `$Stat_…` key: locale table, then built-in, then derived.

    A `MultiplySum` record carries the `…Bonus` twin of its stat key, which the game
    names nowhere - so it takes the stat's own (localized) name plus "Bonus" rather
    than a parallel table that would need an entry per stat and stay English anyway."""
    base = _bonus_base(key)
    if base is not None:
        return f"{resolve_stat_name(loc_map, base)} Bonus"
    return _from_loc(loc_map, key) or STAT_NAMES.get(key) or _humanize(key)


def bonus_label(name: str, stat_key: str) -> str:
    """A site label with the `Bonus` suffix a MultiplySum earns.

    `resolve_stat_name` gives the game's own name for the whole key, which would
    also replace the friendlier names the site's data files use - it calls
    `$Stat_IncomingDamageMod` "Incoming Damage" where the site says "Damage
    Reduction", and `$Stat_OutgoingDamageMod` just "Damage". So the base name is
    kept and only the suffix is taken, which is the part that carries meaning:
    an `Add` and a `MultiplySum` on one stat are different things.

    A trailing "%" goes first - "Maximum Health %" was the site marking the bonus
    form by hand, and "Maximum Health % Bonus" says it twice.
    """
    if _bonus_base(stat_key) is None:
        return name
    base = name.removesuffix(" %").strip()
    return base if base.endswith(" Bonus") else f"{base} Bonus"


def resolve_slot_name(loc_map: dict[str, str], key: str | None) -> str:
    if not key:
        return ""
    return _from_loc(loc_map, key) or SLOT_NAMES.get(key) or _humanize(key)


def resolve_text(loc_map: dict[str, str], key: str | None) -> str:
    """Plain locale lookup (e.g. an ability `$prefabs_…_description`). '' if absent."""
    return _from_loc(loc_map, key)

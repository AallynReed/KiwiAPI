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
    "$Stat_CriticalHitDamageBonus": "Critical Damage Bonus",
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


def resolve_stat_name(loc_map: dict[str, str], key: str) -> str:
    """Display name for a `$Stat_…` key: locale table, then built-in, then derived."""
    return _from_loc(loc_map, key) or STAT_NAMES.get(key) or _humanize(key)


def resolve_slot_name(loc_map: dict[str, str], key: str | None) -> str:
    if not key:
        return ""
    return _from_loc(loc_map, key) or SLOT_NAMES.get(key) or _humanize(key)


def resolve_text(loc_map: dict[str, str], key: str | None) -> str:
    """Plain locale lookup (e.g. an ability `$prefabs_…_description`). '' if absent."""
    return _from_loc(loc_map, key)

"""One-shot migration: rewrite the source-note translation key so the
leaderboards page reads "weekly contests roll Mondays" (correct) instead
of "Tuesdays" (was wrong — chaos chest rolls Tuesday, leaderboards roll
Monday). The English key changes too, so we delete the old key and
write the new one with day-name substituted in each locale's value.

Re-runnable: a second invocation is a no-op once the old key is gone.

Run: `python scripts/fix_leaderboards_weekly_day.py`
"""
from __future__ import annotations

import json
from pathlib import Path

OLD_KEY = (
    "Captures come from an in-game scraping bot every hour. Boards reset "
    "on the same cadence Trove does (daily 11:00 UTC; weekly contests "
    "roll Tuesdays)."
)
NEW_KEY = (
    "Captures come from an in-game scraping bot every hour. Boards reset "
    "on the same cadence Trove does (daily 11:00 UTC; weekly contests "
    "roll Mondays)."
)

# Per-locale Tuesday→Monday substitutions inside the TRANSLATED value.
# We can't blindly swap text since each language renders "Tuesday" /
# "Monday" with its own spelling and grammar.
DAY_SUBS: dict[str, tuple[str, str]] = {
    "fr":    ("le mardi", "le lundi"),
    "de":    ("dienstags", "montags"),
    "pt-PT": ("rolam às terças-feiras", "rolam às segundas-feiras"),
    "ru":    ("по вторникам", "по понедельникам"),
    "ja":    ("火曜日",       "月曜日"),
    "zh-CN": ("周二",         "周一"),
}


def migrate() -> None:
    locales_dir = Path("site/static/locales")
    for lang, (old_day, new_day) in DAY_SUBS.items():
        target = locales_dir / f"{lang}.json"
        if not target.exists():
            print(f"  skip {lang} (no file)")
            continue
        with target.open("r", encoding="utf-8") as f:
            existing = json.load(f)
        if OLD_KEY not in existing:
            print(f"  {lang}: already migrated (no old key)")
            continue
        old_value = existing.pop(OLD_KEY)
        new_value = old_value.replace(old_day, new_day, 1)
        if new_value == old_value:
            print(f"  {lang}: WARN — day substitution didn't match. "
                  f"Look for '{old_day}' in: {old_value!r}")
        existing[NEW_KEY] = new_value
        with target.open("w", encoding="utf-8") as f:
            json.dump(existing, f, ensure_ascii=False, indent=2, sort_keys=True)
            f.write("\n")
        print(f"  {lang}: migrated (total still {len(existing)})")


if __name__ == "__main__":
    migrate()

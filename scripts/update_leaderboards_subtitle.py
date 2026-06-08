"""One-shot: swap the leaderboards subtitle in each locale JSON from the
hardcoded ``3-day`` copy to a ``{days}``-placeholder template. JS reads
``/site/leaderboards/config`` for the live value and substitutes the
placeholder, so the subtitle tracks the runtime tunable.

Re-runnable: only acts on locales that still have the OLD key, so it's
safe to run multiple times.

Run: `python scripts/update_leaderboards_subtitle.py`
"""
from __future__ import annotations

import json
from pathlib import Path

OLD_KEY = "Top players per board, ranked. Hourly captures, 3-day live retention, full archive beyond."
NEW_KEY = "Top players per board, ranked. Hourly captures, {days}-day live retention, full archive beyond."

# Each value uses {days} where the runtime number should land.
NEW_VALUES: dict[str, str] = {
    "fr": "Meilleurs joueurs par classement, par rang. Captures horaires, {days} jours en direct, archive complète au-delà.",
    "de": "Top-Spieler pro Liste, sortiert. Stündliche Erfassung, {days} Tage live, danach vollständiges Archiv.",
    "pt-PT": "Melhores jogadores por classificação, ordenados. Capturas horárias, {days} dias em direto, arquivo completo para além disso.",
    "ru": "Лучшие игроки по каждой таблице, по рангу. Ежечасные снимки, {days} дня в живом доступе, далее — полный архив.",
    "ja": "ボードごとの上位プレイヤー、順位付き。毎時キャプチャ、{days} 日間のライブ保持、それ以降は完全アーカイブ。",
    "zh-CN": "每个榜单的顶尖玩家，按排名展示。每小时采集，{days} 天热数据保留，更早的进入完整归档。",
}


def update() -> None:
    locales_dir = Path("site/static/locales")
    for lang, new_value in NEW_VALUES.items():
        target = locales_dir / f"{lang}.json"
        if not target.exists():
            print(f"  skip {lang} (no file)")
            continue
        with target.open("r", encoding="utf-8") as f:
            existing = json.load(f)
        changed = False
        # Drop the old hardcoded-3 entry if present (it's orphaned once
        # the HTML no longer ships that English source).
        if OLD_KEY in existing:
            del existing[OLD_KEY]
            changed = True
        # Set the new placeholder template.
        if existing.get(NEW_KEY) != new_value:
            existing[NEW_KEY] = new_value
            changed = True
        if changed:
            with target.open("w", encoding="utf-8") as f:
                json.dump(existing, f, ensure_ascii=False, indent=2, sort_keys=True)
                f.write("\n")
            print(f"  {lang}: updated (total now {len(existing)})")
        else:
            print(f"  {lang}: no change needed")


if __name__ == "__main__":
    update()

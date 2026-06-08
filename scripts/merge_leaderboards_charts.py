"""One-shot: add translation keys for the per-board / per-player score
trajectory charts on the leaderboards page. Additive — existing keys
are preserved.

Run: `python scripts/merge_leaderboards_charts.py`
"""
from __future__ import annotations

import json
from pathlib import Path

# Each entry: English source string → per-locale translation. Keys are
# the EXACT JS source the chart code passes to t() — keep in sync with
# leaderboards.js.
TRANSLATIONS: dict[str, dict[str, str]] = {
    "Top players over the last 7 days": {
        "fr": "Meilleurs joueurs des 7 derniers jours",
        "de": "Top-Spieler der letzten 7 Tage",
        "pt-PT": "Melhores jogadores dos últimos 7 dias",
        "ru": "Лучшие игроки за последние 7 дней",
        "ja": "過去7日間のトッププレイヤー",
        "zh-CN": "过去 7 天的顶尖玩家",
    },
    "Score across boards (last 7 days)": {
        "fr": "Score sur les classements (7 derniers jours)",
        "de": "Punkte über Bestenlisten (letzte 7 Tage)",
        "pt-PT": "Pontuação nas classificações (últimos 7 dias)",
        "ru": "Очки по таблицам (за 7 дней)",
        "ja": "ボード別スコア（過去7日間）",
        "zh-CN": "各榜单分数（过去 7 天）",
    },
    "Top {n} · {h} captures": {
        "fr": "Top {n} · {h} captures",
        "de": "Top {n} · {h} Aufnahmen",
        "pt-PT": "Top {n} · {h} capturas",
        "ru": "Топ {n} · {h} срезов",
        "ja": "トップ{n}・{h}回のキャプチャ",
        "zh-CN": "前 {n} 名 · {h} 次抓取",
    },
    "{n} board(s) · {h} captures": {
        "fr": "{n} classement(s) · {h} captures",
        "de": "{n} Bestenliste(n) · {h} Aufnahmen",
        "pt-PT": "{n} classificação(ões) · {h} capturas",
        "ru": "{n} таблиц(ы) · {h} срезов",
        "ja": "{n}個のボード・{h}回のキャプチャ",
        "zh-CN": "{n} 个榜单 · {h} 次抓取",
    },
}


def merge() -> None:
    locales_dir = Path("site/static/locales")
    languages: set[str] = set()
    for translations in TRANSLATIONS.values():
        languages.update(translations.keys())

    for lang in sorted(languages):
        target = locales_dir / f"{lang}.json"
        if not target.exists():
            print(f"  skip {lang} (no file)")
            continue
        with target.open("r", encoding="utf-8") as f:
            existing = json.load(f)
        added = 0
        for english, per_lang in TRANSLATIONS.items():
            if english in existing:
                continue
            value = per_lang.get(lang)
            if value is None:
                continue
            existing[english] = value
            added += 1
        if not added:
            print(f"  {lang}: already up to date")
            continue
        with target.open("w", encoding="utf-8") as f:
            json.dump(existing, f, ensure_ascii=False, indent=2, sort_keys=True)
            f.write("\n")
        print(f"  {lang}: added {added} key(s) (total now {len(existing)})")


if __name__ == "__main__":
    merge()

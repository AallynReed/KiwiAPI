"""One-shot: add translation keys for the price-evolution outlier filter
on /market.  Additive — existing keys are preserved.

Run: `python scripts/merge_market_outliers_translations.py`
"""
from __future__ import annotations

import json
from pathlib import Path

TRANSLATIONS: dict[str, dict[str, str]] = {
    "Show outlier listings": {
        "fr": "Afficher les listings aberrants",
        "de": "Ausreißer-Listings anzeigen",
        "pt-PT": "Mostrar listagens fora do padrão",
        "ru": "Показывать листинги-выбросы",
        "ja": "外れ値リスティングを表示",
        "zh-CN": "显示离群挂牌",
    },
    "{n} outlier(s) excluded ({range})": {
        "fr": "{n} aberrant(s) exclu(s) ({range})",
        "de": "{n} Ausreißer ausgeschlossen ({range})",
        "pt-PT": "{n} fora do padrão excluído(s) ({range})",
        "ru": "Выбросов исключено: {n} ({range})",
        "ja": "{n} 件の外れ値を除外 ({range})",
        "zh-CN": "已排除 {n} 个离群项（{range}）",
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

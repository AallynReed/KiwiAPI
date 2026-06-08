"""One-shot: add translation keys for the /market price-evolution
chart. Additive — existing keys are preserved.

Run: `python scripts/merge_market_chart_translations.py`
"""
from __future__ import annotations

import json
from pathlib import Path

TRANSLATIONS: dict[str, dict[str, str]] = {
    "Price evolution": {
        "fr": "Évolution du prix", "de": "Preisverlauf",
        "pt-PT": "Evolução do preço", "ru": "Динамика цены",
        "ja": "価格の推移", "zh-CN": "价格走势",
    },
    "Include expired listings": {
        "fr": "Inclure les listings expirés",
        "de": "Abgelaufene Listings einbeziehen",
        "pt-PT": "Incluir listagens expiradas",
        "ru": "Включая истёкшие листинги",
        "ja": "期限切れリスティングを含める",
        "zh-CN": "包含已过期的挂牌",
    },
    "{n} listings · window {from} → now": {
        "fr": "{n} listings · fenêtre {from} → maintenant",
        "de": "{n} Listings · Fenster {from} → jetzt",
        "pt-PT": "{n} listagens · janela {from} → agora",
        "ru": "{n} листингов · окно {from} → сейчас",
        "ja": "{n} 件のリスティング · 期間 {from} → 現在",
        "zh-CN": "{n} 个挂牌 · 区间 {from} → 现在",
    },
    "{n} listings (capped) · window {from} → now": {
        "fr": "{n} listings (plafonné) · fenêtre {from} → maintenant",
        "de": "{n} Listings (gedeckelt) · Fenster {from} → jetzt",
        "pt-PT": "{n} listagens (limitado) · janela {from} → agora",
        "ru": "{n} листингов (ограничено) · окно {from} → сейчас",
        "ja": "{n} 件 (上限) · 期間 {from} → 現在",
        "zh-CN": "{n} 个挂牌（已截断）· 区间 {from} → 现在",
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

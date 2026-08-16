"""Fold a batch of translations into the locale files.

Input is one JSON object keyed by the English string, each value a map of
locale -> translation:

    { "Undo": { "de": "Rückgängig", "fr": "Annuler", ... }, ... }

English IS the key, so nothing here invents one: a batch may only supply
translations for strings some template or script already marks, and the merge
refuses anything else rather than quietly adding a key the site will never look
up. Existing entries are left alone unless --overwrite is passed - a locale file
is hand-corrected over time and a re-run must not undo that.

    python scripts/i18n_merge.py batch.json
    python scripts/i18n_merge.py batch.json --overwrite
    python scripts/i18n_merge.py batch.json --dry-run
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import i18n_audit as audit  # noqa: E402


# Every placeholder form the site substitutes into. Lose one in translation and the
# string renders with a literal {n} or drops the number entirely.
_PLACEHOLDER = re.compile(r"\{[a-zA-Z_]\w*\}|%[nsmz]")

# A rough script signature per locale. Latin-script locales must not contain CJK or
# Cyrillic, and vice versa: a stray character from another alphabet is a slip of the
# hand while writing a batch, and it is invisible in a diff of 3,600 lines.
_CJK = re.compile(r"[぀-ヿ㐀-鿿가-힯]")
_CYRILLIC = re.compile(r"[Ѐ-ӿ]")
_FORBIDDEN = {
    "de": (_CJK, _CYRILLIC), "es": (_CJK, _CYRILLIC),
    "fr": (_CJK, _CYRILLIC), "pt-PT": (_CJK, _CYRILLIC),
    "ru": (_CJK,), "ja": (_CYRILLIC,), "ko": (_CYRILLIC,), "zh-CN": (_CYRILLIC,),
    "th": (_CJK, _CYRILLIC),
}


def check(batch: dict[str, dict[str, str]]) -> list[str]:
    problems = []
    for english, per_locale in batch.items():
        want = sorted(set(_PLACEHOLDER.findall(english)))
        for loc, text in per_locale.items():
            got = sorted(set(_PLACEHOLDER.findall(text)))
            if want and got != want:
                problems.append(
                    f"{loc}: placeholders {got or 'none'} != {want}\n"
                    f"       en: {english}\n       {loc}: {text}")
            for pattern in _FORBIDDEN.get(loc, ()):
                stray = pattern.search(text)
                if stray:
                    problems.append(
                        f"{loc}: stray {stray.group()!r} from another script\n"
                        f"       {loc}: {text}")
    return problems


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("batch", type=Path)
    ap.add_argument("--keys", type=Path,
                    help="JSON array of English strings; the batch is then keyed by "
                         "index into it rather than by the string itself")
    ap.add_argument("--overwrite", action="store_true",
                    help="replace translations that are already there")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    batch = json.loads(args.batch.read_text(encoding="utf-8"))
    if args.keys:
        # Retyping an English key by hand is how a curly quote becomes a straight one
        # and the entry lands somewhere the site never looks. Address it by position
        # in the manifest instead and the string is never transcribed at all.
        manifest = json.loads(args.keys.read_text(encoding="utf-8"))
        batch = {manifest[int(i)]: v for i, v in batch.items()}
    every, _, _ = audit.all_keys()

    unknown = [k for k in batch if audit.norm(k) not in every]
    if unknown:
        print(f"REFUSED: {len(unknown)} key(s) no template or script marks.")
        for k in unknown[:20]:
            print(f"  {k!r}")
        print("\nA key the site never looks up is dead weight - fix the string or"
              " mark it in the source first.")
        raise SystemExit(1)

    problems = check(batch)
    if problems:
        print(f"REFUSED: {len(problems)} problem(s).\n")
        for p in problems:
            print(f"  - {p}")
        raise SystemExit(1)

    added: dict[str, int] = {}
    replaced: dict[str, int] = {}
    for path in sorted(audit.LOCALES.glob("*.json")):
        name = path.stem
        data = json.loads(path.read_text(encoding="utf-8"))
        before = len(data)
        for english, per_locale in batch.items():
            value = per_locale.get(name)
            if not value:
                continue
            key = audit.norm(english)
            if key in data:
                if not args.overwrite or data[key] == value:
                    continue
                replaced[name] = replaced.get(name, 0) + 1
            else:
                added[name] = added.get(name, 0) + 1
            data[key] = value
        # ONLY write a file this batch actually changed. Rewriting a locale nothing
        # was added to is pure churn in the diff, and worse than churn while somebody
        # is filling that file in by hand - a read-modify-write over their editor's
        # copy is how in-progress work gets clobbered.
        touched = len(data) != before or replaced.get(name)
        if touched and not args.dry_run:
            # A trailing newline and utf-8: these are read by humans in a diff as
            # much as by the browser.
            path.write_text(
                json.dumps(data, ensure_ascii=False, indent=2) + "\n",
                encoding="utf-8",
            )

    for name in sorted(set(added) | set(replaced)):
        bits = []
        if added.get(name):
            bits.append(f"+{added[name]}")
        if replaced.get(name):
            bits.append(f"~{replaced[name]}")
        print(f"  {name:>6}  {' '.join(bits)}")
    if args.dry_run:
        print("\n(dry run - nothing written)")


if __name__ == "__main__":
    main()

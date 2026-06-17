#!/usr/bin/env python3
"""Offline accuracy harness for the character-stat OCR.

Runs the REAL pipeline (engine.image_to_lines -> parse.extract_character_stats)
over a folder of screenshots and scores it against a hand-labeled ground truth,
so "should be accurate" becomes a measured number ("N% of stats across M real
screenshots") and a regression you can re-run after any vocabulary/parser change.

    python scripts/ocr_eval.py --images-dir "S:\\Desktop\\New folder\\downloads"
    python scripts/ocr_eval.py --images-dir <dir> --dump      # just print extractions
    python scripts/ocr_eval.py --images-dir <dir> --truth <path-to-ground_truth.json>

Ground truth (default: tests/fixtures/ocr/ground_truth.json) is keyed by image
FILENAME (no usernames / no images committed - just the stat values, anonymized):

    { "<filename>": { "complete": true, "stats": { "physical_damage": 799894, ... } } }

`complete: true` means the listed stats are ALL of them on that sheet, so any
EXTRA extracted stat is counted as a false positive (spurious). Use `false` for
partial labels (e.g. an equipment view that only shows Power Rank), where extras
aren't penalised.

NOTE: the OCR engine (rapidocr-onnxruntime) only runs on the deployed Docker
image; a dev venv without it prints an "engine unavailable" notice and exits 2.
The SCORING logic below is pure and unit-tested (tests/unit/trove/test_ocr_eval.py),
so it's verified independently of whether the engine is installed here.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

_IMAGE_EXTS = {".png", ".jpg", ".jpeg", ".webp", ".bmp", ".gif"}
_DEFAULT_TRUTH = ROOT / "tests" / "fixtures" / "ocr" / "ground_truth.json"


def values_match(got, expected, *, float_tol: float = 0.05) -> bool:
    """Exact for integers; small absolute tolerance for floats (absorbs a
    114.8-vs-114.80 representation diff without masking a real misread)."""
    if isinstance(expected, float) or isinstance(got, float):
        return abs(float(got) - float(expected)) <= float_tol
    return int(got) == int(expected)


def score_image(extracted: dict, truth_stats: dict, *, complete: bool) -> dict:
    """Compare one image's extracted stats against its ground truth.

    ``extracted`` is parse.extract_character_stats()'s ``stats`` dict
    (``{key: {value, ...}}``); ``truth_stats`` is ``{key: value}``. Returns
    correct / wrong / missed / spurious breakdowns + the truth total."""
    correct: list[str] = []
    wrong: list[dict] = []
    missed: list[str] = []
    for key, want in truth_stats.items():
        if key not in extracted:
            missed.append(key)
            continue
        got = extracted[key]["value"]
        if values_match(got, want):
            correct.append(key)
        else:
            wrong.append({"stat": key, "expected": want, "got": got})
    spurious = sorted(set(extracted) - set(truth_stats)) if complete else []
    return {
        "correct": correct, "wrong": wrong, "missed": missed,
        "spurious": spurious, "total": len(truth_stats),
    }


def aggregate(results: list[dict]) -> dict:
    """Roll per-image scores into overall stat-level metrics."""
    correct = sum(len(r["correct"]) for r in results)
    wrong = sum(len(r["wrong"]) for r in results)
    missed = sum(len(r["missed"]) for r in results)
    spurious = sum(len(r["spurious"]) for r in results)
    total = sum(r["total"] for r in results)
    return {
        "images": len(results),
        "total_stats": total,
        "correct": correct, "wrong": wrong, "missed": missed, "spurious": spurious,
        "stat_accuracy": (correct / total) if total else 0.0,
    }


def _iter_images(images_dir: Path):
    return sorted(p for p in images_dir.iterdir()
                  if p.is_file() and p.suffix.lower() in _IMAGE_EXTS)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Score the character-stat OCR against ground truth.")
    ap.add_argument("--images-dir", required=True, type=Path, help="Folder of screenshots")
    ap.add_argument("--truth", type=Path, default=_DEFAULT_TRUTH, help="Ground-truth JSON")
    ap.add_argument("--dump", action="store_true",
                    help="Print every image's extracted stats (no scoring)")
    args = ap.parse_args(argv)

    # Imported here so --help works even where the engine dep is absent.
    from app.trove.ocr import engine, parse

    if not engine.available():
        print("OCR engine unavailable (rapidocr-onnxruntime not installed).\n"
              "This harness runs the real engine - run it on the deployed image, "
              "or `pip install rapidocr-onnxruntime` locally.", file=sys.stderr)
        return 2
    if not args.images_dir.is_dir():
        print(f"Not a directory: {args.images_dir}", file=sys.stderr)
        return 2

    truth = {}
    if not args.dump:
        if not args.truth.is_file():
            print(f"Ground truth not found: {args.truth} (use --dump to skip scoring)",
                  file=sys.stderr)
            return 2
        truth = json.loads(args.truth.read_text(encoding="utf-8"))

    results = []
    for img in _iter_images(args.images_dir):
        try:
            lines = engine.image_to_lines(img.read_bytes())
        except Exception as exc:  # noqa: BLE001 - report + continue the sweep
            print(f"[skip] {img.name}: {exc}")
            continue
        extracted = parse.extract_character_stats(lines)["stats"]

        if args.dump or img.name not in truth:
            tag = "" if args.dump else "  (no ground truth)"
            print(f"\n=== {img.name} ({len(extracted)} stats){tag} ===")
            for key in sorted(extracted):
                v = extracted[key]
                flags = "" if (v["in_range"] and v["type_match"]) else "  <-- FLAGGED"
                print(f"  {key:18} {v['value']!s:>14}  conf={v['confidence']:.2f}{flags}")
            continue

        entry = truth[img.name]
        score = score_image(extracted, entry.get("stats", {}),
                            complete=bool(entry.get("complete")))
        results.append(score)
        acc = len(score["correct"]) / score["total"] if score["total"] else 0.0
        print(f"\n=== {img.name}: {len(score['correct'])}/{score['total']} ({acc:.0%}) ===")
        for w in score["wrong"]:
            print(f"  WRONG   {w['stat']:18} expected {w['expected']} got {w['got']}")
        for k in score["missed"]:
            print(f"  MISSED  {k}")
        for k in score["spurious"]:
            print(f"  EXTRA   {k}")

    if results:
        agg = aggregate(results)
        print("\n" + "=" * 50)
        print(f"Images scored : {agg['images']}")
        print(f"Stat accuracy : {agg['stat_accuracy']:.1%}  "
              f"({agg['correct']}/{agg['total_stats']})")
        print(f"  wrong={agg['wrong']}  missed={agg['missed']}  spurious={agg['spurious']}")
    elif not args.dump:
        print("\nNo images matched the ground truth. Add entries keyed by filename, "
              "or run with --dump to see raw extractions.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

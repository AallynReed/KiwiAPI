"""Text-vs-binary detection + structured unified-diff producer for the
``GET /v1/updates/{branch}/file/compare`` endpoint.

Not to be confused with ``app/trove/updates/diff.py`` (which classifies
ingestion-time manifest changes). This module operates on two blob byte
strings AFTER they've been pulled from the CAS and produces hunks ready
to render as side-by-side / unified diff. Pure functions - no IO.
"""

from __future__ import annotations

import difflib
from dataclasses import dataclass

# Decode budgets. Anything bigger than ``MAX_DIFF_BYTES`` on either side is
# treated as binary regardless of contents - protects the response payload
# AND the page-side diff renderer from a multi-MB blob landing in the DOM.
MAX_DIFF_BYTES = 1_024 * 1_024            # 1 MiB per side
# Context lines kept around each hunk. 3 matches ``diff -u`` defaults.
HUNK_CONTEXT = 3


@dataclass(frozen=True)
class DecodedSide:
    """Result of attempting to decode a blob into text for diffing."""
    text: str | None
    lines: list[str]
    is_text: bool
    reason: str | None = None    # only set when is_text=False


def _escape_ctrl(text: str) -> str:
    """Rewrite NUL + other C0 control bytes (except TAB/CR/LF/FF) as
    visible ``\\xNN`` escapes so the diff renderer doesn't drop them and
    so the user can see them as comparable content. Cost is linear in
    the text length; called once per side."""
    out: list[str] = []
    for c in text:
        o = ord(c)
        if c in ("\t", "\r", "\n", "\f"):
            out.append(c)
        elif o < 0x20 or o == 0x7F:
            out.append(f"\\x{o:02x}")
        else:
            out.append(c)
    return "".join(out)


def decode_blob(data: bytes | None) -> DecodedSide:
    """Decode ``data`` for diffing. Strategy:
      • ``None`` (path didn't exist at this side's version) → empty text side
      • bytes > MAX_DIFF_BYTES → "too large" - refuse to diff inline so
        we don't ship a 100 MB JSON payload to the browser
      • otherwise: always decode as text (utf-8 first, latin-1 fallback
        - latin-1 maps every byte unambiguously so decode never fails)
        and escape NUL + other C0 control bytes to visible ``\\xNN``
        markers so the diff renderer doesn't choke on them.

    We DELIBERATELY don't try to classify "binary vs text" by content.
    Two reasons:
      1. Trove's hybrid formats (``.binfab``, ``.blueprint``, archive
         tables) are mostly readable text wrapped around small packed
         structs - they look "binary" by any cheap ratio heuristic but
         the user wants to diff them and the result is actually useful.
      2. Structured binary formats (length-prefixed records, packed
         tag tables) have HIGHER control-byte ratios than random binary
         data does, so a ratio gate would flag exactly the formats we
         most want to expose. Better to render everything as text and
         let truly binary content look like the noise it is - the user
         can still see WHICH bytes changed, which is the whole point.
    """
    if data is None:
        return DecodedSide(text="", lines=[], is_text=True)
    if len(data) > MAX_DIFF_BYTES:
        return DecodedSide(text=None, lines=[], is_text=False, reason="too large")

    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        # latin-1 maps every byte to a code point - always succeeds.
        text = data.decode("latin-1")

    text = _escape_ctrl(text)
    # splitlines drops trailing CR/LF - matches difflib expectations.
    return DecodedSide(text=text, lines=text.splitlines(), is_text=True)


def make_hunks(
    a_lines: list[str], b_lines: list[str], context: int = HUNK_CONTEXT,
) -> list[dict]:
    """Build structured unified-diff hunks from two decoded line lists.

    Returns a list of ``{left_start, right_start, lines}`` dicts.
    ``lines[*].kind`` is one of ``equal`` / ``remove`` / ``add``.
    Identical files return ``[]`` - caller should set ``identical=True``.
    """
    sm = difflib.SequenceMatcher(a=a_lines, b=b_lines, autojunk=False)
    opcodes = sm.get_opcodes()
    hunks: list[dict] = []

    i = 0
    while i < len(opcodes):
        tag, _i1, _i2, _j1, _j2 = opcodes[i]
        if tag == "equal":
            i += 1
            continue

        # Pull context BEFORE the first change.
        if i > 0 and opcodes[i - 1][0] == "equal":
            _, ei1, ei2, ej1, ej2 = opcodes[i - 1]
            ctx_before = min(context, ei2 - ei1)
        else:
            ei1 = ei2 = opcodes[i][1]
            ej1 = ej2 = opcodes[i][3]
            ctx_before = 0
        left_start_idx = ei2 - ctx_before
        right_start_idx = ej2 - ctx_before

        # Collect changes, bridging short equal-runs (≤ 2*context).
        run: list[tuple[str, int, int, int, int]] = []
        if ctx_before:
            run.append((
                "equal", left_start_idx, ei2, right_start_idx, ej2,
            ))

        j = i
        while j < len(opcodes):
            otag, oi1, oi2, oj1, oj2 = opcodes[j]
            if otag == "equal":
                # Either bridge if short and not last, or stop + add context-after.
                if j + 1 < len(opcodes) and (oi2 - oi1) <= 2 * context:
                    run.append((otag, oi1, oi2, oj1, oj2))
                    j += 1
                    continue
                ctx_after = min(context, oi2 - oi1)
                if ctx_after:
                    run.append((
                        "equal", oi1, oi1 + ctx_after, oj1, oj1 + ctx_after,
                    ))
                j += 1
                break
            run.append((otag, oi1, oi2, oj1, oj2))
            j += 1

        # Materialise run → structured line dicts.
        lines: list[dict] = []
        for otag, oi1, oi2, oj1, oj2 in run:
            if otag == "equal":
                for k in range(oi2 - oi1):
                    lines.append({
                        "kind": "equal",
                        "left": oi1 + k + 1,
                        "right": oj1 + k + 1,
                        "text": a_lines[oi1 + k],
                    })
            elif otag == "delete":
                for k in range(oi2 - oi1):
                    lines.append({
                        "kind": "remove",
                        "left": oi1 + k + 1,
                        "right": None,
                        "text": a_lines[oi1 + k],
                    })
            elif otag == "insert":
                for k in range(oj2 - oj1):
                    lines.append({
                        "kind": "add",
                        "left": None,
                        "right": oj1 + k + 1,
                        "text": b_lines[oj1 + k],
                    })
            elif otag == "replace":
                # Show removes first, then adds - familiar diff layout.
                for k in range(oi2 - oi1):
                    lines.append({
                        "kind": "remove",
                        "left": oi1 + k + 1,
                        "right": None,
                        "text": a_lines[oi1 + k],
                    })
                for k in range(oj2 - oj1):
                    lines.append({
                        "kind": "add",
                        "left": None,
                        "right": oj1 + k + 1,
                        "text": b_lines[oj1 + k],
                    })

        hunks.append({
            "left_start": left_start_idx + 1,
            "right_start": right_start_idx + 1,
            "lines": lines,
        })
        i = j
    return hunks

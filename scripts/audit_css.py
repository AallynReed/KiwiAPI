"""Identify CSS class selectors that aren't referenced anywhere in the
HTML or in the static JavaScript bundles.

A "reference" is one of:
  - present in a `class="..."` attribute in index.html
  - present in a `classList.add/remove/toggle/contains('...')` call in any
    of the static JS files (so dynamically-added classes count as used)
  - present as a hyphenated-token bare string literal in JS (conservative
    heuristic for class names built inline)

Anything that fails all three is reported as POTENTIALLY unused — review
each one manually before deleting, because a class name could also be
injected by a template macro, a CSS preprocessor, or referenced by
attribute (`data-*`) selectors that aren't tokens.
"""
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HTML = ROOT / "site" / "templates" / "index.html"
CSS = ROOT / "site" / "static" / "style.css"
JS_FILES = [
    ROOT / "site" / "static" / "app.js",
    ROOT / "site" / "static" / "landing.js",
    ROOT / "site" / "static" / "i18n.js",
]

html_src = HTML.read_text(encoding="utf-8")
css_src = CSS.read_text(encoding="utf-8")
js_src = "\n".join(p.read_text(encoding="utf-8") for p in JS_FILES)

# Strip CSS comments so we don't false-positive on .selectors-mentioned-in-prose
css_nocomments = re.sub(r"/\*[\s\S]*?\*/", "", css_src)

# Every class used as a selector token in style.css
selector_classes: set[str] = set()
for m in re.finditer(r"([^{}@][^{}]*)\{", css_nocomments):
    block = m.group(1)
    for cls in re.findall(r"\.([\w-]+)", block):
        selector_classes.add(cls)

# Classes that show up in any class="..." attribute in the HTML
html_classes: set[str] = set()
for v in re.findall(r"class=\"([^\"]+)\"", html_src):
    html_classes.update(v.split())

# Classes the JS deliberately manipulates via classList.<verb>("xxx")
js_classes: set[str] = set()
verb_re = re.compile(
    r"classList\.(?:add|remove|toggle|contains)\(\s*['\"]([\w-]+)['\"]"
)
for m in verb_re.finditer(js_src):
    js_classes.add(m.group(1))

# Bare string literals that look like class tokens (hyphenated, alphanumeric)
# — conservative heuristic for inline className construction. Single-token
# strings like 'flash' or 'open' won't match (too many false positives),
# but multi-hyphen ones almost certainly are class names.
hyphenated_re = re.compile(r"['\"]([\w-]+(?:-[\w-]+)+)['\"]")
for m in hyphenated_re.finditer(js_src):
    js_classes.add(m.group(1))

# className = "..." or element.className = `...` — direct property writes
classname_re = re.compile(r"\.className\s*=\s*['\"`]([^'\"`]+)['\"`]")
for m in classname_re.finditer(js_src):
    js_classes.update(m.group(1).split())
# Same pattern across all html templates too (the shotFail() inline script
# in index.html does `ph.className = 'shot-placeholder'`).
for m in classname_re.finditer(html_src):
    js_classes.update(m.group(1).split())

# class="..." baked into JS/HTML template literals (e.g. innerHTML
# building). Matches "class='foo bar'" or 'class="foo bar"' regardless of
# quote nesting. Multi-class supported.
class_attr_re = re.compile(r"""class\s*=\s*\\?['\"]([^'\"\\]+)\\?['\"]""")
for m in class_attr_re.finditer(js_src):
    js_classes.update(m.group(1).split())

used = html_classes | js_classes
unused = sorted(selector_classes - used)

print(f"Total class selectors in CSS:    {len(selector_classes)}")
print(f"Classes referenced (HTML + JS):  {len(used)}")
print(f"POTENTIALLY unused:              {len(unused)}")
print()
for c in unused:
    print(f"  .{c}")

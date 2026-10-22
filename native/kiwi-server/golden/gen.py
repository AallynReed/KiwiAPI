"""Regenerate the runtime_config registry table and the golden fixtures from the
Python code. Run from the repo root with the project venv:

    .venv/Scripts/python.exe native/kiwi-server/golden/gen.py
"""

from __future__ import annotations

import ast
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
REGISTRY_RS = HERE.parent / "src" / "runtime_config" / "registry.rs"

REGISTRY_ENV = {
    "API_RATE_LIMIT_MAX": "77",
    "CHEATERS_Z_THRESHOLD": "4.5",
    "CHEATERS_MIN_BOARD_SIZE": "7.0",
    "MODS_HUB_ENABLED": "false",
    "TROVE_STATUS_EU_HOST": "eu.example.test",
    "CHEATERS_EXCLUDED_BOARD_UUIDS": "1100, 21012",
    "capture_completeness_enabled": "0",
    "Trove_Status_Game_Hold_Seconds": "2",
}

SETTINGS_CASES = [
    {"API_RATE_LIMIT_MAX": "77"},
    {"debug": "yes"},
    {"Debug": "On"},
    {"DEBUG": "T"},
    {"DEBUG": "0"},
    {"DEBUG": "2"},
    {"DEBUG": " true"},
    {"DEBUG": ""},
    {"MAX_REQUEST_BODY_BYTES": " 1_000 "},
    {"MAX_REQUEST_BODY_BYTES": "5.0"},
    {"MAX_REQUEST_BODY_BYTES": "+5.000"},
    {"MAX_REQUEST_BODY_BYTES": "-007"},
    {"MAX_REQUEST_BODY_BYTES": "5.5"},
    {"MAX_REQUEST_BODY_BYTES": "5."},
    {"MAX_REQUEST_BODY_BYTES": "1e3"},
    {"MAX_REQUEST_BODY_BYTES": "1__0"},
    {"MAX_REQUEST_BODY_BYTES": ""},
    {"MAX_REQUEST_BODY_BYTES": "1_000.0"},
    {"MAX_REQUEST_BODY_BYTES": "-0.0"},
    {"MAX_REQUEST_BODY_BYTES": ".0"},
    {"MAX_REQUEST_BODY_BYTES": "+1_2"},
    {"MAX_REQUEST_BODY_BYTES": "1_"},
    {"MAX_REQUEST_BODY_BYTES": "٣"},
    {"MAX_REQUEST_BODY_BYTES": "\t42\n"},
    {"CHEATERS_Z_THRESHOLD": "4"},
    {"CHEATERS_Z_THRESHOLD": "1_0.5"},
    {"CHEATERS_Z_THRESHOLD": "_1.5"},
    {"CHEATERS_Z_THRESHOLD": "+.5"},
    {"CHEATERS_Z_THRESHOLD": "1E3"},
    {"CHEATERS_Z_THRESHOLD": "-0"},
    {"CHEATERS_Z_THRESHOLD": "inf"},
    {"CHEATERS_Z_THRESHOLD": "NaN"},
    {"CHEATERS_Z_THRESHOLD": "1e400"},
    {"CHEATERS_Z_THRESHOLD": "1e-5"},
    {"CHEATERS_Z_THRESHOLD": " 2_5.5 "},
    {"CHEATERS_Z_THRESHOLD": "5."},
    {"CHEATERS_Z_THRESHOLD": "abc"},
    {"CHEATERS_Z_THRESHOLD": ""},
    {"REDIS_URL": ""},
    {"REDIS_URL": "redis://redis:6379/0"},
    {"POSTGRES_DSN": "postgresql://x"},
    {"POSTGRES_DSN": ""},
    {"GITHUB_CLIENT_ID": "a"},
    {"GITHUB_CLIENT_ID": "a", "GITHUB_CLIENT_SECRET": "b"},
    {"GITHUB_CLIENT_ID": "a", "GITHUB_CLIENT_SECRET": ""},
    {"DISCORD_CLIENT_ID": "123"},
    {"DISCORD_CLIENT_ID": "123", "DISCORD_CLIENT_SECRET": "s"},
    {"DISCORD_CLIENT_ID": "123", "DISCORD_INSTALL_URL": "https://install.test/x"},
    {"DISCORD_CLIENT_ID": "123", "DISCORD_INSTALL_URL": ""},
    {"DISCORD_CLIENT_ID": "", "DISCORD_INSTALL_URL": ""},
    {"SMTP_HOST": "mail"},
    {"SMTP_HOST": ""},
    {"ASSET_BASE_URL": "https://api.aallyn.net/"},
    {"APP_URL": "https://x.test//"},
    {"APP_URL": "", "ASSET_BASE_URL": ""},
    {"CORS_ORIGINS": '["https://a.test", "http://b.test:1"]'},
    {"CORS_ORIGINS": "[]"},
    {"CORS_ORIGINS": "https://a.test"},
    {"CORS_ORIGINS": "[1]"},
    {"CORS_ORIGINS": '"x"'},
    {"CORS_ORIGINS": '{"a": 1}'},
    {"CORS_ORIGINS": ""},
    {"CORS_ORIGINS": ' [ "x" ] '},
    {"CAPTCHA_PROVIDER": "hcaptcha"},
    {"CAPTCHA_PROVIDER": "HCAPTCHA"},
    {"CAPTCHA_PROVIDER": "recaptcha"},
    {"RATE_LIMIT_ALERT_EMAIL": ""},
    {"TROVE_STATUS_TIMEOUT_SECONDS": "8"},
    {"BACKLOG_RETENTION_DAYS": "-1"},
    {"UNKNOWN_THING": "x"},
    {"SECRET_KEY": "  spaced  "},
    {"APP_NAME": "Kïwi ✓"},
    {"MODS_HUB_ENABLED": "false", "TROVE_UPDATE_ENABLED": "on"},
]

DECODE_CASES = [
    "5", "5.0", "-3", "0", '"hi"', '"\\u00e9t\\u00e9"', '"café"', "true", "false",
    "null", "[1, 2]", '{"a": 1}', "nope", "", " 7 ", "1e3", '"', "01", "True",
    "1.5e+3", "0.1", "1e16", "1e-05", "-0.0", "  \"x\"\n",
]


def field_names() -> list[str]:
    from app.core.config import Settings
    return list(Settings.model_fields)


def clean_env(extra: dict[str, str]) -> dict[str, str]:
    names = {n.lower() for n in field_names()}
    env = {k: v for k, v in os.environ.items() if k.lower() not in names}
    env.update(extra)
    env["PYTHONIOENCODING"] = "utf-8"
    return env


def jsonable(v):
    if isinstance(v, float) and v != v:
        return {"$float": "nan"}
    if isinstance(v, float) and v in (float("inf"), float("-inf")):
        return {"$float": repr(v)}
    return v


# ── child modes (run in a subprocess with a controlled environment) ────────


def child_registry() -> None:
    from app.admin import runtime_config as rc
    src = (ROOT / "app" / "admin" / "runtime_config.py").read_text(encoding="utf-8")
    from_settings = {}
    for node in ast.walk(ast.parse(src)):
        if isinstance(node, ast.Call) and getattr(node.func, "id", None) == "_t":
            kw = {k.arg: k.value for k in node.keywords}
            d = kw["default"]
            if isinstance(d, ast.Attribute) and getattr(d.value, "id", None) == "settings":
                from_settings[ast.literal_eval(kw["key"])] = d.attr
            elif not isinstance(d, ast.Constant):
                raise SystemExit(f"unsupported default expression for {ast.literal_eval(kw['key'])}")
    out = []
    for key, s in rc.REGISTRY.items():
        out.append({
            "key": key,
            "type": s.type,
            "category": s.category,
            "description": s.description,
            "secret": s.secret,
            "min_value": s.min_value,
            "max_value": s.max_value,
            "choices": list(s.choices) if s.choices is not None else None,
            "pattern": s.pattern,
            "default": jsonable(s.default),
            "default_from": from_settings.get(key),
        })
    sys.stdout.write(json.dumps(out, ensure_ascii=False))


def child_settings(cases_json: str) -> None:
    from app.core.config import Settings
    cases = json.loads(cases_json)
    names = {n.lower() for n in Settings.model_fields}

    def dump(s) -> dict:
        d = {k: jsonable(getattr(s, k)) for k in Settings.model_fields}
        for p in ("github_oauth_enabled", "discord_oauth_enabled", "discord_install_link",
                  "postgres_enabled", "email_enabled", "asset_url"):
            d[p] = getattr(s, p)
        return d

    for k in list(os.environ):
        if k.lower() in names:
            del os.environ[k]
    base = dump(Settings(_env_file=None))
    results = []
    for env in cases:
        for k in list(os.environ):
            if k.lower() in names:
                del os.environ[k]
        os.environ.update(env)
        try:
            got = dump(Settings(_env_file=None))
        except Exception as e:
            results.append({"env": env, "error": type(e).__name__})
            continue
        results.append({"env": env, "changed": {k: v for k, v in got.items() if base[k] != v
                                                  or type(base[k]) is not type(v)}})
    sys.stdout.write(json.dumps({"defaults": base, "cases": results}, ensure_ascii=False))


def child_validation() -> None:
    from app.admin import runtime_config as rc

    probes_common = ["str", "", "ünïcöde ✓", "\U0001f600", "a\"b\\c\n\x7f", True, False,
                     0, 1, -1, 1.5, 2.0, None, [], {}, 1e16, 1e-05, 0.1, 10**15, 123456.789]
    seen = set()
    cases = []

    def run(spec, value):
        try:
            v = rc._validate(spec, value)
            return {"ok": json.dumps(v)}
        except rc.InvalidSettingError as e:
            return {"error": str(e)}

    def add(spec, value, synthetic=None):
        case = {"key": spec.key, "value": value, **run(spec, value)}
        if synthetic is not None:
            case["spec"] = synthetic
        cases.append(case)

    for spec in rc.REGISTRY.values():
        sig =(spec.type, spec.min_value, spec.max_value, spec.choices, spec.pattern,
               type(spec.min_value), type(spec.max_value))
        add(spec, spec.default)
        add(spec, "x" if spec.type != "str" else 1)
        if sig in seen:
            continue
        seen.add(sig)
        values = list(probes_common)
        for b in (spec.min_value, spec.max_value):
            if b is not None:
                values += [b, b - 1, b + 1, b - 0.5, b + 0.5, float(b)]
        if spec.pattern:
            values += ["FR", "fr, de", " FR ,DE ", "FRA", "F", "FR,", " FR", "\x1cFR", "FR\n",
                       "FR ,　DE", "ÉÉ"]
        for v in values:
            add(spec, v)

    synthetic = [
        {"key": "x.choice", "type": "str", "choices": ["a", "b"]},
        {"key": "x.quotes", "type": "str", "choices": ["it's", 'say "hi"', "both ' \"", "back\\slash",
                                                         "tab\there", "nl\nx", "café", "\x01\x7f"]},
        {"key": "x.empty", "type": "str", "choices": []},
        {"key": "x.fbounds", "type": "float", "min_value": 1e-05, "max_value": 1e16},
        {"key": "x.ibounds", "type": "int", "min_value": 0.5, "max_value": 10.0},
        {"key": "x.negzero", "type": "float", "min_value": -0.0, "max_value": 100},
        {"key": "x.pattern", "type": "str", "pattern": r"[a-z]+\d{2}", "choices": ["ab12", "zz"]},
    ]
    for syn in synthetic:
        spec = rc.TunableSetting(
            key=syn["key"], default=None, type=syn["type"], category="test", description="",
            min_value=syn.get("min_value"), max_value=syn.get("max_value"),
            choices=tuple(syn["choices"]) if "choices" in syn else None, pattern=syn.get("pattern"),
        )
        for v in ["a", "c", "it's", "zz", "ab12", "AB12", "", 0.5, 0, 1e-06, 1e17, 10, 11, -1e-300,
                  True, 3, 1e16, 2.5, 0.25]:
            add(spec, v, synthetic=syn)
    sys.stdout.write(json.dumps(cases, ensure_ascii=False))


def child_decode() -> None:
    # get_setting's rule: json.loads(doc.value), or the default when that fails.
    out = []
    for raw in DECODE_CASES:
        try:
            out.append({"stored": raw, "value": jsonable(json.loads(raw))})
        except json.JSONDecodeError:
            out.append({"stored": raw, "default": True})
    sys.stdout.write(json.dumps(out, ensure_ascii=False))


def child_floats() -> None:
    import random
    import struct
    rnd = random.Random(20261022)
    vals = [0.0, -0.0, 1.0, -1.0, 0.1, 0.2 + 0.1, 1e16, 1e15, 9999999999999998.0, 1e-4, 1e-5,
            0.00012345, 123456789.123, 5e-324, 1.7976931348623157e308, 2.5, 1e22, 1e-7, 100.0,
            0.05, 0.9, 1234567890123456.7, 12345678901234567.0, 0.000999, 1.5e-05]
    for _ in range(300):
        vals.append(rnd.uniform(-1, 1) * 10 ** rnd.randint(-30, 30))
    for _ in range(400):
        vals.append(rnd.randint(2**50, 2**53) / 2 ** rnd.randint(0, 8) * rnd.choice((1, -1)))
    for _ in range(100):
        f = struct.unpack("<d", struct.pack("<Q", rnd.getrandbits(64)))[0]
        if f == f and f not in (float("inf"), float("-inf")):
            vals.append(f)
    sys.stdout.write(json.dumps([repr(v) for v in vals]))


# ── parent ─────────────────────────────────────────────────────────────────


def run_child(mode: str, env_extra: dict[str, str] | None = None, arg: str | None = None):
    cmd = [sys.executable, str(Path(__file__).resolve()), mode] + ([arg] if arg else [])
    res = subprocess.run(cmd, cwd=ROOT, env=clean_env(env_extra or {}), capture_output=True)
    if res.returncode != 0:
        sys.stderr.write(res.stderr.decode("utf-8", "replace"))
        raise SystemExit(f"child {mode} failed")
    return json.loads(res.stdout.decode("utf-8"))


def rust_str(s: str) -> str:
    out = []
    for ch in s:
        if ch == "\\":
            out.append("\\\\")
        elif ch == '"':
            out.append('\\"')
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\t":
            out.append("\\t")
        elif ch == "\r":
            out.append("\\r")
        elif ord(ch) < 0x20 or ord(ch) == 0x7F:
            out.append(f"\\u{{{ord(ch):x}}}")
        else:
            out.append(ch)
    return '"' + "".join(out) + '"'


def rust_wrapped(s: str, indent: str, width: int = 72) -> str:
    """A string literal split with `\\` continuations (only after a space that is
    followed by a non-space, so no whitespace is swallowed)."""
    lit = rust_str(s)[1:-1]
    lines, cur = [], ""
    i = 0
    while i < len(lit):
        cur += lit[i]
        if lit[i] == " " and len(cur) >= width and i + 1 < len(lit) and lit[i + 1] != " " \
                and not cur.endswith("\\ "):
            lines.append(cur)
            cur = ""
        i += 1
    lines.append(cur)
    if len(lines) == 1:
        return '"' + lines[0] + '"'
    return '"' + ("\\\n" + indent).join(lines) + '"'


def rust_num(v) -> str:
    if v is None:
        return "None"
    if isinstance(v, bool):
        raise TypeError(v)
    if isinstance(v, int):
        return f"Some(Num::Int({v}))"
    r = repr(float(v))
    if "e" not in r and "." not in r:
        r += ".0"
    return f"Some(Num::Float({r}))"


def write_registry_rs(entries: list[dict], settings_types: dict[str, str]) -> None:
    lines = [
        "// Generated by golden/gen.py from app/admin/runtime_config.py. Do not edit.",
        "",
        "use serde_json::Value;",
        "",
        "use super::{DefaultValue, Num, Spec, Ty};",
        "",
        "pub static REGISTRY: &[Spec] = &[",
    ]
    for e in entries:
        if e["default_from"]:
            f = e["default_from"]
            conv = f"Value::from(s.{f}.clone())" if settings_types[f] == "str" else f"Value::from(s.{f})"
            default = f"DefaultValue::Setting(|s| {conv})"
        else:
            d = e["default"]
            if isinstance(d, bool):
                default = f"DefaultValue::Bool({'true' if d else 'false'})"
            elif isinstance(d, int):
                default = f"DefaultValue::Int({d})"
            elif isinstance(d, float):
                default = f"DefaultValue::Float({d!r})"
            elif isinstance(d, str):
                default = f"DefaultValue::Str({rust_str(d)})"
            else:
                raise SystemExit(f"unsupported default {d!r}")
        ty = {"str": "Str", "int": "Int", "bool": "Bool", "float": "Float"}[e["type"]]
        lines += [
            "    Spec {",
            f"        key: {rust_str(e['key'])},",
            f"        ty: Ty::{ty},",
            f"        category: {rust_str(e['category'])},",
            f"        description: {rust_wrapped(e['description'], ' ' * 12)},",
            f"        default: {default},",
        ]
        if e["secret"]:
            lines.append("        secret: true,")
        if e["min_value"] is not None:
            lines.append(f"        min_value: {rust_num(e['min_value'])},")
        if e["max_value"] is not None:
            lines.append(f"        max_value: {rust_num(e['max_value'])},")
        if e["choices"] is not None:
            lines.append("        choices: Some(&[" + ", ".join(rust_str(c) for c in e["choices"]) + "]),")
        if e["pattern"] is not None:
            p = e["pattern"]
            lit = f'r"{p}"' if '"' not in p else f'r#"{p}"#'
            lines.append(f"        pattern: Some({lit}),")
        lines += ["        ..Spec::BASE", "    },"]
    lines += ["];", ""]
    REGISTRY_RS.parent.mkdir(parents=True, exist_ok=True)
    REGISTRY_RS.write_text("\n".join(lines), encoding="utf-8", newline="\n")


def settings_types() -> dict[str, str]:
    from app.core.config import Settings
    out = {}
    for name, f in Settings.model_fields.items():
        a = f.annotation
        out[name] = {str: "str", int: "int", float: "float", bool: "bool"}.get(a, repr(a))
    return out


def write_json(name: str, data, one_per_line: bool = False) -> None:
    if one_per_line:
        text = "[\n" + ",\n".join(json.dumps(c, ensure_ascii=False) for c in data) + "\n]\n"
    else:
        text = json.dumps(data, ensure_ascii=False, indent=1) + "\n"
    (HERE / name).write_text(text, encoding="utf-8", newline="\n")


def main() -> None:
    sys.path.insert(0, str(ROOT))
    os.chdir(ROOT)
    entries = run_child("--registry")
    write_registry_rs(entries, settings_types())
    write_json("runtime_config_registry.json", {
        "entries": entries,
        "custom_env": REGISTRY_ENV,
        "custom_defaults": {e["key"]: e["default"] for e in run_child("--registry", REGISTRY_ENV)},
    })
    write_json("runtime_config_validation.json", run_child("--validation"), one_per_line=True)
    write_json("runtime_config_decode.json", run_child("--decode"), one_per_line=True)
    write_json("settings.json", run_child("--settings", arg=json.dumps(SETTINGS_CASES)))
    write_json("float_repr.json", run_child("--floats"), one_per_line=True)


if __name__ == "__main__":
    sys.path.insert(0, str(ROOT))
    mode = sys.argv[1] if len(sys.argv) > 1 else ""
    if mode == "--registry":
        child_registry()
    elif mode == "--validation":
        child_validation()
    elif mode == "--decode":
        child_decode()
    elif mode == "--settings":
        child_settings(sys.argv[2])
    elif mode == "--floats":
        child_floats()
    else:
        main()

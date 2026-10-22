"""Golden fixtures for the stdlib-only behaviour the foundation mirrors.

Run with the production Python version (3.13+, stdlib only):
    py -3.14 native/kiwi-server/testdata/foundation/gen_stdlib.py
Writes stdlib.json next to this file. Values cross over in a tagged form
(see enc) so floats and big ints survive exactly.
"""
import datetime as dt
import http
import json
import math
import random
import struct
import sys
import urllib.parse
from pathlib import Path

rng = random.Random(20261022)
OUT = Path(__file__).with_name("stdlib.json")


def fbits(x: float) -> str:
    return "%016x" % struct.unpack("<Q", struct.pack("<d", x))[0]


def enc(v):
    if v is None or isinstance(v, (bool, str)):
        return v
    if isinstance(v, int):
        return {"i": str(v)}
    if isinstance(v, float):
        return {"f": fbits(v)}
    if isinstance(v, list):
        return [enc(x) for x in v]
    if isinstance(v, dict):
        return {"d": [[k, enc(x)] for k, x in v.items()]}
    raise TypeError(type(v))


def floats() -> list[float]:
    out = [0.0, -0.0, 1.0, -1.0, 0.1, 0.2, 0.3, 1 / 3, 2 / 3, 0.5, 1.5, 2.5, 100.0, 1e15, 1e16, 1e17,
           9999999999999998.0, 9007199254740993.0, 123456789012345678.0, 1e-4, 1e-5, 1e-6, 1e-7,
           0.0001, 0.00009999, 0.00001234, 1.5e-7, 9.5e-6, 1e21, 1e22, 1e100, 1e300, 5e-324,
           2.2250738585072014e-308, sys.float_info.max, sys.float_info.epsilon, 12345.678, 0.1 + 0.2]
    for e in range(-325, 310):
        for m in (1.0, 9.999):
            x = float(f"{m}e{e}")
            if math.isfinite(x):
                out.append(x if e % 2 else -x)
    # halfway cases: odd / 2^j has 17-19 exact digits, so the shortest form ties
    for _ in range(600):
        x = (rng.randrange(2**52, 2**53) | 1) / 2 ** rng.randint(1, 6)
        out.append(x if rng.random() < 0.5 else -x)
    while len(out) < 4600:
        kind = rng.random()
        if kind < 0.4:
            x = struct.unpack("<d", struct.pack("<Q", rng.getrandbits(64)))[0]
        elif kind < 0.7:
            x = rng.randint(-10**9, 10**9) / 10 ** rng.randint(0, 12)
        elif kind < 0.85:
            x = float(rng.randint(-2**63, 2**63))
        else:
            x = rng.random() * 10 ** rng.randint(-30, 30)
        if math.isfinite(x):
            out.append(x)
    return out


CHARS = (
    [chr(c) for c in range(0x20, 0x7f)] * 3
    + [chr(c) for c in range(0x00, 0x20)] + ["\x7f"]
    + [chr(c) for c in range(0x80, 0x100, 7)]
    + [" ", " ", "﻿", "￿", "", "̀", "中", "é"]
    + ["\U0001F600", "\U0010FFFF", "\U00010000", "\U0001F4A9"]
)


def rand_str(n_max=12) -> str:
    return "".join(rng.choice(CHARS) for _ in range(rng.randint(0, n_max)))


def rand_float() -> float:
    while True:
        k = rng.random()
        if k < 0.3:
            x = struct.unpack("<d", struct.pack("<Q", rng.getrandbits(64)))[0]
        elif k < 0.7:
            x = rng.randint(-10**6, 10**6) / 10 ** rng.randint(0, 8)
        else:
            x = rng.random() * 10 ** rng.randint(-25, 25)
        if math.isfinite(x):
            return x


def rand_val(depth=0):
    k = rng.random()
    if depth > 3 or k < 0.45:
        s = rng.random()
        if s < 0.1:
            return None
        if s < 0.2:
            return rng.random() < 0.5
        if s < 0.4:
            return rng.choice([rng.randint(-2**63, 2**63 - 1), rng.randint(0, 2**64 - 1), rng.randint(-1000, 1000)])
        if s < 0.65:
            return rand_float()
        return rand_str()
    if k < 0.7:
        return [rand_val(depth + 1) for _ in range(rng.randint(0, 5))]
    return {rand_str(6): rand_val(depth + 1) for _ in range(rng.randint(0, 5))}


def gen_dumps():
    out = []
    for _ in range(800):
        v = rand_val()
        out.append({
            "v": enc(v),
            "compact": json.dumps(v, ensure_ascii=False, allow_nan=False, separators=(",", ":")),
            "spaced": json.dumps(v),
        })
    return out


def fix_surrogates(v):
    if isinstance(v, str):
        return "".join("�" if 0xD800 <= ord(c) <= 0xDFFF else c for c in v)
    if isinstance(v, list):
        return [fix_surrogates(x) for x in v]
    if isinstance(v, dict):
        return {fix_surrogates(k): fix_surrogates(x) for k, x in v.items()}
    return v


def nonfinite(v) -> bool:
    if isinstance(v, float):
        return not math.isfinite(v)
    if isinstance(v, list):
        return any(nonfinite(x) for x in v)
    if isinstance(v, dict):
        return any(nonfinite(x) for x in v.values())
    return False


def loads_case(b: bytes) -> dict:
    case = {"hex": b.hex()}
    try:
        v = json.loads(b)
    except json.JSONDecodeError as e:
        case["err"] = e.msg
        case["pos"] = e.pos
    except (ValueError, RecursionError):
        case["decode"] = True
    else:
        if nonfinite(v):
            case["nonfinite"] = True
        else:
            case["ok"] = enc(fix_surrogates(v))
    return case


MUTATE = list('{}[],:"\\ -+.eE0123456789tfnulNaIy/b\t\n\r') + ["\x00", "\x1f", "é", "\U0001F600"]


def gen_loads():
    fixed = [
        '{"name":"a", }', '[1,]', '[1, ]', '[1,', '{"a":1,', '{"a":', '{"a" 1}', '{"a":1 "b"}', '[1 2]',
        '', ' ', '1.', '-', '"abc', '"a\\x"', '"\\u12"', '"\\u12', '"\\ud800"', '"\\ud800\\u0041"', '{', '[',
        'nul', 'tru', '1 2', '01', '-0', '1e', '1e+', '1.5e3', '[,]', '{,}', '{"a":1,,}', '"\x01"', 'NaN',
        '-Infinity', '[1,\n]', '{"a":1, \t}', '{"a":[1,2,]}', '[{"a":1,}]', '"\\', '"\\u"', 'Infinityx',
        '[-]', '{"a":-}', '﻿1', '[NaN, }', '1e400', '[1e400]', '{"a":1,"b":2,"a":3}', '"\\ud83d\\ude00"',
        '"\\uD83D\\uDE00"', '"\\ud83d\\u0041"', '"\\ud83dx"', '"\\ud83d\\uZZZZ"', '"\\ud83d\\u"',
        '"é\\"x"', 'é', '[1, é]', '{"é": 1, "x" 2}', '"\x7f"', '  {"a" : [ true , false , null ] }  ',
        '18446744073709551615', '18446744073709551616', '-9223372036854775808', '-9223372036854775809',
        '123456789012345678901234567890', '1.0', '1E5', '1e-5', '-0.0', '0.1e1', '[]', '{}', '[[]]', '{"":""}',
        '"\\/\\b\\f\\n\\r\\t\\"\\\\"', '"\\u0000"', 'true false', 'nulll', '[1]x', '"a" "b"',
        '[' * 50 + ']' * 50, '[' * 50, '{"a":' * 30 + '1' + '}' * 30, '9' * 4300, '9' * 4301, '-' + '9' * 4301,
    ]
    out = [loads_case(s.encode("utf-8")) for s in fixed]
    raw = [b"\xff", b'"\xff"', b'{"a":"\xc3"}', b"\xef\xbb\xbf[1]", b"\xef\xbb\xbf\xef\xbb\xbf[1]",
           b"\xe2\x82", b'"\xed\xa0\x80"', b"1\x00", b"\x00\x00\x00\x00", b"\x00\x00", b"a\x00", b"\x00a"]
    out += [loads_case(b) for b in raw]
    for codec in ("utf-16", "utf-16-le", "utf-16-be", "utf-32", "utf-32-le", "utf-32-be", "utf-8-sig"):
        for s in ('{"a": [1, "é\U0001F600"]}', '[1,]', '"x', '12'):
            out.append(loads_case(s.encode(codec)))
    while len(out) < 2500:
        v = rand_val()
        s = json.dumps(v, ensure_ascii=rng.random() < 0.5,
                       indent=rng.choice([None, None, 0, 2, "\t"]),
                       separators=rng.choice([None, (",", ":"), (" , ", " : ")]))
        r = rng.random()
        if r < 0.25:
            pass
        elif r < 0.5 and s:
            i = rng.randrange(len(s))
            s = s[:i] + s[i + 1:]
        elif r < 0.75:
            i = rng.randrange(len(s) + 1)
            s = s[:i] + rng.choice(MUTATE) + s[i:]
        elif s:
            s = s[:rng.randrange(len(s))]
        out.append(loads_case(s.encode("utf-8")))
    return out


def gen_round():
    ties = [0.125, 0.375, 2.675, 1.005, 0.5, 1.5, 2.5, -0.5, -1.5, 0.045, 1.0000000000000002, 123456.785,
            1e-10, 5e-324, 1e300, -0.0, 0.0, 9.995, 99.5, 0.285, 1.115, 2.5e-7, 0.0049999, 1234.5678]
    xs = ties + [rand_float() for _ in range(500)]
    out = []
    for x in xs:
        for n in (0, 1, 2, 3, 4, 6, 10, 17, 300, 400):
            out.append([fbits(x), n, fbits(round(x, n))])
    ints = []
    for x in ties + [rng.randint(-10**6, 10**6) / 4 for _ in range(300)] + [rng.uniform(-1e15, 1e15) for _ in range(300)]:
        ints.append([fbits(x), round(x)])
    return out, ints


EPOCH = dt.datetime(1970, 1, 1, tzinfo=dt.timezone.utc)


def gen_datetimes():
    offsets = [0, 3600, -18000, 19800, 20700, -34200, 50400, -43200, 3661, -61]
    out = []
    for i in range(800):
        if i % 3 == 0:
            us = rng.randint(-62135596800 * 10**6, 253402300799 * 10**6)
        else:
            us = rng.randint(0, 2 * 10**15)
        k = rng.random()
        if k < 0.2:
            us -= us % 10**6
        elif k < 0.5:
            us -= us % 1000
        try:
            base = EPOCH + dt.timedelta(microseconds=us)
            off = rng.choice(offsets)
            d = base.astimezone(dt.timezone(dt.timedelta(seconds=off)))
        except (OverflowError, ValueError):
            continue
        naive = d.replace(tzinfo=None)
        out.append({"us": us, "off": off, "iso": d.isoformat(), "str": str(d),
                    "naive_iso": naive.isoformat(), "naive_str": str(naive), "date": d.date().isoformat()})
    return out


def gen_fromtimestamp():
    out = []
    xs = [0.0, 0.5e-6, 1.5e-6, 2.5e-6, 1.0000005, 1e9 + 0.5e-6, 1700000000.9999995, 1700000000.123456,
          1700000000.1234565, 1.9999996]
    xs += [rng.uniform(0, 4e9) for _ in range(800)] + [rng.randint(0, 4 * 10**9) + rng.randint(0, 10**7) / 10**7 for _ in range(400)]
    for x in xs:
        try:
            d = dt.datetime.fromtimestamp(x, dt.timezone.utc)
        except (OverflowError, OSError, ValueError):
            continue
        out.append([fbits(x), (d - EPOCH) // dt.timedelta(microseconds=1)])
    return out


def gen_qsl():
    fixed = ["", "a", "a=", "=b", "a=1&a=2", "a=1&&b=2", "&", "a=b=c", "a+b=c+d", "a%20b=%2B", "x=%zz", "x=%4",
             "x=%41%42", "x=%%41", "x=%E9", "x=%C3%A9", "x=%C3", "x=%FF%FE", "x=%e2%82%ac", "a;b=1", "k=v#frag",
             "x=%F0%9F%98%80", "x=%ED%A0%80", "x=%E2%82", "q=1+2%2B3", "%61=%62", "a=1&b&c="]
    alphabet = list("ab=&+;%") + ["%20", "%41", "%2B", "%E9", "%C3%A9", "%FF", "%E2%82", "%4", "%g1", "%"]
    out = []
    for s in fixed + ["".join(rng.choice(alphabet) for _ in range(rng.randint(0, 14))) for _ in range(600)]:
        out.append([s, [list(p) for p in urllib.parse.parse_qsl(s, keep_blank_values=True)]])
    return out


def main():
    fl = floats()
    rounds, round_ints = gen_round()
    data = {
        "python": sys.version.split()[0],
        "float_repr": [[fbits(x), repr(x)] for x in fl] + [[fbits(x), repr(x)] for x in (math.inf, -math.inf, math.nan)],
        "dumps": gen_dumps(),
        "loads": gen_loads(),
        "round": rounds,
        "round_int": round_ints,
        "datetimes": gen_datetimes(),
        "fromtimestamp": gen_fromtimestamp(),
        "parse_qsl": gen_qsl(),
        "repr_str": [[s, repr(s)] for s in ["a", "it's", 'say "hi"', "both ' \"", "tab\t", "nl\n", "bs\\", "\x01",
                                             "\x7f", "é", "\U0001F600", "json", "live-us"]],
        "http_phrases": [[s.value, s.phrase] for s in http.HTTPStatus],
    }
    OUT.write_text(json.dumps(data, ensure_ascii=True, separators=(",", ":")) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes) with Python {data['python']}")


if __name__ == "__main__":
    main()

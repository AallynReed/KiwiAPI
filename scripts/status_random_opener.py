"""Decisive test: does a FRESHLY-GENERATED random ephemeral opener make the live
glsserver hold the socket (like a real client), or does the server validate the
opener's content (rejecting anything but a current real one)?

Frida proved real clients send a random 32-byte body every connection
(2000000000 + 32 random bytes). If the server accepts any well-formed ephemeral
key, a random opener will HOLD on a live box -> a version-independent probe that
never goes stale. If random openers get FIN'd while a captured-current one holds,
the server validates content and we can't fake it.

Controls: 7c06 = current captured (known HOLD), 7f0a = old captured (known FIN).
"""
import asyncio
import os
import time

TIMEOUT = 5.0
HOLD = 1.8
SUBSTANTIVE = 256

CURRENT = "20000000003df232536bcb1518164c4685392572b843d0bcbb71be7c06bb098626e23accfb"
OLD     = "20000000003df232536bcb1518164c4685392572b843d0bcbb71be7f0abb098626e23accfb"

def rand_opener() -> str:
    return "2000000000" + os.urandom(32).hex()

TARGETS = [("EU ams-c12-b05", "ams-c12-b05.ams.triongames.com", 6560),
           ("EU 51.77.91.76", "51.77.91.76", 6560)]


async def probe(host, port, opener_hex):
    t0 = time.monotonic()
    try:
        reader, writer = await asyncio.wait_for(
            asyncio.open_connection(host, port), timeout=TIMEOUT)
    except Exception as e:
        return f"CONNECT_FAIL:{type(e).__name__}", 0, 0.0
    try:
        writer.write(bytes.fromhex(opener_hex)); await writer.drain()
        total = 0
        try:
            while True:
                chunk = await asyncio.wait_for(reader.read(4096), timeout=HOLD)
                el = (time.monotonic() - t0) * 1000
                if chunk == b"":
                    return ("FIN(down)" if total <= SUBSTANTIVE else "FIN-after-data"), total, el
                total += len(chunk)
        except asyncio.TimeoutError:
            return "HELD(online)", total, (time.monotonic() - t0) * 1000
        except (ConnectionResetError, ConnectionError) as e:
            return f"RESET:{type(e).__name__}", total, (time.monotonic() - t0) * 1000
    finally:
        try:
            writer.close(); await writer.wait_closed()
        except Exception:
            pass


async def main():
    for label, host, port in TARGETS:
        print(f"\n=== {label} ({host}:{port}) ===")
        cases = [("current 7c06", CURRENT), ("old 7f0a", OLD)]
        cases += [(f"RANDOM #{i+1}", rand_opener()) for i in range(4)]
        for name, op in cases:
            v, total, el = await probe(host, port, op)
            print(f"  {name:14s} {v:18s} bytes={total:<4d} {el:6.0f}ms")
            await asyncio.sleep(1.2)


asyncio.run(main())

"""The deep game probe: a region in maintenance still accepts TCP on 6560 and
answers the glsserver hello, then drops the connection - so connect-only reads
as a false "online". These tests pin the behaviour with real loopback servers
that reproduce each wire pattern we saw in the EU maintenance capture."""
import asyncio

from app.trove import status

# asyncio_mode = "auto" (pyproject) runs the async tests; no module-level mark so
# the sync verdict/overall tests below don't get falsely marked asyncio.

HELLO = "20000000003df232"  # any non-empty hex; the server doesn't validate it


async def _serve(handler):
    server = await asyncio.start_server(handler, "127.0.0.1", 0)
    return server, server.sockets[0].getsockname()[1]


async def test_maintenance_server_drops_after_hello_is_offline():
    """Accept TCP, answer the hello, then close - the EU maintenance signature."""
    async def handler(reader, writer):
        await reader.read(64)              # consume the hello
        writer.write(b"\x10\x00\x00\x00")  # tiny handshake reply...
        await writer.drain()
        writer.close()                     # ...then FIN right away => maintenance

    server, port = await _serve(handler)
    async with server:
        res = await status._probe_game(
            "127.0.0.1", port, deep=True, hello_hex=HELLO, hold_seconds=0.5,
        )
    assert res["online"] is False
    assert res["probe"] == "glsserver"
    assert res["error"] == "glsserver_dropped"


async def test_playable_server_holds_connection_is_online():
    """Accept TCP and keep the session socket open - a live region."""
    async def handler(reader, writer):
        await reader.read(64)
        writer.write(b"\x10\x00\x00\x00")
        await writer.drain()
        try:
            await asyncio.sleep(5)         # hold it open past the probe window
        except asyncio.CancelledError:
            pass

    server, port = await _serve(handler)
    async with server:
        res = await status._probe_game(
            "127.0.0.1", port, deep=True, hello_hex=HELLO, hold_seconds=0.3,
        )
    assert res["online"] is True
    assert res["probe"] == "glsserver"


async def test_server_that_sends_real_data_then_closes_is_online():
    """A close AFTER a substantial reply (a data-bearing directory response) is
    NOT the maintenance signature (which is a near-empty fast FIN) → online."""
    async def handler(reader, writer):
        await reader.read(64)
        writer.write(b"\x10\x00\x00\x00" + b"\xab" * 600)  # > _GLS_SUBSTANTIVE_BYTES
        await writer.drain()
        writer.close()

    server, port = await _serve(handler)
    async with server:
        res = await status._probe_game(
            "127.0.0.1", port, deep=True, hello_hex=HELLO, hold_seconds=0.5,
        )
    assert res["online"] is True
    assert res["probe"] == "glsserver"


async def test_deep_probe_disabled_falls_back_to_connect_only():
    """deep=False reproduces the old behaviour: TCP accepted == online, even for
    a server that would drop us under the deep probe."""
    async def handler(reader, writer):
        await reader.read(64)
        writer.close()

    server, port = await _serve(handler)
    async with server:
        res = await status._probe_game(
            "127.0.0.1", port, deep=False, hello_hex=HELLO, hold_seconds=0.3,
        )
    assert res["online"] is True
    assert res["probe"] == "tcp"


async def test_empty_hello_falls_back_to_connect_only():
    async def handler(reader, writer):
        await reader.read(64)
        writer.close()

    server, port = await _serve(handler)
    async with server:
        res = await status._probe_game(
            "127.0.0.1", port, deep=True, hello_hex="", hold_seconds=0.3,
        )
    assert res["online"] is True
    assert res["probe"] == "tcp"


def test_random_opener_is_well_formed_and_fresh():
    """The opener is a 5-byte frame header + 32 random bytes, regenerated each call
    (the real client uses a fresh per-connection ephemeral key, proven via Frida)."""
    a = status._random_opener()
    b = status._random_opener()
    assert len(a) == 37 and a[:5] == b"\x20\x00\x00\x00\x00"
    assert len(b) == 37 and b[:5] == b"\x20\x00\x00\x00\x00"
    assert a != b  # fresh random body each time


async def test_random_opener_holds_is_online_and_sends_fresh_37b():
    """random_opener mode: send a fresh well-formed opener; a holding server (live)
    reads online, and the bytes on the wire are a 37-byte 0x20-framed opener -
    never the (stale-prone) captured hex."""
    received = []

    async def handler(reader, writer):
        received.append(await reader.read(64))
        try:
            await asyncio.sleep(5)            # hold open => live
        except asyncio.CancelledError:
            pass

    server, port = await _serve(handler)
    async with server:
        res = await status._probe_game(
            "127.0.0.1", port, deep=True, hello_hex="deadbeef",  # content ignored
            hold_seconds=0.3, random_opener=True,
        )
    assert res["online"] is True
    assert res["probe"] == "glsserver"
    assert res["opener"] == "random"
    assert len(received) == 1
    assert received[0][:5] == b"\x20\x00\x00\x00\x00" and len(received[0]) == 37
    assert received[0].hex() != "deadbeef"   # the hex content was NOT replayed


async def test_random_opener_dropped_is_offline():
    """A maintenance server (near-empty reply then FIN) still reads down under the
    random opener."""
    async def handler(reader, writer):
        await reader.read(64)
        writer.write(b"\x10\x00\x00\x00")
        await writer.drain()
        writer.close()

    server, port = await _serve(handler)
    async with server:
        res = await status._probe_game(
            "127.0.0.1", port, deep=True, hello_hex="20", hold_seconds=0.5,
            random_opener=True,
        )
    assert res["online"] is False
    assert res["error"] == "glsserver_dropped"


async def test_random_opener_empty_flag_is_connect_only():
    """An empty hello_hex disables the deep probe for that env (the PTS convention),
    even in random mode → connect-only."""
    async def handler(reader, writer):
        await reader.read(64)
        writer.close()

    server, port = await _serve(handler)
    async with server:
        res = await status._probe_game(
            "127.0.0.1", port, deep=True, hello_hex="", hold_seconds=0.3,
            random_opener=True,
        )
    assert res["online"] is True
    assert res["probe"] == "tcp"


def test_verdict_is_binary_online_or_down():
    """No 'maintenance' state: online only when auth AND game are up, else down."""
    from app.trove.status import _verdict
    assert _verdict(True, True) == "online"
    assert _verdict(True, False) == "down"   # auth up, game socket dead
    assert _verdict(False, True) == "down"   # login gateway down
    assert _verdict(False, False) == "down"


def test_overall_is_online_only_when_all_live_regions_online():
    from app.trove.status import _overall
    assert _overall({"eu": {"status": "online"}, "us": {"status": "online"}}) == "online"
    # Any live region not online → overall down (consumers read partial-vs-full
    # from the per-region detail).
    assert _overall({"eu": {"status": "down"}, "us": {"status": "online"}}) == "down"
    assert _overall({"eu": {"status": "down"}, "us": {"status": "down"}}) == "down"
    assert _overall({}) == "unknown"


async def test_refused_connection_is_offline():
    """A closed port (refused/unreachable) is offline regardless of deep probe."""
    server, port = await _serve(lambda r, w: None)
    server.close()
    await server.wait_closed()  # port now free -> connect is refused
    res = await status._probe_game(
        "127.0.0.1", port, deep=True, hello_hex=HELLO, hold_seconds=0.3,
    )
    assert res["online"] is False
    assert "probe" not in res          # never connected, so no app-layer verdict
    assert res["error"]                # carries the connect failure (e.g. refused)

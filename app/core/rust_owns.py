"""Background loops the Rust server has taken over.

``RUST_OWNS`` is a comma list of the loop names below, read once at import. A
loop named there never starts in Python, so it never runs twice while the port
is in flight (native/PORT_PLAN.md). Unset or empty changes nothing.

    usage                usage-event recorder; owned = Python records no usage
    email_outbox         email outbox delivery worker
    news                 Trove news relay
    feeds                Twitch / YouTube / Bilibili feeds
    trove_events         Trovesaurus event calendar
    mod_stats            mod statistics snapshot
    steam_workshop       Steam Workshop catalog
    chaos                Chaos Chest relay
    status_prober        Trove server status prober
    delves               delve rotation refresher
    btt_releases         BetterTroveTools releases relay
    update_archiver      game-file version archiver
    leaderboards_warmer  leaderboards warmer (detection, activity, renames,
                         duplicates, page caches, cold tiering)
    giveaways            giveaway auto-open and draw
    event_listener       events-bus subscriber feeding this process's SSE streams
    event_watch          events-bus safety-net re-publisher
    event_scheduler      time-driven rotation events
    webhooks             webhook delivery queue consumer
    dm_subs              DM subscription delivery queue consumer
    maintenance          daily token and rate-limit maintenance sweep
"""

import logging
import os

logger = logging.getLogger("kiwi.rust_owns")

LOOPS = frozenset({
    "usage", "email_outbox", "news", "feeds", "trove_events", "mod_stats",
    "steam_workshop", "chaos", "status_prober", "delves", "btt_releases",
    "update_archiver", "leaderboards_warmer", "giveaways", "event_listener",
    "event_watch", "event_scheduler", "webhooks", "dm_subs", "maintenance",
})


def _parse(raw: str) -> frozenset[str]:
    names = {n.strip().lower() for n in raw.split(",") if n.strip()}
    if unknown := sorted(names - LOOPS):
        logger.warning("RUST_OWNS: unknown loop name(s): %s", ", ".join(unknown))
    return frozenset(names & LOOPS)


OWNED: frozenset[str] = _parse(os.environ.get("RUST_OWNS", ""))


def rust_owns(loop: str) -> bool:
    """True when ``loop`` runs in the Rust server, so Python must not start it."""
    if loop not in LOOPS:
        raise ValueError(f"unknown background loop {loop!r}")
    if loop in OWNED:
        logger.info("%s: owned by the Rust server, not started here", loop)
        return True
    return False

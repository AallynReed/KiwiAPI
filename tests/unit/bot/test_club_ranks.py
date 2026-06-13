"""Club rank promote/demote transitions (pure, no DB).

``promote_rank`` / ``demote_rank`` decide the rank a member moves INTO when the
dashboard promotes/demotes them. The rules under test:
  - one step at a time toward the next LINKED rank, skipping unlinked ranks;
  - president is never a promotion target (set in-game only) - VP is the ceiling;
  - a president CAN be demoted (the no-president rule only blocks promoting TO it);
  - member is the demotion floor.
``linked`` is the set of ranks the club has mapped to a Discord role.
"""
from app.bot.models import CLUB_RANKS, demote_rank, promote_rank

ALL = set(CLUB_RANKS)


# ── promote ──────────────────────────────────────────────────────────────────

def test_member_promotes_to_captain_when_linked():
    assert promote_rank("member", ALL) == "captain"


def test_member_skips_unlinked_captain_to_officer():
    # captain not linked -> skip it and promote to officer instead
    assert promote_rank("member", {"member", "officer", "vice_president"}) == "officer"


def test_member_skips_to_vice_president_when_only_it_is_linked_above():
    assert promote_rank("member", {"member", "vice_president"}) == "vice_president"


def test_vice_president_cannot_be_promoted_president_excluded():
    # president is a valid link but never a promotion target
    assert promote_rank("vice_president", ALL) is None


def test_president_cannot_be_promoted():
    assert promote_rank("president", ALL) is None


def test_promote_with_no_higher_linked_rank_is_none():
    # captain is the only rank above member that's linked... but here nothing is
    assert promote_rank("member", {"member"}) is None


# ── demote ───────────────────────────────────────────────────────────────────

def test_captain_demotes_to_member():
    assert demote_rank("captain", ALL) == "member"


def test_officer_skips_unlinked_captain_to_member():
    assert demote_rank("officer", {"officer", "member"}) == "member"


def test_president_can_be_demoted():
    assert demote_rank("president", ALL) == "vice_president"


def test_member_cannot_be_demoted_floor():
    assert demote_rank("member", ALL) is None


def test_demote_with_no_lower_linked_rank_is_none():
    assert demote_rank("officer", {"officer", "president"}) is None


# ── guards ───────────────────────────────────────────────────────────────────

def test_unknown_rank_returns_none_both_ways():
    assert promote_rank("emperor", ALL) is None
    assert demote_rank("emperor", ALL) is None


def test_full_ladder_round_trip():
    # member -> captain -> officer -> vice_president, then back down to member
    assert promote_rank("member", ALL) == "captain"
    assert promote_rank("captain", ALL) == "officer"
    assert promote_rank("officer", ALL) == "vice_president"
    assert demote_rank("vice_president", ALL) == "officer"
    assert demote_rank("officer", ALL) == "captain"
    assert demote_rank("captain", ALL) == "member"

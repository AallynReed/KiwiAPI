# pvpplayercounterui.swf
> A PvP HUD element that displays a player count or score counter during PvP matches in Trove. The SWF contains no ActionScript of any kind: it is a text field and an icon that the engine fills from outside.

**Document/main class:** None (the SWF has no `DoABC` tag)
**SWF-specific classes:** 0

---

## Notes

The `scripts/` folder for this SWF is empty because the file carries no ActionScript in any form — there is no `DoABC` tag, so there are no classes, no shared framework copies and no frame scripts. It is a 120×100, 24 fps, single-frame stage with two root children:

- `counterText` (chid 2, a `DefineEditText` in Open Sans) at depth 1. With no code in the SWF, whatever it shows is written into it by the engine, by instance name — the same name `pvphud.swf` declares for its counter.
- An unnamed shape (chid 4) at depth 2, filled with a person-figure bitmap (chid 3) that is byte-identical to the bitmap behind `pvphud.swf`'s `killCounterIcon`.

There are no `ExternalInterface` callbacks or outbound calls, no translate keys and no Iggy runtime dependencies.

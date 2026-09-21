/* How a requested picture looks in Zakros UI, drawn on canvas at in-game size and
   shown zoomed 2x.

   Shared by the request page (/zakros-ui-requests) and the dev portal's custom art
   queue, which serves this same file. Sizes and colours are the Chat and Nameplate
   defaults, read out of their sources:

     profile picture  the 15px box on a 28px whisper tab (Chat ui/Face.as)
     club picture     38px on the 39px channel strip (Chat ui/Rail.as)
     club banner      13px tall in front of a message - the tag size, 13 - 4, plus 4
                      (Chat ui/Line.as); in the 26px box under a 30px bold name on a
                      nameplate, left-aligned with the name (Nameplate ui/Banner.as); and
                      21px tall in place of the club's name on a 62px club row
                      (Clubs ClubTile.as)
     both, in Clubs   a club row draws the picture in a 26px box inside the 30px bordered
                      square, and the banner where the name would be (Clubs ClubTile.as)

   The chrome around each picture is a likeness of the default look, not the game's
   own rendering; the picture itself is scaled exactly as the mods scale it. The canvas
   is drawn at the zoom rather than enlarged after the fact, because the mods reach these
   three lanes through a filtered bitmap fill and Iggy puts the whole screen through
   globalScale - so in game the picture is resampled smoothly, never by whole pixels, and
   a nearest-neighbour zoom here would show a harder edge than the game ever draws. */
(function () {
    "use strict";

    var ZOOM = 2;
    var FONT = "'Open Sans', Inter, system-ui, sans-serif";
    var BACKDROP = "#27303A";
    var PANEL = "rgba(11, 12, 14, 0.745)";
    var VALUE = "#E8ECF1";
    var LABEL = "#8A929C";
    var MAIN = "#C1C6CD";
    var STAMP = "#6E747C";
    var WHISPER = "#DC70F0";
    var WHISPER_LIT = "#E69BF4";
    var BORDER = "rgba(138, 146, 156, 0.3)";

    function surface(host, w, h, panel) {
        var c = document.createElement("canvas");
        c.width = w * ZOOM;
        c.height = h * ZOOM;
        c.style.width = w * ZOOM + "px";
        c.style.height = h * ZOOM + "px";
        host.appendChild(c);
        var ctx = c.getContext("2d");
        ctx.scale(ZOOM, ZOOM);
        ctx.imageSmoothingEnabled = true;
        ctx.imageSmoothingQuality = "high";
        ctx.fillStyle = BACKDROP;
        ctx.fillRect(0, 0, w, h);
        if (panel) {
            ctx.fillStyle = PANEL;
            ctx.fillRect(0, 0, w, h);
        }
        return ctx;
    }

    function write(ctx, s, x, y, size, color, weight) {
        ctx.font = (weight || 400) + " " + size + "px " + FONT;
        ctx.fillStyle = color;
        ctx.textBaseline = "middle";
        ctx.fillText(s, x, y);
        return ctx.measureText(s).width;
    }

    function whisperTab(host, src, r, name) {
        var ctx = surface(host, 280, 28, true);
        var x = 8 + write(ctx, "ALL", 8, 14, 11, MAIN, 600) + 12;
        ctx.fillStyle = BORDER;
        ctx.fillRect(x - 1, 4, 1, 20);
        ctx.font = "400 11px " + FONT;
        var wide = 8 + 15 + 6 + ctx.measureText(name).width + 4 + 8;
        ctx.fillStyle = "rgba(220, 112, 240, 0.16)";
        ctx.fillRect(x, 0, wide, 28);
        ctx.fillStyle = WHISPER;
        ctx.fillRect(x, 27, wide, 1);
        ctx.drawImage(src, r.x, r.y, r.w, r.h, x + 8, 6, 15, 15);
        write(ctx, name, x + 8 + 15 + 6, 14, 11, WHISPER_LIT);
    }

    function channelStrip(host, src, r) {
        var box = 38;
        var ctx = surface(host, 240, box * 3, true);
        [["#73B8FF", "1"], ["#4CC38A", "2"]].forEach(function (row, i) {
            ctx.globalAlpha = 0.4;
            ctx.fillStyle = row[0];
            ctx.fillRect(6, i * box + 6, box - 12, box - 12);
            ctx.globalAlpha = 1;
            ctx.textAlign = "center";
            write(ctx, row[1], box / 2, i * box + box / 2, 13, VALUE, 600);
            ctx.textAlign = "left";
        });
        ctx.fillStyle = "rgba(232, 236, 241, 0.06)";
        ctx.fillRect(0, box * 2, 39, box);
        ctx.drawImage(src, r.x, r.y, r.w, r.h, 0, box * 2, box, box);
        ctx.fillStyle = BORDER;
        ctx.fillRect(39, 0, 1, box * 3);
        write(ctx, "12:34", 48, box * 2 + box / 2, 11, STAMP);
        write(ctx, "Player  hello", 84, box * 2 + box / 2, 13, VALUE);
    }

    function chatLine(host, src, r) {
        var high = 13;
        var wide = Math.round(high * r.w / r.h);
        var ctx = surface(host, Math.max(240, 150 + wide), 24, true);
        var x = 8 + write(ctx, "12:34", 8, 12, 11, STAMP) + 8;
        ctx.drawImage(src, r.x, r.y, r.w, r.h, x, 12 - high / 2, wide, high);
        x += wide + 8;
        x += write(ctx, "Player", x, 12, 13, VALUE, 600) + 8;
        write(ctx, "hello", x, 12, 13, VALUE);
    }

    function nameplate(host, src, r) {
        var w = 280;
        var scale = Math.min(26 / r.h, (w - 16) / r.w);
        var ctx = surface(host, w, 50 + Math.ceil(r.h * scale) + 8, false);
        ctx.font = "700 30px " + FONT;
        var left = Math.round((w - ctx.measureText("Player").width) / 2);
        ctx.shadowColor = "#000000";
        ctx.shadowOffsetX = ctx.shadowOffsetY = Math.SQRT2;
        write(ctx, "Player", left, 26, 30, "#FFFFFF", 700);
        ctx.shadowColor = "transparent";
        ctx.drawImage(src, r.x, r.y, r.w, r.h, Math.max(8, left), 50, r.w * scale, r.h * scale);
    }

    function clubRow(host, src, r, name, asBanner) {
        var w = 280;
        var ctx = surface(host, w, 62, true);
        var badge = 30;
        var box = badge - 4;
        var top = (62 - badge) / 2;
        var scale = Math.min(box / r.h, box / r.w);
        ctx.strokeStyle = BORDER;
        ctx.lineWidth = 1;
        ctx.strokeRect(14.5, top + 0.5, badge - 1, badge - 1);
        var left = 14 + badge + 14;
        if (asBanner) {
            write(ctx, name.charAt(0).toUpperCase(), 14 + badge / 2 - 4, top + badge / 2, 13, MAIN, 600);
            var high = 17 + 4;
            var room = w - left - 16;
            var bs = Math.min(high / r.h, room / r.w);
            ctx.drawImage(src, r.x, r.y, r.w, r.h, left, 10 + (23 - r.h * bs) / 2,
                          r.w * bs, r.h * bs);
        } else {
            ctx.drawImage(src, r.x, r.y, r.w, r.h,
                          14 + (badge - r.w * scale) / 2, top + (badge - r.h * scale) / 2,
                          r.w * scale, r.h * scale);
            write(ctx, name, left, 10 + 11, 17, VALUE, 600);
        }
        write(ctx, "12 members", left, 62 - 18, 10, LABEL);
    }

    function render(host, lane, src, rect, name) {
        host.textContent = "";
        if (!src || !rect || rect.w <= 0 || rect.h <= 0) return;
        var club = (name || "").trim() || "Club";
        if (lane === "pfp") {
            whisperTab(host, src, rect, (name || "").trim() || "Player");
        } else if (lane === "club") {
            channelStrip(host, src, rect);
            clubRow(host, src, rect, club, false);
        } else {
            chatLine(host, src, rect);
            nameplate(host, src, rect);
            clubRow(host, src, rect, club, true);
        }
    }

    window.ArtMock = { render: render };
})();

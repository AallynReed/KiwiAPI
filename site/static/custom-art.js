/* /zakros-ui-requests - ask for a picture in the Zakros UI Chat and Nameplate mods.

   A player request carries a profile picture; a club request carries a club
   picture, a club banner, or both. A profile or club picture is square and a
   banner is from square up to five times as wide as it is tall. A picture that
   doesn't fit the shape, or is too big to send, opens a cropper in place and is
   sent as the cropped PNG, no bigger than 768px on its long side. The API checks
   the same rules again.

   Every static string is marked in the template; the player/club wording is two
   marked elements toggled by `hidden` rather than text swapped in here, so i18n.js
   keeps hold of both. Messages built here go through t(), and ttf() for the ones
   with a number in them - translate first, then fill.

   A captcha token is spent the moment the server verifies it, so every failed
   send resets the widget. */
(function () {
    "use strict";

    var OUTPUT_MAX = 768;
    var MIN_SIDE = 8;

    var config = { max_bytes: 3 * 1024 * 1024, widest: 5, captcha_sitekey: null, captcha_provider: "turnstile" };
    var captcha = { lib: null, id: null, token: null };
    var busy = false;

    function t(s) { return window.BTTi18n ? window.BTTi18n.t(s) : s; }
    function ttf(s, values) {
        return t(s).replace(/\{(\w+)\}/g, function (m, key) { return key in values ? values[key] : m; });
    }

    var form = document.getElementById("art-form");
    var doneEl = document.getElementById("art-done");
    var doneMsg = document.getElementById("art-done-msg");
    var nameEl = document.getElementById("art-name");
    var emailEl = document.getElementById("art-email");
    var noteEl = document.getElementById("art-note");
    var captchaEl = document.getElementById("art-captcha");
    var statusEl = document.getElementById("art-status");
    var goBtn = document.getElementById("art-go");

    function clamp(v, lo, hi) { return Math.max(lo, Math.min(hi, v)); }
    function humanSize(bytes) {
        return bytes >= 1048576 ? (bytes / 1048576).toFixed(1) + " MB" : Math.max(1, Math.round(bytes / 1024)) + " KB";
    }

    function say(message, kind) {
        statusEl.textContent = message || "";
        statusEl.className = "drop-status" + (kind ? " is-" + kind : "");
        statusEl.hidden = !message;
    }

    // ── One picture slot: pick, check, crop, preview ─────────────────────────

    function makeSlot(root, square, laneOf) {
        var q = function (sel) { return root.querySelector(sel); };
        var mockWrap = q(".art-mock-wrap");
        var mock = q(".art-mock");
        var mocked = null, accepted = null, mockDue = false;
        var input = q("input[type=file]");
        var zone = q(".drop-zone");
        var crop = q(".art-crop");
        var canvas = q(".art-crop-canvas");
        var ctx = canvas.getContext("2d");
        var ready = q(".art-ready");
        var preview = q(".art-preview");
        var info = q(".art-ready-info");
        var status = q(".art-slot-status");

        var slot = { file: null };
        var image = null, sourceName = "", sourceUrl = null, previewUrl = null;
        var rect = null, scale = 1, drag = null;

        function note(message, kind) {
            status.textContent = message || "";
            status.className = "drop-status art-slot-status" + (kind ? " is-" + kind : "");
            status.hidden = !message;
        }

        function fits(w, h) { return square ? w === h : h <= w && w <= config.widest * h; }

        function show(which) {
            zone.hidden = which !== "zone";
            crop.hidden = which !== "crop";
            ready.hidden = which !== "ready";
        }

        function mockOf(src, rect) {
            mocked = src ? { src: src, rect: rect } : null;
            mockWrap.hidden = !mocked || !window.ArtMock;
            if (mockWrap.hidden || mockDue) return;
            mockDue = true;
            requestAnimationFrame(function () {
                mockDue = false;
                if (mocked) window.ArtMock.render(mock, laneOf(), mocked.src, mocked.rect, nameEl.value);
            });
        }

        function dropPreview() {
            if (previewUrl && previewUrl !== sourceUrl) URL.revokeObjectURL(previewUrl);
            previewUrl = null;
        }

        function reset() {
            dropPreview();
            if (sourceUrl) URL.revokeObjectURL(sourceUrl);
            sourceUrl = null; image = null; rect = null; slot.file = null; accepted = null;
            input.value = "";
            show("zone");
            note("");
            mockOf(null);
        }

        function accept(file, w, h, url, src) {
            dropPreview();
            previewUrl = url;
            slot.file = file;
            preview.src = url;
            info.textContent = w + " × " + h + " · " + humanSize(file.size);
            show("ready");
            accepted = { src: src, rect: { x: 0, y: 0, w: w, h: h } };
            mockOf(accepted.src, accepted.rect);
        }

        function load(file) {
            if (!file) return;
            if (!/^image\/(png|jpeg|webp|gif)$/.test(file.type)) {
                note(t("Pick a PNG, JPEG, WebP or GIF picture."), "bad");
                input.value = "";
                return;
            }
            var url = URL.createObjectURL(file);
            var img = new Image();
            img.onload = function () {
                reset();
                sourceUrl = url; image = img; sourceName = file.name;
                var w = img.naturalWidth, h = img.naturalHeight;
                var size = w + " × " + h;
                if (fits(w, h) && file.size <= config.max_bytes) {
                    accept(file, w, h, url, img);
                    return;
                }
                openCrop();
                if (fits(w, h)) {
                    note(ttf("This picture is over the {mb} MB limit. Crop it here and it's sent smaller.", { mb: Math.round(config.max_bytes / 1048576) }), "bad");
                } else if (square) {
                    note(ttf("This picture is {size}, and it has to be square. Crop it here.", { size: size }), "bad");
                } else {
                    note(ttf("This picture is {size}, and it has to be from square up to {widest} times as wide as it is tall. Crop it here.", { size: size, widest: config.widest }), "bad");
                }
            };
            img.onerror = function () {
                URL.revokeObjectURL(url);
                note(t("That file couldn't be read as a picture."), "bad");
                input.value = "";
            };
            img.src = url;
        }

        function initRect() {
            var w = image.naturalWidth, h = image.naturalHeight, cw, ch;
            if (square) { cw = ch = Math.min(w, h); }
            else { cw = Math.min(w, config.widest * h); ch = Math.min(h, cw); }
            rect = { x: (w - cw) / 2, y: (h - ch) / 2, w: cw, h: ch };
        }

        function openCrop() {
            show("crop");
            initRect();
            var w = image.naturalWidth, h = image.naturalHeight;
            var room = Math.max(160, crop.clientWidth || root.clientWidth || 480);
            scale = Math.min(room / w, 420 / h);
            canvas.width = Math.max(1, Math.round(w * scale));
            canvas.height = Math.max(1, Math.round(h * scale));
            draw();
            canvas.focus();
        }

        function corners() {
            return [[rect.x, rect.y], [rect.x + rect.w, rect.y],
                    [rect.x, rect.y + rect.h], [rect.x + rect.w, rect.y + rect.h]];
        }

        function draw() {
            var W = canvas.width, H = canvas.height, s = scale;
            var x = rect.x * s, y = rect.y * s, w = rect.w * s, h = rect.h * s;
            ctx.clearRect(0, 0, W, H);
            ctx.imageSmoothingEnabled = s < 1;
            ctx.drawImage(image, 0, 0, W, H);
            ctx.fillStyle = "rgba(0, 0, 0, 0.62)";
            ctx.fillRect(0, 0, W, y);
            ctx.fillRect(0, y + h, W, H - y - h);
            ctx.fillRect(0, y, x, h);
            ctx.fillRect(x + w, y, W - x - w, h);
            ctx.strokeStyle = "#569cff";
            ctx.lineWidth = 2;
            ctx.strokeRect(x + 1, y + 1, Math.max(0, w - 2), Math.max(0, h - 2));
            ctx.fillStyle = "#569cff";
            corners().forEach(function (c) { ctx.fillRect(c[0] * s - 5, c[1] * s - 5, 10, 10); });
            mockOf(image, whole());
        }

        function toSource(e) {
            var box = canvas.getBoundingClientRect();
            var k = canvas.width / box.width;
            return { x: (e.clientX - box.left) * k / scale, y: (e.clientY - box.top) * k / scale, k: k };
        }

        canvas.addEventListener("pointerdown", function (e) {
            if (!rect) return;
            var p = toSource(e);
            var reach = 16 * p.k / scale;
            var list = corners();
            var hit = -1;
            list.forEach(function (c, i) {
                if (hit < 0 && Math.abs(c[0] - p.x) <= reach && Math.abs(c[1] - p.y) <= reach) hit = i;
            });
            if (hit >= 0) {
                drag = { mode: "size", ax: list[3 - hit][0], ay: list[3 - hit][1] };
            } else if (p.x >= rect.x && p.x <= rect.x + rect.w && p.y >= rect.y && p.y <= rect.y + rect.h) {
                drag = { mode: "move", dx: p.x - rect.x, dy: p.y - rect.y };
            } else {
                return;
            }
            canvas.setPointerCapture(e.pointerId);
            e.preventDefault();
        });

        canvas.addEventListener("pointermove", function (e) {
            if (!drag) return;
            var W = image.naturalWidth, H = image.naturalHeight, p = toSource(e);
            if (drag.mode === "move") {
                rect.x = clamp(p.x - drag.dx, 0, W - rect.w);
                rect.y = clamp(p.y - drag.dy, 0, H - rect.h);
            } else {
                var px = clamp(p.x, 0, W), py = clamp(p.y, 0, H);
                var roomW = px >= drag.ax ? W - drag.ax : drag.ax;
                var roomH = py >= drag.ay ? H - drag.ay : drag.ay;
                var w = Math.abs(px - drag.ax), h = Math.abs(py - drag.ay);
                if (square) {
                    w = h = Math.min(Math.max(w, h), roomW, roomH);
                } else {
                    if (w < h) { if (h <= roomW) w = h; else h = w; }
                    if (w > config.widest * h) { if (w / config.widest <= roomH) h = w / config.widest; else w = config.widest * h; }
                }
                h = Math.max(h, MIN_SIDE);
                w = square ? h : Math.max(w, h);
                rect.w = w; rect.h = h;
                rect.x = clamp(px >= drag.ax ? drag.ax : drag.ax - w, 0, W - w);
                rect.y = clamp(py >= drag.ay ? drag.ay : drag.ay - h, 0, H - h);
            }
            draw();
        });

        function endDrag() { drag = null; }
        canvas.addEventListener("pointerup", endDrag);
        canvas.addEventListener("pointercancel", endDrag);

        canvas.addEventListener("keydown", function (e) {
            var keys = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
            var d = keys[e.key];
            if (!d || !rect) return;
            e.preventDefault();
            var W = image.naturalWidth, H = image.naturalHeight;
            var step = Math.max(1, Math.round(Math.max(W, H) / 100));
            if (!e.shiftKey) {
                rect.x = clamp(rect.x + d[0] * step, 0, W - rect.w);
                rect.y = clamp(rect.y + d[1] * step, 0, H - rect.h);
            } else {
                var grow = d[0] > 0 || d[1] < 0 ? step : -step;
                if (square) {
                    rect.w = rect.h = clamp(rect.w + grow, MIN_SIDE, Math.min(W - rect.x, H - rect.y));
                } else if (d[0] !== 0) {
                    rect.w = clamp(rect.w + grow, rect.h, Math.min(W - rect.x, config.widest * rect.h));
                } else {
                    rect.h = clamp(rect.h + grow, Math.max(MIN_SIDE, Math.ceil(rect.w / config.widest)), Math.min(H - rect.y, rect.w));
                }
            }
            draw();
        });

        function whole() {
            var W = image.naturalWidth, H = image.naturalHeight;
            var x = clamp(Math.round(rect.x), 0, W - 1), y = clamp(Math.round(rect.y), 0, H - 1);
            var w = Math.min(Math.round(rect.w), W - x), h = Math.min(Math.round(rect.h), H - y);
            if (square) w = h = Math.min(w, h);
            else { if (w < h) h = w; if (w > config.widest * h) w = config.widest * h; }
            return { x: x, y: y, w: Math.max(1, w), h: Math.max(1, h) };
        }

        function useCrop() {
            var r = whole();
            var k = Math.min(1, OUTPUT_MAX / Math.max(r.w, r.h));
            var ow = Math.max(1, Math.round(r.w * k)), oh = Math.max(1, Math.round(r.h * k));
            if (square) ow = oh = Math.min(ow, oh);
            else { if (ow < oh) oh = ow; if (ow > config.widest * oh) ow = config.widest * oh; }
            var out = document.createElement("canvas");
            out.width = ow;
            out.height = oh;
            out.getContext("2d").drawImage(image, r.x, r.y, r.w, r.h, 0, 0, ow, oh);
            out.toBlob(function (blob) {
                if (!blob) { note(t("That crop couldn't be made. Try another picture."), "bad"); return; }
                if (blob.size > config.max_bytes) { note(t("That crop is still over the limit. Pick a smaller area."), "bad"); return; }
                var stem = sourceName.replace(/\.[^.]*$/, "") || "picture";
                accept(new File([blob], stem + "-crop.png", { type: "image/png" }), ow, oh, URL.createObjectURL(blob), out);
                note("");
            }, "image/png");
        }

        input.addEventListener("change", function () { load(input.files && input.files[0]); });

        ["dragenter", "dragover"].forEach(function (name) {
            zone.addEventListener(name, function (e) { e.preventDefault(); zone.classList.add("dragover"); });
        });
        ["dragleave", "drop"].forEach(function (name) {
            zone.addEventListener(name, function (e) {
                if (name === "dragleave" && zone.contains(e.relatedTarget)) return;
                e.preventDefault();
                zone.classList.remove("dragover");
            });
        });
        zone.addEventListener("drop", function (e) {
            var files = e.dataTransfer && e.dataTransfer.files;
            if (files && files.length) load(files[0]);
        });

        q(".art-crop-use").addEventListener("click", useCrop);
        q(".art-crop-cancel").addEventListener("click", function () {
            if (slot.file) {
                show("ready");
                note("");
                mockOf(accepted.src, accepted.rect);
            } else {
                reset();
            }
        });
        q(".art-recrop").addEventListener("click", function () { if (image) { openCrop(); note(""); } });
        q(".art-remove").addEventListener("click", reset);

        slot.cropping = function () { return !crop.hidden; };
        slot.refresh = function () { if (mocked) mockOf(mocked.src, mocked.rect); };
        return slot;
    }

    var pfp = makeSlot(document.getElementById("art-slot-pfp"), true,
                       function () { return kind() === "club" ? "club" : "pfp"; });
    var banner = makeSlot(document.getElementById("art-slot-banner"), false,
                          function () { return "banner"; });

    nameEl.addEventListener("input", function () { pfp.refresh(); });

    // ── Player or club ───────────────────────────────────────────────────────

    function kind() {
        var checked = form.querySelector("input[name=kind]:checked");
        return checked ? checked.value : "player";
    }

    function syncKind() {
        var club = kind() === "club";
        form.querySelectorAll(".art-if-player").forEach(function (el) { el.hidden = club; });
        form.querySelectorAll(".art-if-club").forEach(function (el) { el.hidden = !club; });
        pfp.refresh();
        say("");
    }

    form.querySelectorAll("input[name=kind]").forEach(function (radio) {
        radio.addEventListener("change", syncKind);
    });

    // ── Captcha ──────────────────────────────────────────────────────────────

    function mountCaptcha() {
        if (!config.captcha_sitekey) return;
        var hc = config.captcha_provider === "hcaptcha";
        var script = document.createElement("script");
        script.src = hc ? "https://js.hcaptcha.com/1/api.js?render=explicit"
                        : "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit";
        script.async = true;
        script.onload = function () {
            captcha.lib = hc ? window.hcaptcha : window.turnstile;
            if (!captcha.lib) return;
            captcha.id = captcha.lib.render(captchaEl, {
                sitekey: config.captcha_sitekey,
                theme: "dark",
                callback: function (token) { captcha.token = token; },
                "expired-callback": function () { captcha.token = null; }
            });
        };
        script.onerror = function () {
            say(t("The captcha couldn't load. Turn off anything blocking it and reload the page."), "bad");
        };
        document.head.appendChild(script);
    }

    function resetCaptcha() {
        captcha.token = null;
        if (captcha.lib && captcha.id != null) {
            try { captcha.lib.reset(captcha.id); } catch (e) { /* widget already gone */ }
        }
    }

    // ── Send ─────────────────────────────────────────────────────────────────

    function finish(message) {
        busy = false;
        goBtn.disabled = false;
        form.classList.remove("is-busy");
        resetCaptcha();
        say(message, "bad");
    }

    form.addEventListener("submit", function (e) {
        e.preventDefault();
        if (busy) return;

        var club = kind() === "club";
        var slots = club ? [pfp, banner] : [pfp];
        var name = nameEl.value.trim();
        var email = emailEl.value.trim();
        if (!name) { say(t("Enter the name the picture is for."), "bad"); nameEl.focus(); return; }
        if (/[,"\\/:*?<>|]/.test(name) || /\.$/.test(name)) {
            say(t('A name can\'t end in a full stop or contain , " \\ / : * ? < > or |.'), "bad");
            nameEl.focus();
            return;
        }
        if (slots.some(function (s) { return s.cropping(); })) { say(t("Finish cropping first - use the crop or cancel it."), "bad"); return; }
        if (!slots.some(function (s) { return s.file; })) {
            say(club ? t("Add a club picture, a club banner, or both.") : t("Add the profile picture."), "bad");
            return;
        }
        if (!email || !emailEl.checkValidity()) { say(t("Enter an email address we can answer."), "bad"); emailEl.focus(); return; }
        if (config.captcha_sitekey && !captcha.token) { say(t("Complete the captcha first."), "bad"); return; }

        var data = new FormData();
        data.append("kind", kind());
        data.append("name", name);
        data.append("email", email);
        if (noteEl.value.trim()) data.append("note", noteEl.value.trim());
        if (captcha.token) data.append("captcha_token", captcha.token);
        if (pfp.file) data.append("pfp", pfp.file, pfp.file.name);
        if (club && banner.file) data.append("banner", banner.file, banner.file.name);

        busy = true;
        goBtn.disabled = true;
        form.classList.add("is-busy");
        say(t("Sending..."), "busy");

        fetch("/site/custom-art", { method: "POST", body: data })
            .then(function (res) {
                if (res.ok) {
                    doneMsg.textContent = ttf("We'll write to {email} if it's turned down, and when it's in the mod.", { email: email });
                    form.hidden = true;
                    doneEl.hidden = false;
                    busy = false;
                    return;
                }
                if (res.status === 429) {
                    finish(t("That's a lot of requests from here. Wait an hour and try again."));
                    return;
                }
                return res.json().then(function (body) {
                    finish((body && body.error && body.error.message) || t("That didn't go through. Try again in a moment."));
                }, function () {
                    finish(t("That didn't go through. Try again in a moment."));
                });
            })
            .catch(function () {
                finish(t("Couldn't reach the server. Check your connection and try again."));
            });
    });

    syncKind();
    fetch("/site/custom-art/config", { headers: { Accept: "application/json" } })
        .then(function (res) { return res.ok ? res.json() : null; })
        .then(function (data) {
            if (data) config = data;
            mountCaptcha();
        })
        .catch(function () { mountCaptcha(); });
})();

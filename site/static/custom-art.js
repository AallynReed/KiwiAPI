/* /custom-art - ask for a picture in the Zakros UI Chat and Nameplate mods.

   Anyone can send one; the API checks the captcha, the name and the picture, and
   a master reviews it by hand. The captcha key comes from /site/custom-art/config
   so a provider switch needs no page change. A captcha token is spent the moment
   the server verifies it, so every failed send resets the widget. */
(function () {
    "use strict";

    var form = document.getElementById("art-form");
    var doneEl = document.getElementById("art-done");
    var doneMsg = document.getElementById("art-done-msg");
    var nameEl = document.getElementById("art-name");
    var nameLabel = document.getElementById("art-name-label");
    var nameHint = document.getElementById("art-name-hint");
    var emailEl = document.getElementById("art-email");
    var noteEl = document.getElementById("art-note");
    var fileEl = document.getElementById("art-file");
    var zone = document.getElementById("art-zone");
    var zoneEmpty = document.getElementById("art-zone-empty");
    var zoneReady = document.getElementById("art-zone-ready");
    var preview = document.getElementById("art-preview");
    var fileName = document.getElementById("art-file-name");
    var clearBtn = document.getElementById("art-clear");
    var captchaEl = document.getElementById("art-captcha");
    var statusEl = document.getElementById("art-status");
    var goBtn = document.getElementById("art-go");

    var NAMES = {
        pfp: ["Player name", "Exactly as it reads in game."],
        club: ["Club name", "Exactly as the club spells it, spaces and capitals included."],
        banner: ["Club name", "Exactly as the club spells it, spaces and capitals included."]
    };
    var BANNED = /[,"\\/:*?<>|]/;

    var config = { max_bytes: 5 * 1024 * 1024, captcha_sitekey: null, captcha_provider: "turnstile" };
    var captcha = { lib: null, id: null, token: null };
    var previewUrl = null;
    var busy = false;

    function say(message, kind) {
        statusEl.textContent = message || "";
        statusEl.className = "drop-status" + (kind ? " is-" + kind : "");
        statusEl.hidden = !message;
    }

    function kind() {
        var checked = form.querySelector("input[name=kind]:checked");
        return checked ? checked.value : "pfp";
    }

    function syncKind() {
        var words = NAMES[kind()];
        nameLabel.textContent = words[0];
        nameHint.textContent = words[1];
    }

    function renderFile(file) {
        if (previewUrl) { URL.revokeObjectURL(previewUrl); previewUrl = null; }
        zoneEmpty.hidden = !!file;
        zoneReady.hidden = !file;
        clearBtn.hidden = !file;
        zone.classList.toggle("has-file", !!file);
        if (file) {
            previewUrl = URL.createObjectURL(file);
            preview.src = previewUrl;
            fileName.textContent = file.name;
        }
    }

    function pick(file) {
        say("");
        if (file && file.size > config.max_bytes) {
            say("That picture is over the " + Math.round(config.max_bytes / 1048576) + " MB limit.", "bad");
            fileEl.value = "";
            renderFile(null);
            return;
        }
        renderFile(file || null);
    }

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
            say("The captcha couldn't load. Turn off anything blocking it and reload the page.", "bad");
        };
        document.head.appendChild(script);
    }

    function resetCaptcha() {
        captcha.token = null;
        if (captcha.lib && captcha.id != null) {
            try { captcha.lib.reset(captcha.id); } catch (e) { /* widget already gone */ }
        }
    }

    // ── Wiring ───────────────────────────────────────────────────────────────

    form.querySelectorAll("input[name=kind]").forEach(function (radio) {
        radio.addEventListener("change", syncKind);
    });

    fileEl.addEventListener("change", function () { pick(fileEl.files && fileEl.files[0]); });

    clearBtn.addEventListener("click", function () {
        fileEl.value = "";
        renderFile(null);
        say("");
        fileEl.focus();
    });

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
        if (files && files.length) { fileEl.files = files; pick(files[0]); }
    });

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

        var name = nameEl.value.trim();
        var email = emailEl.value.trim();
        var file = fileEl.files && fileEl.files[0];
        if (!name) { say("Enter the name the picture is for.", "bad"); nameEl.focus(); return; }
        if (BANNED.test(name) || /\.$/.test(name)) {
            say('A name can\'t end in a full stop or contain , " \\ / : * ? < > or |.', "bad");
            nameEl.focus();
            return;
        }
        if (!email || !emailEl.checkValidity()) { say("Enter an email address we can answer.", "bad"); emailEl.focus(); return; }
        if (!file) { say("Pick the picture you want added.", "bad"); fileEl.focus(); return; }
        if (config.captcha_sitekey && !captcha.token) { say("Complete the captcha first.", "bad"); return; }

        var data = new FormData();
        data.append("kind", kind());
        data.append("name", name);
        data.append("email", email);
        if (noteEl.value.trim()) data.append("note", noteEl.value.trim());
        if (captcha.token) data.append("captcha_token", captcha.token);
        data.append("file", file, file.name);

        busy = true;
        goBtn.disabled = true;
        form.classList.add("is-busy");
        say("Sending...", "busy");

        fetch("/site/custom-art", { method: "POST", body: data })
            .then(function (res) {
                if (res.ok) {
                    doneMsg.textContent = "We'll write to " + email + " if it's turned down, and when it's in the mod.";
                    form.hidden = true;
                    doneEl.hidden = false;
                    busy = false;
                    return;
                }
                if (res.status === 429) {
                    finish("That's a lot of requests from here. Wait an hour and try again.");
                    return;
                }
                return res.json().then(function (body) {
                    finish((body && body.error && body.error.message) || "That didn't go through. Try again in a moment.");
                }, function () {
                    finish("That didn't go through. Try again in a moment.");
                });
            })
            .catch(function () {
                finish("Couldn't reach the server. Check your connection and try again.");
            });
    });

    syncKind();
    renderFile(null);
    fetch("/site/custom-art/config", { headers: { Accept: "application/json" } })
        .then(function (res) { return res.ok ? res.json() : null; })
        .then(function (data) {
            if (data) config = data;
            mountCaptcha();
        })
        .catch(function () { mountCaptcha(); });
})();

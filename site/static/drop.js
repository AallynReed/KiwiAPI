/* /drop/<slug> - a one-off, PIN-protected upload link.

   The page ships as a shell; everything it shows about the link (what it's for,
   how big a file it takes, how long it has left) comes from the API. A link that
   is expired, spent, revoked or simply wrong all answer the same 404, so this
   never has to work out which - there is one dead state and one message.

   The upload runs on XHR rather than fetch because XHR is the only one that
   reports upload progress, and a friend sending 200 MB deserves a bar rather
   than a frozen button. `_site_util.js` rewrites the /site/* URL onto the API
   origin for both. */
(function () {
    "use strict";

    var body = document.body;
    var slug = body.getAttribute("data-slug") || "";
    var base = "/site/drops/" + encodeURIComponent(slug);

    var loadingEl = document.getElementById("drop-loading");
    var deadEl = document.getElementById("drop-dead");
    var deadMsg = document.getElementById("drop-dead-msg");
    var form = document.getElementById("drop-form");
    var doneEl = document.getElementById("drop-done");
    var doneMsg = document.getElementById("drop-done-msg");

    var labelEl = document.getElementById("drop-label");
    var termsEl = document.getElementById("drop-terms");
    var pinEl = document.getElementById("drop-pin");
    var fileEl = document.getElementById("drop-file");
    var zone = document.getElementById("drop-zone");
    var zoneEmpty = document.getElementById("drop-zone-empty");
    var zoneReady = document.getElementById("drop-zone-ready");
    var nameEl = document.getElementById("drop-name");
    var sizeEl = document.getElementById("drop-size");
    var clearBtn = document.getElementById("drop-clear");
    var noteEl = document.getElementById("drop-note");
    var statusEl = document.getElementById("drop-status");
    var progress = document.getElementById("drop-progress");
    var progressBar = document.getElementById("drop-progress-bar");
    var goBtn = document.getElementById("drop-go");

    var meta = null;
    var busy = false;

    function humanSize(bytes) {
        var units = ["B", "KB", "MB", "GB"], i = 0;
        while (bytes >= 1024 && i < units.length - 1) { bytes /= 1024; i++; }
        // A round number stays round: "256 MB", not "256.0 MB".
        var n = i === 0 || bytes % 1 === 0 ? bytes.toFixed(0) : bytes.toFixed(1);
        return n + " " + units[i];
    }

    // "in 3 hours" / "in 2 days" - a deadline is easier to act on as a duration
    // than as a timestamp in a timezone the reader may not be in.
    function humanLeft(iso) {
        var ms = new Date(iso).getTime() - Date.now();
        if (!isFinite(ms) || ms <= 0) return "";
        var mins = Math.round(ms / 60000);
        if (mins < 60) return mins + (mins === 1 ? " minute" : " minutes");
        var hours = Math.round(mins / 60);
        if (hours < 48) return hours + (hours === 1 ? " hour" : " hours");
        var days = Math.round(hours / 24);
        return days + (days === 1 ? " day" : " days");
    }

    function show(el) {
        [loadingEl, deadEl, form, doneEl].forEach(function (node) {
            if (node) node.hidden = node !== el;
        });
    }

    function dead(message) {
        if (message) deadMsg.textContent = message;
        show(deadEl);
    }

    function say(message, kind) {
        statusEl.textContent = message || "";
        statusEl.className = "drop-status" + (kind ? " is-" + kind : "");
        statusEl.hidden = !message;
    }

    function setProgress(pct) {
        progress.hidden = pct == null;
        progressBar.style.width = (pct == null ? 0 : pct) + "%";
    }

    function renderFile(file) {
        zoneEmpty.hidden = !!file;
        zoneReady.hidden = !file;
        clearBtn.hidden = !file;
        zone.classList.toggle("has-file", !!file);
        if (file) {
            nameEl.textContent = file.name;
            sizeEl.textContent = humanSize(file.size);
        }
    }

    function pick(file) {
        say("");
        // Checked here as well as on the server so a file that was never going to
        // fit is refused instantly rather than after a long upload.
        if (file && meta && file.size > meta.max_file_bytes) {
            say("That file is " + humanSize(file.size) + " - this link takes up to "
                + humanSize(meta.max_file_bytes) + ".", "bad");
            fileEl.value = "";
            renderFile(null);
            return;
        }
        renderFile(file || null);
    }

    // ── Load the link ────────────────────────────────────────────────────────

    function load() {
        fetch(base, { headers: { Accept: "application/json" } })
            .then(function (res) {
                if (res.status === 404) { dead(); return null; }
                if (res.status === 429) {
                    dead("Too many attempts on this link just now. Wait a few minutes and reload.");
                    return null;
                }
                if (!res.ok) throw new Error("http " + res.status);
                return res.json();
            })
            .then(function (data) {
                if (!data) return;
                meta = data;
                labelEl.textContent = data.label;
                var bits = ["Up to " + humanSize(data.max_file_bytes)];
                if (data.uploads_left === 1) bits.push("one file only");
                else bits.push(data.uploads_left + " files left");
                var left = humanLeft(data.expires_at);
                if (left) bits.push("expires in " + left);
                termsEl.textContent = bits.join(" · ");
                show(form);
                pinEl.focus();
            })
            .catch(function () {
                dead("This link couldn't be checked right now. Try again in a moment.");
            });
    }

    // ── Picker ───────────────────────────────────────────────────────────────

    fileEl.addEventListener("change", function () {
        pick(fileEl.files && fileEl.files[0]);
    });

    clearBtn.addEventListener("click", function () {
        fileEl.value = "";
        renderFile(null);
        say("");
        fileEl.focus();
    });

    ["dragenter", "dragover"].forEach(function (name) {
        zone.addEventListener(name, function (e) {
            e.preventDefault();
            zone.classList.add("dragover");
        });
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
        if (files && files.length) {
            fileEl.files = files;
            pick(files[0]);
        }
    });

    // ── Send ─────────────────────────────────────────────────────────────────

    function finish(message, kind) {
        busy = false;
        goBtn.disabled = false;
        form.classList.remove("is-busy");
        setProgress(null);
        say(message, kind);
    }

    function errorMessage(xhr, fallback) {
        try {
            var body = JSON.parse(xhr.responseText);
            if (body && body.error && body.error.message) return body.error.message;
        } catch (e) { /* not JSON */ }
        return fallback;
    }

    function upload(file, pin) {
        var data = new FormData();
        data.append("pin", pin);
        data.append("file", file, file.name);
        if (noteEl.value.trim()) data.append("note", noteEl.value.trim());

        var xhr = new XMLHttpRequest();
        xhr.open("POST", base + "/upload");

        xhr.upload.addEventListener("progress", function (ev) {
            if (!ev.lengthComputable) return;
            var pct = Math.round((ev.loaded / ev.total) * 100);
            setProgress(pct);
            say(pct >= 100 ? "Finishing up..." : "Sending... " + pct + "%", "busy");
        });

        xhr.addEventListener("load", function () {
            if (xhr.status >= 200 && xhr.status < 300) {
                doneMsg.textContent = file.name + " (" + humanSize(file.size)
                    + ") is with them. You can close this page.";
                busy = false;
                show(doneEl);
                return;
            }
            if (xhr.status === 404 || xhr.status === 409) {
                busy = false;
                dead(errorMessage(xhr, "This link isn't active any more."));
                return;
            }
            if (xhr.status === 413) {
                finish(errorMessage(xhr, "That file is too big for this link."), "bad");
                return;
            }
            if (xhr.status === 429) {
                finish("Too many attempts just now. Wait a few minutes and try again.", "bad");
                return;
            }
            finish(errorMessage(xhr, "That didn't go through. Try again in a moment."), "bad");
        });
        xhr.addEventListener("error", function () {
            finish("The upload didn't get through. Check your connection and try again.", "bad");
        });
        xhr.addEventListener("abort", function () { finish("", ""); });

        xhr.send(data);
    }

    form.addEventListener("submit", function (e) {
        e.preventDefault();
        if (busy) return;

        var pin = pinEl.value.trim();
        var file = fileEl.files && fileEl.files[0];
        if (!pin) { say("Enter the PIN you were given.", "bad"); pinEl.focus(); return; }
        if (!file) { say("Pick the file you want to send.", "bad"); fileEl.focus(); return; }

        busy = true;
        goBtn.disabled = true;
        form.classList.add("is-busy");
        say("Checking the PIN...", "busy");

        // The PIN is checked on its own first so a typo costs a keystroke rather
        // than a finished upload. The upload re-checks it server-side regardless -
        // this step grants nothing.
        var check = new FormData();
        check.append("pin", pin);
        fetch(base + "/verify", { method: "POST", body: check })
            .then(function (res) {
                if (res.ok) { say("Sending...", "busy"); setProgress(0); upload(file, pin); return; }
                if (res.status === 403) { finish("That PIN isn't right.", "bad"); pinEl.focus(); return; }
                if (res.status === 429) {
                    finish("Too many attempts just now. Wait a few minutes and try again.", "bad");
                    return;
                }
                if (res.status === 404) { busy = false; dead(); return; }
                finish("That didn't go through. Try again in a moment.", "bad");
            })
            .catch(function () {
                finish("Couldn't reach the server. Check your connection and try again.", "bad");
            });
    });

    renderFile(null);
    load();
})();

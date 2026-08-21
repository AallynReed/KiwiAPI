/* /unlock-debug - pick a Trove.exe, get it back with the debug console on.

   The upload is the whole game executable (~60 MB), which is why this drives the
   request with XHR rather than fetch: XHR is the only one that reports upload
   progress, and a minute of a dead button with no explanation reads as a broken
   page. `_site_util.js` rewrites the /site/* URL onto the API origin for both,
   so the call still lands in the right place.

   The plain <form> underneath stays a real form on purpose - if this script never
   runs, submitting it still works, it just does so without a progress bar. */
(function () {
    "use strict";

    function t(s) {
        return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s;
    }

    var form = document.getElementById("udbg-form");
    var input = document.getElementById("udbg-file");
    var drop = document.getElementById("udbg-drop");
    if (!form || !input || !drop) return;

    var emptyEl = document.getElementById("udbg-drop-empty");
    var readyEl = document.getElementById("udbg-drop-ready");
    var nameEl = document.getElementById("udbg-name");
    var sizeEl = document.getElementById("udbg-size");
    var clearBtn = document.getElementById("udbg-clear");
    var statusEl = document.getElementById("udbg-status");
    var goBtn = document.getElementById("udbg-go");
    var busy = false;

    function humanSize(bytes) {
        var units = ["B", "KB", "MB", "GB"], i = 0;
        while (bytes >= 1024 && i < units.length - 1) { bytes /= 1024; i++; }
        return bytes.toFixed(i === 0 ? 0 : 1) + " " + units[i];
    }

    function say(message, kind) {
        statusEl.textContent = message;
        statusEl.className = "udbg-status" + (kind ? " is-" + kind : "");
        statusEl.hidden = !message;
    }

    function render(file) {
        emptyEl.hidden = !!file;
        readyEl.hidden = !file;
        clearBtn.hidden = !file;
        drop.classList.toggle("has-file", !!file);
        goBtn.disabled = !file || busy;
        if (file) {
            nameEl.textContent = file.name;
            sizeEl.textContent = humanSize(file.size);
        }
    }

    function pick(file) {
        say("");
        // A .exe is the only thing this can patch, and the accept= filter is a
        // suggestion the OS picker is free to ignore - so say so here rather than
        // spending a 60 MB upload to be told the same thing by the server.
        if (file && !/\.exe$/i.test(file.name)) {
            say(t("That isn't an .exe - pick your Trove.exe."), "bad");
            input.value = "";
            render(null);
            return;
        }
        render(file || null);
    }

    input.addEventListener("change", function () {
        pick(input.files && input.files[0]);
    });

    clearBtn.addEventListener("click", function () {
        input.value = "";
        render(null);
        say("");
        input.focus();
    });

    ["dragenter", "dragover"].forEach(function (name) {
        drop.addEventListener(name, function (e) {
            e.preventDefault();
            drop.classList.add("dragover");
        });
    });
    ["dragleave", "drop"].forEach(function (name) {
        drop.addEventListener(name, function (e) {
            if (name === "dragleave" && drop.contains(e.relatedTarget)) return;
            e.preventDefault();
            drop.classList.remove("dragover");
        });
    });
    drop.addEventListener("drop", function (e) {
        var files = e.dataTransfer && e.dataTransfer.files;
        if (files && files.length) {
            input.files = files;
            pick(files[0]);
        }
    });

    // The error envelope arrives as JSON even though the success case is a binary
    // blob, so a failed response has to be read back out of the Blob as text.
    function readError(blob, done) {
        var reader = new FileReader();
        reader.onload = function () {
            var message = "";
            try { message = JSON.parse(reader.result).error.message; } catch (e) { /* not JSON */ }
            done(message);
        };
        reader.onerror = function () { done(""); };
        reader.readAsText(blob);
    }

    function save(blob) {
        var url = URL.createObjectURL(blob);
        var a = document.createElement("a");
        a.href = url;
        a.download = "Trove.exe";
        document.body.appendChild(a);
        a.click();
        a.remove();
        // Revoked on a turn of its own: Safari cancels the download if the object
        // URL disappears in the same tick as the click.
        setTimeout(function () { URL.revokeObjectURL(url); }, 60000);
    }

    form.addEventListener("submit", function (e) {
        var file = input.files && input.files[0];
        if (!file || busy) { e.preventDefault(); return; }
        e.preventDefault();

        busy = true;
        goBtn.disabled = true;
        form.classList.add("is-busy");
        say(t("Uploading..."), "busy");

        var body = new FormData();
        body.append("trove_exe", file, file.name);

        var xhr = new XMLHttpRequest();
        xhr.open("POST", "/site/unlock-debug");
        xhr.responseType = "blob";

        xhr.upload.addEventListener("progress", function (ev) {
            if (!ev.lengthComputable) return;
            var pct = Math.round((ev.loaded / ev.total) * 100);
            say(pct >= 100 ? t("Patching...") : t("Uploading...") + " " + pct + "%", "busy");
        });

        function finish(message, kind) {
            busy = false;
            form.classList.remove("is-busy");
            goBtn.disabled = false;
            say(message, kind);
        }

        xhr.addEventListener("load", function () {
            if (xhr.status === 200) {
                save(xhr.response);
                finish(t("Done - your patched Trove.exe is downloading."), "good");
                return;
            }
            if (xhr.status === 429) {
                finish(t("Too many patches from this connection just now. Try again in a few minutes."), "bad");
                return;
            }
            if (xhr.status === 413) {
                finish(t("That file is too large to send."), "bad");
                return;
            }
            readError(xhr.response, function (message) {
                finish(message || t("That didn't work. Try again in a moment."), "bad");
            });
        });
        xhr.addEventListener("error", function () {
            finish(t("The upload didn't get through. Check your connection and try again."), "bad");
        });
        xhr.addEventListener("abort", function () { finish("", ""); });

        xhr.send(body);
    });

    render(null);
})();

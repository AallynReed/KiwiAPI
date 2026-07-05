/* Shared modal plumbing (window.BTTModal.open). Reuses the existing .mp-modal CSS
   (backdrop + card), so the look is unchanged; only the duplicated wrap/Escape/close
   wiring is shared. Two content modes:
     open({ title, body, wide })  -> card = close button + <h2>title</h2> + body html
     open({ html })               -> card content is the raw html (caller owns close/title)
   Returns { wrap, close }. */
(function () {
    "use strict";

    const ESC = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };
    const esc = (s) => String(s == null ? "" : s).replace(/[&<>"']/g, (c) => ESC[c]);

    let current = null;  // one modal at a time; opening a new one replaces it

    function open(opts) {
        opts = opts || {};
        if (current) current.close();
        const inner = opts.html != null
            ? opts.html
            : '<button type="button" class="mp-modal-close" data-close aria-label="Close">'
              + '<i class="fa-solid fa-xmark"></i></button>'
              + '<h2 class="mp-modal-title">' + esc(opts.title) + "</h2>"
              + (opts.body || "");
        const wrap = document.createElement("div");
        wrap.className = "mp-modal";
        wrap.innerHTML = '<div class="mp-modal-backdrop" data-close></div>'
            + '<div class="mp-modal-card ' + (opts.wide ? "wide" : "") + '">' + inner + "</div>";
        (opts.root || document.body).appendChild(wrap);

        const handle = { wrap, close: null };
        const onKey = (e) => { if (e.key === "Escape") handle.close(); };
        handle.close = () => {
            wrap.remove();
            document.removeEventListener("keydown", onKey);
            if (current === handle) current = null;
        };
        document.addEventListener("keydown", onKey);
        wrap.querySelectorAll("[data-close]").forEach((b) => b.addEventListener("click", handle.close));
        // Re-translate any [data-i18n] nodes injected into the modal.
        if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
        current = handle;
        return handle;
    }

    window.BTTModal = { open };
})();

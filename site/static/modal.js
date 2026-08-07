/* Shared modal plumbing (window.BTTModal.open). Reuses the existing .mp-modal CSS
   (backdrop + card), so the look is unchanged; only the duplicated wrap/Escape/close
   wiring is shared. Two content modes:
     open({ title, body, wide })  -> card = close button + <h2>title</h2> + body html
     open({ html })               -> card content is the raw html (caller owns close/title)
   Returns { wrap, close }. */
(function () {
    "use strict";

    const { esc } = window.BTTUtil;

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
        // Title mode gets a labelled heading for aria-labelledby; caller-html mode
        // owns its own labelling.
        let titleId = "";
        if (opts.html == null) {
            titleId = "bttm-title-" + Math.random().toString(36).slice(2, 8);
        }
        const cardInner = opts.html == null && titleId
            ? inner.replace('<h2 class="mp-modal-title">', '<h2 class="mp-modal-title" id="' + titleId + '">')
            : inner;
        const wrap = document.createElement("div");
        wrap.className = "mp-modal";
        wrap.innerHTML = '<div class="mp-modal-backdrop" data-close></div>'
            + '<div class="mp-modal-card ' + (opts.wide ? "wide" : "") + '" role="dialog" aria-modal="true"'
            + (titleId ? ' aria-labelledby="' + titleId + '"' : "") + ">" + cardInner + "</div>";
        (opts.root || document.body).appendChild(wrap);
        const card = wrap.querySelector(".mp-modal-card");

        const handle = { wrap, close: null };
        // Focus trap + restore (Escape routed through it). Falls back to a bare
        // Escape listener if the shared helper somehow isn't loaded.
        let release = null;
        const onKeyFallback = (e) => { if (e.key === "Escape") handle.close(); };
        handle.close = () => {
            if (release) release(); else document.removeEventListener("keydown", onKeyFallback);
            wrap.remove();
            if (current === handle) current = null;
        };
        if (window.BTTUtil && window.BTTUtil.trapFocus) {
            release = window.BTTUtil.trapFocus(card, { onEscape: handle.close });
        } else {
            document.addEventListener("keydown", onKeyFallback);
        }
        wrap.querySelectorAll("[data-close]").forEach((b) => b.addEventListener("click", handle.close));
        // Re-translate any [data-i18n] nodes injected into the modal.
        if (window.BTTi18n && window.BTTi18n.refresh) window.BTTi18n.refresh();
        current = handle;
        return handle;
    }

    window.BTTModal = { open };
})();

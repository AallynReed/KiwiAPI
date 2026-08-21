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

    // Matches the .mp-modal.is-closing transition in style.css section 21.
    const EXIT_MS = 190;

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
        let closing = false;
        // Escape belongs to the TOP layer. A viewer that stacks OVER this modal
        // (the .swf code viewer) marks itself [data-overlay-layer] while it is
        // open; ordering can't settle this on its own, because the trap listens on
        // document in capture and this modal registered first, so it would always
        // win the key. Checking for the marker instead means one Escape closes one
        // layer, and the reader lands back in the modal they opened it from.
        const onEscape = () => {
            if (!document.querySelector("[data-overlay-layer]")) handle.close();
        };
        const onKeyFallback = (e) => { if (e.key === "Escape") onEscape(); };
        handle.close = () => {
            if (closing) return;
            closing = true;
            // Everything the caller can observe happens now: focus goes back,
            // the modal stops being "current", and the shell stops taking
            // clicks. Only the DOM removal waits for the exit to play, so a
            // caller that closes one modal and opens another in the same tick
            // still gets the new one immediately.
            if (release) release(); else document.removeEventListener("keydown", onKeyFallback);
            if (current === handle) current = null;
            wrap.classList.add("is-closing");
            setTimeout(() => wrap.remove(), EXIT_MS);
        };
        if (window.BTTUtil && window.BTTUtil.trapFocus) {
            release = window.BTTUtil.trapFocus(card, { onEscape });
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

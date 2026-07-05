/* Transient toast notifications (window.BTTToast.show(msg, isErr)). Self-contained:
   injects its own style + host on first use, stacks, fades, auto-dismisses. Uses the
   site theme vars with fallbacks, so it looks right on any page that loads it. */
(function () {
    "use strict";

    let host = null;

    function ensureStyle() {
        if (document.getElementById("btt-toast-style")) return;
        const s = document.createElement("style");
        s.id = "btt-toast-style";
        s.textContent =
            ".btt-toast-host{position:fixed;bottom:22px;left:50%;transform:translateX(-50%);" +
            "z-index:9999;display:flex;flex-direction:column;gap:8px;align-items:center;pointer-events:none}" +
            ".btt-toast{max-width:min(90vw,420px);background:var(--bg-card-hi,var(--bg-card,#141821));" +
            "border:1px solid var(--bg-line,#2a3140);border-left:3px solid var(--accent-blue,#4a9eff);" +
            "border-radius:var(--radius-sm,8px);padding:11px 18px;color:var(--text,#e6ebf2);font-size:.9rem;" +
            "box-shadow:0 12px 30px rgba(0,0,0,.4);opacity:0;transform:translateY(8px);" +
            "transition:opacity .2s,transform .2s}" +
            ".btt-toast.show{opacity:1;transform:translateY(0)}" +
            ".btt-toast.err{border-left-color:var(--accent-red,#ff6b81);color:var(--accent-red,#ff8a9c)}";
        document.head.appendChild(s);
    }

    // isErr is truthy for the error variant; also accepts the legacy string "error".
    function show(msg, isErr) {
        ensureStyle();
        if (!host) {
            host = document.createElement("div");
            host.className = "btt-toast-host";
            document.body.appendChild(host);
        }
        const el = document.createElement("div");
        el.className = "btt-toast" + (isErr === true || isErr === "error" ? " err" : "");
        el.textContent = msg;
        host.appendChild(el);
        requestAnimationFrame(() => el.classList.add("show"));
        setTimeout(() => {
            el.classList.remove("show");
            setTimeout(() => el.remove(), 220);
        }, 2800);
    }

    window.BTTToast = { show };
})();

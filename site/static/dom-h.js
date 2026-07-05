/* Hyperscript DOM builder (window.BTTDom.h). Loaded before the pages that use it. */
(function () {
    "use strict";

    // h(tag, attrs, ...children) -> Element. attrs dispatch: class, html (innerHTML),
    // style (object -> Object.assign), dataset (object), on* (event listener), else
    // setAttribute. null/false attrs and children are skipped; children may be nested
    // arrays; non-object children become text nodes.
    function h(tag, attrs) {
        const e = document.createElement(tag);
        if (attrs) {
            for (const k in attrs) {
                const v = attrs[k];
                if (v == null || v === false) continue;
                if (k === "class") e.className = v;
                else if (k === "html") e.innerHTML = v;
                else if (k === "style" && typeof v === "object") Object.assign(e.style, v);
                else if (k === "dataset") Object.assign(e.dataset, v);
                else if (k.slice(0, 2) === "on" && typeof v === "function") e.addEventListener(k.slice(2).toLowerCase(), v);
                else e.setAttribute(k, v);
            }
        }
        for (let i = 2; i < arguments.length; i++) {
            const kids = arguments[i];
            (Array.isArray(kids) ? kids : [kids]).forEach((kid) => {
                if (kid == null || kid === false) return;
                e.appendChild(typeof kid === "object" ? kid : document.createTextNode(String(kid)));
            });
        }
        return e;
    }

    window.BTTDom = { h };
})();

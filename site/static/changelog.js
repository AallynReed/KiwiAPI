/* Website changelog (/changelog): renders the site's own open-source commit
   history from /site/changelog, grouped by version tag. Read-only, no auth.
   Loaded after _site_util.js (window.BTTUtil) + i18n.js (window.t). */
(function () {
    "use strict";

    const { esc, fetchJSON } = window.BTTUtil;
    const t = window.t || ((s) => s);
    const list = document.getElementById("cl-list");
    if (!list) return;

    // conventional-commit type -> [display label, style class]. Unknown types
    // fall back to the raw token with a neutral chip.
    const TYPES = {
        feat: ["Feature", "feat"],
        fix: ["Fix", "fix"],
        perf: ["Performance", "perf"],
        refactor: ["Refactor", "refactor"],
        docs: ["Docs", "docs"],
        test: ["Tests", "chore"],
        build: ["Build", "chore"],
        ci: ["CI", "chore"],
        chore: ["Chore", "chore"],
        style: ["Style", "chore"],
        revert: ["Revert", "fix"],
    };

    function typeChip(type) {
        if (!type) return "";
        const meta = TYPES[type] || [type, "other"];
        return `<span class="cl-type cl-type-${meta[1]}">${esc(t(meta[0]))}</span>`;
    }

    // Drop the "type(scope): " prefix from the first line - the chip already
    // conveys the type, so the message reads cleaner without it.
    function cleanMessage(msg, type) {
        if (!type) return msg;
        return msg.replace(/^[a-zA-Z]+(\([^)]+\))?:\s*/, "");
    }

    function commitRow(c) {
        const msg = esc(cleanMessage(c.message || "", c.type));
        const sha = c.url
            ? `<a class="cl-sha" href="${esc(c.url)}" target="_blank" rel="noopener" title="${esc(t("View commit on GitHub"))}">${esc(c.short_sha)}</a>`
            : `<span class="cl-sha">${esc(c.short_sha)}</span>`;
        return `<li class="cl-commit">${typeChip(c.type)}<span class="cl-msg">${msg}</span>${sha}</li>`;
    }

    function groupBlock(g) {
        const isUnreleased = g.version === "Unreleased";
        const label = isUnreleased ? t("Recent changes") : g.version;
        const n = g.commits.length;
        const countLabel = `${n} ${n === 1 ? t("change") : t("changes")}`;
        return `<section class="cl-group">
      <div class="cl-group-head">
        <h2 class="cl-version">${esc(label)}</h2>
        <span class="cl-count">${esc(countLabel)}</span>
      </div>
      <ul class="cl-commits">${g.commits.map(commitRow).join("")}</ul>
    </section>`;
    }

    async function load() {
        try {
            const data = await fetchJSON("/site/changelog");
            const groups = (data && data.groups) || [];
            if (!groups.length) {
                list.innerHTML = (data && data.rate_limited)
                    ? `<p class="cl-empty">${esc(t("The changelog is briefly unavailable. Try again in a minute."))}</p>`
                    : `<p class="cl-empty">${esc(t("No changes to show yet."))}</p>`;
                return;
            }
            list.innerHTML = groups.map(groupBlock).join("");
        } catch (_e) {
            list.innerHTML = `<p class="cl-empty">${esc(t("Couldn't load the changelog right now."))}</p>`;
        }
    }

    load();
})();

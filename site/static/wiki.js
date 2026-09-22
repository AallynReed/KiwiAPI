/* ===========================================================================
   wiki.js - Kiwi Wiki (the wiki host; see app/web/wiki.py).

   Renders page text (markdown + [[wiki links]]), shows the edit or suggest
   action that fits the reader, and drives the tool pages: search, recent
   changes, all pages, history (with diffs and restore) and suggestion review.
   Exposes window.BTTWiki for wiki_edit.js.
   ========================================================================== */
(function () {
  "use strict";

  const { esc } = window.BTTUtil;
  const SITE = document.body.dataset.siteOrigin || "";

  // ── helpers ────────────────────────────────────────────────────────────
  // Same rule as app/wiki/service.py slugify().
  function slugify(text) {
    return String(text || "").normalize("NFKD").replace(/[̀-ͯ]/g, "")
      .toLowerCase().replace(/['’]/g, "").replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "").slice(0, 100).replace(/-+$/, "");
  }

  // Where a stored slug is read. class/<tech> pages live at /class/<name>, data/<name> at /<name>.
  function pageHref(slug, title) {
    if (slug.startsWith("class/")) return "/class/" + slugify(title);
    return "/" + slug.replace(/^data\//, "");
  }

  function hasSession() {
    return document.cookie.split("; ").some((c) => c.startsWith("kiwi_site_session="));
  }

  function signInUrl() {
    return SITE + "/login?next=" + encodeURIComponent(location.href);
  }

  async function api(path, opts) {
    opts = opts || {};
    const init = { method: opts.method || "GET", headers: {} };
    if (opts.body !== undefined) {
      init.headers["Content-Type"] = "application/json";
      init.body = JSON.stringify(opts.body);
    }
    const res = await fetch(path, init);
    let data = null;
    try { data = await res.json(); } catch (_) { /* empty body */ }
    if (!res.ok) {
      const msg = (data && data.error && data.error.message)
        || (data && typeof data.detail === "string" && data.detail)
        || (res.status === 401 ? "Your session ended. Sign in again." : "Something went wrong. Try again.");
      const err = new Error(msg);
      err.status = res.status;
      throw err;
    }
    return data;
  }

  const q = (k) => new URLSearchParams(location.search).get(k);

  const _rtf = (typeof Intl !== "undefined" && Intl.RelativeTimeFormat)
    ? new Intl.RelativeTimeFormat("en", { numeric: "auto" }) : null;
  function ago(iso) {
    const t = Date.parse(iso);
    if (!t) return "";
    const s = (t - Date.now()) / 1000;
    const steps = [[60, "second"], [3600, "minute"], [86400, "hour"], [604800, "day"], [2629800, "week"], [31557600, "month"], [Infinity, "year"]];
    const units = { second: 1, minute: 60, hour: 3600, day: 86400, week: 604800, month: 2629800, year: 31557600 };
    for (const [limit, unit] of steps) {
      if (Math.abs(s) < limit) {
        const v = Math.round(s / units[unit]);
        return _rtf ? _rtf.format(v, unit) : new Date(t).toLocaleDateString();
      }
    }
    return "";
  }
  function timeTag(iso) {
    if (!iso) return "";
    return `<time datetime="${esc(iso)}" title="${esc(new Date(iso).toLocaleString())}">${esc(ago(iso))}</time>`;
  }

  // ── rendering ──────────────────────────────────────────────────────────
  // [[Target]] / [[Target|label]] -> a markdown link. Code spans and fences are
  // left alone so a snippet can show the syntax itself.
  function wikiLinks(md) {
    const parts = String(md || "").split(/(```[\s\S]*?```|~~~[\s\S]*?~~~|`[^`\n]*`)/);
    return parts.map((part, i) => (i % 2) ? part : part.replace(/\[\[([^\[\]|\n]{1,120})(?:\|([^\[\]\n]{1,120}))?\]\]/g, (m, target, label) => {
      target = target.trim();
      const isClass = /^class\//i.test(target);
      const name = isClass ? target.slice(6) : target;
      const href = isClass ? "/class/" + slugify(name) : "/" + slugify(name);
      const text = (label || name).trim().replace(/[\[\]]/g, "");
      return `[${text}](${href})`;
    })).join("");
  }

  const SCROLL_ROWS = 15;

  let _known = null;
  function knownPages() {
    if (!_known) {
      // Generated routes (/classes, data pages) exist whether or not a write-up does.
      const generated = (document.body.dataset.generated || "").split(" ").filter(Boolean);
      _known = api("/site/wiki/pages")
        .then((d) => new Set([...generated, ...(d.items || []).map((p) => p.slug)]))
        .catch(() => null);
    }
    return _known;
  }

  function renderInto(el, md, opts) {
    opts = opts || {};
    el.innerHTML = window.BTTMarkdown.render(wikiLinks(md));
    const internal = [];
    el.querySelectorAll("a[href]").forEach((a) => {
      const href = a.getAttribute("href");
      if (href.startsWith("/") && !href.startsWith("//")) {
        a.removeAttribute("target");
        a.setAttribute("rel", "nofollow");
        internal.push(a);
      }
    });
    // Mark links to pages that don't exist yet, the way wikis do.
    if (internal.length) {
      knownPages().then((known) => {
        if (!known) return;
        internal.forEach((a) => {
          const path = a.getAttribute("href").split(/[?#]/)[0].replace(/^\//, "");
          if (path && !path.includes("/") && !path.startsWith("-") && !known.has(path)) {
            a.classList.add("wk-missing");
            a.title = "This page doesn't exist yet";
          }
        });
      });
    }
    // Long tables scroll inside their own box so they don't bury the page.
    el.querySelectorAll("table").forEach((t) => {
      const body = t.tBodies[0];
      if (!body || body.rows.length <= SCROLL_ROWS) return;
      const box = document.createElement("div");
      box.className = "wk-scrolltable";
      box.tabIndex = 0;
      box.setAttribute("role", "region");
      let head = t.previousElementSibling;
      while (head && !/^H[1-6]$/.test(head.tagName)) head = head.previousElementSibling;
      box.setAttribute("aria-label", head ? head.textContent : "Table");
      t.parentNode.insertBefore(box, t);
      box.appendChild(t);
    });
    // Heading anchors (the sanitizer strips ids, so they're added after it).
    const used = {};
    const heads = [];
    el.querySelectorAll("h1, h2, h3").forEach((h) => {
      let id = slugify(h.textContent) || "section";
      if (used[id]) id += "-" + (++used[id]); else used[id] = 1;
      h.id = id;
      heads.push(h);
    });
    if (opts.toc) {
      const toc = opts.toc;
      const list = heads.filter((h) => h.tagName !== "H1");
      if (list.length >= 3) {
        toc.innerHTML = `<p class="wk-toc-title">On this page</p><ol>${list.map((h) =>
          `<li class="wk-toc-${h.tagName.toLowerCase()}"><a href="#${esc(h.id)}">${esc(h.textContent)}</a></li>`).join("")}</ol>`;
        toc.hidden = false;
      } else {
        toc.hidden = true;
      }
    }
  }

  // Line diff (LCS). Enough for page text; oversized inputs fall back to
  // "everything changed" rather than stalling the tab.
  function diffLines(a, b) {
    const A = String(a || "").split("\n"), B = String(b || "").split("\n");
    let s = 0;
    while (s < A.length && s < B.length && A[s] === B[s]) s++;
    let e = 0;
    while (e < A.length - s && e < B.length - s && A[A.length - 1 - e] === B[B.length - 1 - e]) e++;
    const a2 = A.slice(s, A.length - e), b2 = B.slice(s, B.length - e);
    const ops = A.slice(0, s).map((l) => [" ", l]);
    const n = a2.length, m = b2.length;
    if (n * m > 4e6) {
      a2.forEach((l) => ops.push(["-", l]));
      b2.forEach((l) => ops.push(["+", l]));
    } else {
      const w = m + 1;
      const L = new Uint32Array((n + 1) * w);
      for (let i = n - 1; i >= 0; i--) {
        for (let j = m - 1; j >= 0; j--) {
          L[i * w + j] = a2[i] === b2[j] ? L[(i + 1) * w + j + 1] + 1 : Math.max(L[(i + 1) * w + j], L[i * w + j + 1]);
        }
      }
      let i = 0, j = 0;
      while (i < n && j < m) {
        if (a2[i] === b2[j]) { ops.push([" ", a2[i]]); i++; j++; }
        else if (L[(i + 1) * w + j] >= L[i * w + j + 1]) ops.push(["-", a2[i++]]);
        else ops.push(["+", b2[j++]]);
      }
      while (i < n) ops.push(["-", a2[i++]]);
      while (j < m) ops.push(["+", b2[j++]]);
    }
    A.slice(A.length - e).forEach((l) => ops.push([" ", l]));
    return ops;
  }

  function diffHtml(a, b) {
    const ops = diffLines(a, b);
    if (!ops.some((o) => o[0] !== " ")) return `<p class="wk-list-empty">No differences in the text.</p>`;
    const CONTEXT = 3;
    const keep = ops.map(() => false);
    ops.forEach((o, i) => {
      if (o[0] === " ") return;
      for (let k = Math.max(0, i - CONTEXT); k <= Math.min(ops.length - 1, i + CONTEXT); k++) keep[k] = true;
    });
    let html = "", skipped = 0;
    const flush = () => { if (skipped) { html += `<div class="wk-diff-skip">${skipped} unchanged line${skipped === 1 ? "" : "s"}</div>`; skipped = 0; } };
    ops.forEach((o, i) => {
      if (!keep[i]) { skipped++; return; }
      flush();
      const cls = o[0] === "+" ? "is-add" : o[0] === "-" ? "is-del" : "";
      const sign = o[0] === "+" ? "Added" : o[0] === "-" ? "Removed" : "";
      html += `<div class="wk-diff-line ${cls}">${sign ? `<span class="wk-sr">${sign}: </span>` : ""}<span class="wk-diff-sign" aria-hidden="true">${o[0] === " " ? "" : o[0]}</span><span class="wk-diff-text">${esc(o[1]) || "&nbsp;"}</span></div>`;
    });
    flush();
    return `<div class="wk-diff" role="region" aria-label="Changes">${html}</div>`;
  }

  // ── who's reading ──────────────────────────────────────────────────────
  let _me = null;
  function me() {
    if (!_me) {
      _me = hasSession()
        ? api("/site/wiki/me").catch(() => ({ signed_in: false, is_editor: false, pending: 0 }))
        : Promise.resolve({ signed_in: false, is_editor: false, pending: 0 });
    }
    return _me;
  }

  async function applyViewer() {
    const who = await me();
    document.querySelectorAll("[data-edit]").forEach((a) => {
      const label = a.querySelector("[data-edit-label]");
      if (label) label.textContent = who.is_editor ? "Edit" : "Suggest an edit";
      a.hidden = false;
    });
    document.querySelectorAll("[data-edit-cta]").forEach((a) => {
      if (!who.is_editor && /^(Write|Create)/.test(a.textContent)) {
        a.textContent = a.textContent.replace(/^Write/, "Suggest").replace(/^Create/, "Suggest");
      }
    });
    const link = document.getElementById("wk-sugg-link");
    if (link && who.signed_in) {
      link.hidden = false;
      if (!who.is_editor) link.firstChild.textContent = "My suggestions ";
      const count = document.getElementById("wk-sugg-count");
      if (who.is_editor && who.pending && count) {
        count.textContent = who.pending;
        count.hidden = false;
        link.setAttribute("aria-label", `Suggestions, ${who.pending} waiting for review`);
      }
    }
  }

  // ── views ──────────────────────────────────────────────────────────────
  function readJSON(id) {
    const n = document.getElementById(id);
    if (!n) return null;
    try { return JSON.parse(n.textContent); } catch (_) { return null; }
  }

  function renderPageBody() {
    const body = document.getElementById("wk-body");
    if (!body) return;
    const src = readJSON(body.dataset.srcId);
    if (src != null) renderInto(body, src, { toc: document.getElementById("wk-toc") });
  }

  function revRow(r, withPage) {
    const verb = { edit: "", revert: "Restored", delete: "Deleted" }[r.action] || "";
    const who = r.author ? esc(r.author) : "someone";
    const via = r.approved_by ? ` <span class="wk-via">approved by ${esc(r.approved_by)}</span>` : "";
    const page = withPage ? `<a class="wk-row-title" href="${esc(pageHref(r.slug, r.title))}">${esc(r.title)}</a>` : "";
    const summary = r.summary ? `<span class="wk-row-summary">${esc(r.summary)}</span>` : "";
    const isNew = r.created || (r.rev === 1 && r.action === "edit");
    const edits = r.edits > 1 ? ` · <a href="/-/history/${esc(r.slug)}">${r.edits} edits</a>` : "";
    return `${page}
      <span class="wk-row-meta">${verb ? `<span class="wk-pill">${verb}</span> ` : ""}${isNew ? '<span class="wk-pill">New</span> ' : ""}${who}${via} · ${timeTag(r.created_at)}${edits}</span>
      ${summary}`;
  }

  async function viewHome() {
    const list = document.getElementById("wk-home-recent-list");
    if (!list) return;
    try {
      const d = await api("/site/wiki/recent?per_page=1&limit=" + (list.dataset.limit || 8));
      list.innerHTML = d.items.length
        ? d.items.map((r) => `<li class="wk-row">${revRow(r, true)}</li>`).join("")
        : `<li class="wk-list-empty">Nothing written yet. The first page could be yours.</li>`;
    } catch (_) {
      list.innerHTML = `<li class="wk-list-empty">Couldn't load recent changes.</li>`;
    }
  }

  function toolBody() { return document.getElementById("wk-tool-body"); }
  function fail(el, err) {
    el.innerHTML = `<p class="wk-error" role="alert">${esc(err.message || "Something went wrong.")}</p>`;
  }

  async function viewSearch() {
    const el = toolBody();
    const term = (q("q") || "").trim();
    if (!term) { el.innerHTML = `<p class="wk-list-empty">Type something in the search box above.</p>`; return; }
    try {
      const d = await api("/site/wiki/search?q=" + encodeURIComponent(term));
      const needle = term.toLowerCase();
      const classes = (readJSON("wk-classes") || []).filter((c) => `${c.name} ${c.terms || ""}`.toLowerCase().includes(needle));
      const seen = new Set();
      let html = "";
      classes.forEach((c) => {
        seen.add(c.url);
        html += `<li class="wk-row"><a class="wk-row-title" href="${esc(c.url)}">${esc(c.name)}</a><span class="wk-row-meta">${esc(c.kind || "Class")}</span></li>`;
      });
      d.items.forEach((p) => {
        const href = pageHref(p.slug, p.title);
        if (seen.has(href)) return;
        html += `<li class="wk-row"><a class="wk-row-title" href="${esc(href)}">${esc(p.title)}</a>${p.excerpt ? `<span class="wk-row-summary">${esc(p.excerpt)}</span>` : ""}</li>`;
      });
      const create = /^[a-z0-9]/i.test(term)
        ? `<p class="wk-hint">Can't find it? <a href="/-/new?title=${encodeURIComponent(term)}">Start a page called “${esc(term)}”</a>.</p>` : "";
      el.innerHTML = html
        ? `<p class="wk-count-line" role="status">Results for “${esc(term)}”</p><ul class="wk-list">${html}</ul>${create}`
        : `<p class="wk-list-empty" role="status">No pages match “${esc(term)}”.</p>${create}`;
    } catch (err) { fail(el, err); }
  }

  async function viewRecent() {
    const el = toolBody();
    try {
      const d = await api("/site/wiki/recent?limit=100");
      el.innerHTML = d.items.length
        ? `<ul class="wk-list">${d.items.map((r) => `<li class="wk-row">${revRow(r, true)} <a class="wk-row-link" href="/-/history/${esc(r.slug)}?rev=${r.rev}">View change</a></li>`).join("")}</ul>`
        : `<p class="wk-list-empty">No edits yet.</p>`;
    } catch (err) { fail(el, err); }
  }

  async function viewPages() {
    const el = toolBody();
    try {
      const d = await api("/site/wiki/pages");
      const articles = d.items.filter((p) => !p.slug.startsWith("class/"));
      const guides = d.items.filter((p) => p.slug.startsWith("class/"));
      const block = (title, items) => items.length ? `<h2 class="wk-subhead">${title}</h2><ul class="wk-list wk-list-cols">${items.map((p) =>
        `<li><a href="${esc(pageHref(p.slug, p.title))}">${esc(p.title)}</a></li>`).join("")}</ul>` : "";
      el.innerHTML = (block("Articles", articles) + block("Class guides", guides))
        || `<p class="wk-list-empty">No pages yet. <a href="/-/new">Start the first one</a>.</p>`;
    } catch (err) { fail(el, err); }
  }

  async function viewHistory() {
    const el = toolBody();
    const slug = document.getElementById("wk-tool").dataset.slug;
    const who = await me();
    let d;
    try { d = await api(`/site/wiki/history?slug=${encodeURIComponent(slug)}&limit=200`); }
    catch (err) { fail(el, err); return; }
    if (!d.items.length) { el.innerHTML = `<p class="wk-list-empty">This page has no history yet.</p>`; return; }
    const current = d.items[0];
    const deleted = current.action === "delete";
    el.innerHTML = `
      <ol class="wk-list wk-history">${d.items.map((r) => `
        <li class="wk-row${r.rev === current.rev ? " is-current" : ""}" data-rev="${r.rev}">
          <span class="wk-rev">#${r.rev}</span>
          ${revRow(r, false)}
          <span class="wk-row-actions">
            <button type="button" class="wk-linkbtn" data-act="view">View</button>
            ${r.rev > 1 ? `<button type="button" class="wk-linkbtn" data-act="diff">Changes</button>` : ""}
            ${who.is_editor && (r.rev !== current.rev || deleted) && r.action !== "delete" ? `<button type="button" class="wk-linkbtn" data-act="restore">Restore</button>` : ""}
          </span>
        </li>`).join("")}
      </ol>
      ${who.is_editor && !deleted ? `
      <div class="wk-danger">
        <button type="button" class="wk-btn wk-btn-danger" id="wk-del-open">Delete this page…</button>
        <form class="wk-inline-form" id="wk-del-form" hidden>
          <label for="wk-del-reason">Why is it being deleted?</label>
          <input id="wk-del-reason" maxlength="200" autocomplete="off">
          <div class="wk-actions">
            <button type="submit" class="wk-btn wk-btn-danger">Delete page</button>
            <button type="button" class="wk-btn" id="wk-del-cancel">Keep it</button>
          </div>
          <p class="wk-help">Its history stays, and any editor can restore it.</p>
        </form>
      </div>` : ""}
      <section class="wk-revpanel" id="wk-revpanel" tabindex="-1" hidden></section>
      <p class="wk-error" id="wk-hist-error" role="alert" hidden></p>`;

    const panel = document.getElementById("wk-revpanel");
    const errEl = document.getElementById("wk-hist-error");
    const getRev = (rev) => api(`/site/wiki/revision?slug=${encodeURIComponent(slug)}&rev=${rev}`);
    async function show(rev, mode) {
      errEl.hidden = true;
      try {
        const r = await getRev(rev);
        let inner;
        if (mode === "diff") {
          const prev = await getRev(rev - 1);
          inner = `<h2 class="wk-subhead">Changes in #${rev}</h2>${prev.title !== r.title ? `<p class="wk-hint">Title: “${esc(prev.title)}” → “${esc(r.title)}”</p>` : ""}${diffHtml(prev.body, r.body)}`;
        } else {
          inner = `<h2 class="wk-subhead">Revision #${rev}: ${esc(r.title)}</h2><div class="wk-prose" id="wk-rev-body"></div>`;
        }
        panel.innerHTML = inner;
        panel.hidden = false;
        if (mode !== "diff") renderInto(document.getElementById("wk-rev-body"), r.body);
        panel.focus();
      } catch (err) { errEl.textContent = err.message; errEl.hidden = false; }
    }
    el.querySelectorAll(".wk-history [data-act]").forEach((b) => {
      b.addEventListener("click", async () => {
        const rev = Number(b.closest("[data-rev]").dataset.rev);
        if (b.dataset.act !== "restore") { show(rev, b.dataset.act); return; }
        b.disabled = true;
        try {
          const page = await api("/site/wiki/page/revert", { method: "POST", body: { slug, rev, base_rev: current.rev } });
          location.href = pageHref(page.slug, page.title);
        } catch (err) { errEl.textContent = err.message; errEl.hidden = false; b.disabled = false; }
      });
    });
    const open = document.getElementById("wk-del-open");
    if (open) {
      const form = document.getElementById("wk-del-form");
      open.addEventListener("click", () => { form.hidden = false; open.hidden = true; document.getElementById("wk-del-reason").focus(); });
      document.getElementById("wk-del-cancel").addEventListener("click", () => { form.hidden = true; open.hidden = false; open.focus(); });
      form.addEventListener("submit", async (e) => {
        e.preventDefault();
        try {
          await api("/site/wiki/page/delete", { method: "POST", body: { slug, reason: document.getElementById("wk-del-reason").value, base_rev: current.rev } });
          location.reload();
        } catch (err) { errEl.textContent = err.message; errEl.hidden = false; }
      });
    }
    const want = Number(q("rev"));
    if (want) show(want, want > 1 ? "diff" : "view");
  }

  async function viewSuggestions() {
    const el = toolBody();
    const who = await me();
    if (!who.signed_in) {
      el.innerHTML = `<div class="wk-signin"><p>Sign in to see the suggestions you've sent.</p><p><a class="wk-btn wk-btn-primary" href="${esc(signInUrl())}">Sign in</a></p></div>`;
      return;
    }
    const status = q("status") || "pending";
    const tabs = ["pending", "accepted", "rejected", "withdrawn"].map((s) =>
      `<a class="wk-chip${s === status ? " is-active" : ""}" href="?status=${s}"${s === status ? ' aria-current="page"' : ""}>${s[0].toUpperCase() + s.slice(1)}</a>`).join("");
    el.innerHTML = `<p class="wk-lead">${who.is_editor ? "Changes players have suggested. Accept one to publish it under their name." : "Changes you've suggested, and what happened to them."}</p><nav class="wk-chips" aria-label="Filter by status">${tabs}</nav><div id="wk-sugg-list"><p class="wk-list-empty">Loading…</p></div>`;
    const list = document.getElementById("wk-sugg-list");
    try {
      const d = await api(`/site/wiki/suggestions?status=${status}&limit=100`);
      list.innerHTML = d.items.length
        ? `<ul class="wk-list">${d.items.map((s) => `
          <li class="wk-row">
            <a class="wk-row-title" href="/-/suggestions/${esc(s.id)}">${esc(s.title)}</a>
            <span class="wk-row-meta">${s.base_rev === 0 ? '<span class="wk-pill">New page</span> ' : ""}${esc(s.author || "someone")} · ${timeTag(s.created_at)}</span>
            ${s.summary ? `<span class="wk-row-summary">${esc(s.summary)}</span>` : ""}
          </li>`).join("")}</ul>`
        : `<p class="wk-list-empty">Nothing ${status} here.</p>`;
    } catch (err) { fail(list, err); }
  }

  async function viewSuggestion() {
    const el = toolBody();
    const sid = document.getElementById("wk-tool").dataset.sid;
    const who = await me();
    if (!who.signed_in) {
      el.innerHTML = `<div class="wk-signin"><p>Sign in to see this suggestion.</p><p><a class="wk-btn wk-btn-primary" href="${esc(signInUrl())}">Sign in</a></p></div>`;
      return;
    }
    let s;
    try { s = await api("/site/wiki/suggestions/" + encodeURIComponent(sid)); }
    catch (err) { fail(el, err); return; }
    const cur = s.current;
    const stale = (cur ? cur.rev : 0) !== s.base_rev;
    const href = pageHref(s.slug, s.title);
    const statusLine = s.status === "pending" ? "Waiting for review"
      : `${s.status[0].toUpperCase() + s.status.slice(1)}${s.resolved_by && s.status !== "withdrawn" ? " by " + esc(s.resolved_by) : ""} ${timeTag(s.resolved_at)}`;
    el.innerHTML = `
      <div class="wk-card wk-sugg-head">
        <p><strong>${esc(s.author || "someone")}</strong> suggested ${s.base_rev === 0 ? "a new page" : `a change to <a href="${esc(href)}">${esc(cur ? cur.title : s.title)}</a>`} ${timeTag(s.created_at)}.</p>
        ${s.summary ? `<p class="wk-row-summary">“${esc(s.summary)}”</p>` : ""}
        <p class="wk-row-meta"><span class="wk-pill">${statusLine}</span></p>
        ${s.note ? `<p>Reviewer's note: ${esc(s.note)}</p>` : ""}
        ${s.status === "pending" && stale ? `<p class="wk-hint">The page has changed since this was written, so it can't be applied as-is. Open it in the editor to merge it by hand.</p>` : ""}
      </div>
      <div class="wk-tabs" role="tablist" aria-label="Suggestion view">
        <button type="button" role="tab" id="wk-st-diff" aria-selected="true" aria-controls="wk-sp-diff">Changes</button>
        <button type="button" role="tab" id="wk-st-view" aria-selected="false" aria-controls="wk-sp-view" tabindex="-1">Preview</button>
      </div>
      <div id="wk-sp-diff" role="tabpanel" aria-labelledby="wk-st-diff">${cur && cur.title !== s.title ? `<p class="wk-hint">Title: “${esc(cur.title)}” → “${esc(s.title)}”</p>` : ""}${diffHtml(cur && !cur.deleted ? cur.body : "", s.body)}</div>
      <div id="wk-sp-view" role="tabpanel" aria-labelledby="wk-st-view" class="wk-prose" hidden></div>
      ${s.status === "pending" && who.is_editor ? `
      <div class="wk-actions">
        ${stale ? "" : `<button type="button" class="wk-btn wk-btn-primary" data-act="accept">Accept and publish</button>`}
        <a class="wk-btn" href="/-/edit/${esc(s.slug)}?suggestion=${esc(s.id)}">Edit, then publish</a>
        <button type="button" class="wk-btn" data-act="reject-open">Reject…</button>
      </div>
      <form class="wk-inline-form" id="wk-reject-form" hidden>
        <label for="wk-reject-note">Tell ${esc(s.author || "them")} why <span class="wk-optional">(optional, they'll see it)</span></label>
        <input id="wk-reject-note" maxlength="500" autocomplete="off">
        <div class="wk-actions"><button type="submit" class="wk-btn wk-btn-danger">Reject</button></div>
      </form>` : ""}
      ${s.status === "pending" && !who.is_editor ? `<div class="wk-actions"><button type="button" class="wk-btn" data-act="withdraw">Withdraw my suggestion</button></div>` : ""}
      <p class="wk-error" id="wk-sugg-error" role="alert" hidden></p>`;
    wireTabs(el.querySelector(".wk-tabs"), (id) => {
      if (id === "wk-st-view") renderInto(document.getElementById("wk-sp-view"), s.body);
    });
    const errEl = document.getElementById("wk-sugg-error");
    const run = async (btn, path, body, done) => {
      btn.disabled = true; errEl.hidden = true;
      try { const r = await api(path, { method: "POST", body: body || {} }); done(r); }
      catch (err) { errEl.textContent = err.message; errEl.hidden = false; btn.disabled = false; }
    };
    const act = (name) => el.querySelector(`[data-act="${name}"]`);
    if (act("accept")) act("accept").addEventListener("click", (e) => run(e.currentTarget, `/site/wiki/suggestions/${sid}/accept`, null, (p) => { location.href = pageHref(p.slug, p.title); }));
    if (act("withdraw")) act("withdraw").addEventListener("click", (e) => run(e.currentTarget, `/site/wiki/suggestions/${sid}/withdraw`, null, () => location.reload()));
    if (act("reject-open")) {
      const form = document.getElementById("wk-reject-form");
      act("reject-open").addEventListener("click", () => { form.hidden = false; document.getElementById("wk-reject-note").focus(); });
      form.addEventListener("submit", (e) => {
        e.preventDefault();
        run(form.querySelector("button"), `/site/wiki/suggestions/${sid}/reject`, { note: document.getElementById("wk-reject-note").value }, () => location.reload());
      });
    }
  }

  // Accessible tabs: arrow keys move between them; onShow(id) runs on select.
  function wireTabs(list, onShow) {
    if (!list) return;
    const tabs = Array.from(list.querySelectorAll('[role="tab"]'));
    const select = (tab) => {
      tabs.forEach((t) => {
        const on = t === tab;
        t.setAttribute("aria-selected", String(on));
        t.tabIndex = on ? 0 : -1;
        document.getElementById(t.getAttribute("aria-controls")).hidden = !on;
      });
      if (onShow) onShow(tab.id);
    };
    tabs.forEach((t, i) => {
      t.addEventListener("click", () => select(t));
      t.addEventListener("keydown", (e) => {
        const d = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
        if (!d) return;
        e.preventDefault();
        const next = tabs[(i + d + tabs.length) % tabs.length];
        next.focus(); select(next);
      });
    });
  }

  window.BTTWiki = { api, me, slugify, pageHref, renderInto, wireTabs, signInUrl, q };

  function boot() {
    document.querySelectorAll("time[data-rel]").forEach((t) => {
      const iso = t.getAttribute("datetime");
      t.title = new Date(iso).toLocaleString();
      t.textContent = ago(iso);
    });
    renderPageBody();
    applyViewer();
    const view = document.body.dataset.view;
    if (view === "class" && window.BTTClassLevel) {
      window.BTTClassLevel.wireLevel(document.getElementById("wk-classdata"), readJSON("wk-class-json"));
    }
    const views = { home: viewHome, search: viewSearch, recent: viewRecent, pages: viewPages,
                    history: viewHistory, suggestions: viewSuggestions, suggestion: viewSuggestion };
    if (views[view]) views[view]();
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", boot);
  else boot();
})();

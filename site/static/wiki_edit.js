/* ===========================================================================
   wiki_edit.js - the wiki editor (/-/edit/<slug>, /-/new).

   Editors save straight to the page; every other signed-in reader sends the
   same form as a suggestion for an editor to review. Saves are compare-and-set
   on the revision the editor started from, so a clash is reported, never lost.
   ========================================================================== */
(function () {
  "use strict";

  const W = window.BTTWiki;
  const root = document.getElementById("wk-editor");
  if (!W || !root) return;

  const $ = (id) => document.getElementById(id);
  const form = $("wk-ed-form");
  const titleIn = $("wk-ed-title");
  const slugIn = $("wk-ed-slug");
  const bodyIn = $("wk-ed-body");
  const summaryIn = $("wk-ed-summary");
  const errEl = $("wk-ed-error");
  const submit = $("wk-ed-submit");
  const fixedTitle = root.dataset.fixedTitle || "";

  let slug = root.dataset.slug || "";
  let baseRev = 0;
  let suggestionId = null;
  let isEditor = false;
  let dirty = false;
  let slugTouched = false;

  function showError(msg) {
    errEl.textContent = msg;
    errEl.hidden = !msg;
    if (msg) errEl.scrollIntoView({ block: "nearest" });
  }

  function currentTitle() { return fixedTitle || (titleIn ? titleIn.value.trim() : ""); }

  async function load() {
    const who = await W.me();
    if (!who.signed_in) {
      $("wk-ed-signin-link").href = W.signInUrl();
      $("wk-ed-signin").hidden = false;
      return;
    }
    isEditor = who.is_editor;
    $("wk-ed-mode").textContent = isEditor
      ? "You're an editor: saving publishes straight away."
      : "An editor reviews your change before it goes live. It's published under your name.";
    submit.textContent = isEditor ? "Save" : "Send suggestion";

    const isNew = !slug;
    if (isNew) {
      $("wk-ed-slug-field").hidden = false;
      const want = W.q("slug");
      if (want) { slugIn.value = W.slugify(want); slugTouched = true; }
      if (W.q("title")) { titleIn.value = W.q("title"); if (!slugTouched) slugIn.value = W.slugify(titleIn.value); }
    } else {
      try {
        const page = await W.api("/site/wiki/page?slug=" + encodeURIComponent(slug));
        baseRev = page.rev;
        if (titleIn) titleIn.value = page.title;
        bodyIn.value = page.deleted ? "" : page.body;
        if (page.deleted) $("wk-ed-mode").textContent += " This page was deleted; saving brings it back.";
      } catch (err) {
        if (err.status !== 404) { form.hidden = false; showError(err.message); return; }
      }
    }

    // An editor finishing someone's suggestion: their text, the page's current revision.
    const sid = W.q("suggestion");
    if (sid && isEditor) {
      try {
        const s = await W.api("/site/wiki/suggestions/" + encodeURIComponent(sid));
        if (s.status === "pending" && s.slug === (slug || s.slug)) {
          suggestionId = s.id;
          slug = s.slug;
          baseRev = s.current ? s.current.rev : 0;
          if (titleIn) titleIn.value = s.title;
          if (slugIn) { slugIn.value = s.slug; slugTouched = true; }
          bodyIn.value = s.body;
          summaryIn.value = s.summary || "";
          $("wk-ed-mode").textContent = `Editing ${s.author || "a player"}'s suggestion. Publishing credits them and closes it.`;
          submit.textContent = "Publish";
        }
      } catch (err) { showError(err.message); }
    }

    const back = slug ? W.pageHref(slug, currentTitle()) : "/";
    $("wk-ed-cancel").href = suggestionId ? "/-/suggestions/" + suggestionId : back;
    form.hidden = false;
    (titleIn && !titleIn.value ? titleIn : bodyIn).focus();
  }

  if (titleIn && slugIn) {
    titleIn.addEventListener("input", () => { if (!slugTouched) slugIn.value = W.slugify(titleIn.value); });
    slugIn.addEventListener("input", () => { slugTouched = true; });
    slugIn.addEventListener("blur", () => { slugIn.value = W.slugify(slugIn.value); });
  }
  form.addEventListener("input", () => { dirty = true; });
  window.addEventListener("beforeunload", (e) => { if (dirty) { e.preventDefault(); e.returnValue = ""; } });

  W.wireTabs(form.querySelector(".wk-tabs"), (id) => {
    if (id === "wk-tab-preview") {
      const md = bodyIn.value.trim();
      const out = $("wk-ed-preview");
      if (md) W.renderInto(out, md);
      else out.innerHTML = '<p class="wk-list-empty">Nothing to preview yet.</p>';
    }
  });

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    showError("");
    const target = slug || (slugIn ? W.slugify(slugIn.value) : "");
    if (!fixedTitle && !currentTitle()) { showError("Give the page a title."); titleIn.focus(); return; }
    if (!target) { showError("Give the page an address."); slugIn.focus(); return; }
    if (!bodyIn.value.trim() && !fixedTitle) { showError("Write something first."); bodyIn.focus(); return; }
    const payload = {
      slug: target, title: fixedTitle ? null : currentTitle(), body: bodyIn.value,
      summary: summaryIn.value.trim() || null, base_rev: baseRev,
    };
    if (suggestionId) payload.suggestion_id = suggestionId;
    submit.disabled = true;
    try {
      if (isEditor) {
        const page = await W.api("/site/wiki/page", { method: "POST", body: payload });
        dirty = false;
        location.href = W.pageHref(page.slug, page.title);
        return;
      }
      const s = await W.api("/site/wiki/suggestions", { method: "POST", body: payload });
      dirty = false;
      form.hidden = true;
      $("wk-ed-mode").textContent = "";
      const done = document.createElement("div");
      done.className = "wk-card";
      done.setAttribute("role", "status");
      done.innerHTML = `<p><strong>Thanks, your suggestion is in.</strong> An editor will review it; you can follow it under My suggestions.</p>
        <p class="wk-card-actions"><a class="wk-btn wk-btn-primary" href="/-/suggestions/${encodeURIComponent(s.id)}">View my suggestion</a>
        <a class="wk-btn" href="${W.pageHref(target, currentTitle())}">Back to the page</a></p>`;
      root.appendChild(done);
      done.querySelector("a").focus();
    } catch (err) {
      showError(err.message);
      submit.disabled = false;
    }
  });

  load();
})();

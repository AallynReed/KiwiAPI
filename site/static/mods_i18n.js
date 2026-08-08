/* ═══════════════════════════════════════════════════════════════════════
   Mods Hub - creator-written translations (window.ModsI18n)
   ───────────────────────────────────────────────────────────────────────
   Every piece of prose a modder writes - a mod's title, summary, About text,
   warnings, README, each release's title + changelog, and their own profile -
   can be written again in any language the site speaks. English is always the
   base and the fallback, so a partial translation never leaves a blank.

   This file owns the two shared pieces: the reader-facing SWITCH (a pill per
   language) and the EDITOR (a tab strip + "add a language" picker over one or
   more fields, each keeping a draft per language). Picking which version a
   given reader sees is BTTUtil.localized/textVersions/pickLang; the language
   table itself comes from i18n.js.
   ═══════════════════════════════════════════════════════════════════════ */
(function () {
  'use strict';
  const { esc } = window.BTTUtil;
  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);

  const LANGS = (window.BTTi18n && window.BTTi18n.langs) || [['en', 'English', '🇬🇧']];
  const ORDER = LANGS.map((l) => l[0]);
  // lowercased code -> canonical code, for matching README.zh-cn.md & friends.
  const BY_LOWER = {};
  LANGS.forEach(([c]) => { BY_LOWER[c.toLowerCase()] = c; });

  const langName = (c) => (LANGS.find((l) => l[0] === c) || [c, c])[1];
  const langFlag = (c) => (LANGS.find((l) => l[0] === c) || [c, c, '🌐'])[2];
  const sortLangs = (codes) => codes.slice().sort((a, b) => ORDER.indexOf(a) - ORDER.indexOf(b));

  // The reader's switch: one pill per language, `active` selected. Nothing at
  // all when there's only one language to read. Clicks are picked up by the
  // page (delegated on [data-content-lang]).
  function tabsHTML(codes, active) {
    if (!codes || codes.length < 2) return '';
    return `<div class="mp-langtabs" role="tablist" aria-label="${esc(t('Content language'))}">${
      sortLangs(codes).map((c) => `<button type="button" role="tab" class="mp-langtab${c === active ? ' is-sel' : ''}"
        aria-selected="${c === active}" data-content-lang="${esc(c)}"><span aria-hidden="true">${langFlag(c)}</span> ${esc(langName(c))}</button>`).join('')}</div>`;
  }

  // The editor's markup half - drop it above the fields it governs. `id`
  // namespaces it so several editors can share one modal.
  function editorHTML(id) {
    return `<div class="mp-form-field"><span>${esc(t('Language'))}</span>
      <div class="mp-langedit">
        <div class="mp-langtabs" id="${id}-tabs" role="tablist"></div>
        <select class="mp-langadd" id="${id}-addlang" aria-label="${esc(t('Add a language'))}"></select>
      </div>
      <p class="mp-form-hint">${esc(t('Everyone sees English by default. Add a language and readers who use it see your version instead.'))}</p>
    </div>`;
  }

  // The editor's behaviour half: one tab strip over one or more fields, each
  // keeping a draft per language. Switching the tab swaps every field at once,
  // so a modder writes a whole language in one pass. `fields` are
  // {area, base, translations, labelEl, label}; collect() returns one
  // {base, translations} per field, in the same order.
  function wireEditor(form, id, fields) {
    const drafts = fields.map((f) => {
      const d = { en: f.base || '' };
      Object.keys(f.translations || {}).forEach((c) => {
        if (BY_LOWER[String(c).toLowerCase()]) d[c] = f.translations[c];
      });
      return d;
    });
    // A language is in the editor if ANY of the fields has it.
    const langs = new Set(['en']);
    drafts.forEach((d) => Object.keys(d).forEach((c) => langs.add(c)));
    let cur = 'en';
    const tabs = form.querySelector('#' + id + '-tabs');
    const addSel = form.querySelector('#' + id + '-addlang');
    const stash = () => fields.forEach((f, i) => { drafts[i][cur] = f.area.value; });
    const show = () => fields.forEach((f, i) => {
      f.area.value = drafts[i][cur] || '';
      if (f.labelEl) f.labelEl.textContent = cur === 'en' ? f.label : f.label + ' - ' + langName(cur);
    });

    function paint() {
      tabs.innerHTML = sortLangs([...langs]).map((c) =>
        `<button type="button" role="tab" class="mp-langtab${c === cur ? ' is-sel' : ''}"
          aria-selected="${c === cur}" data-lang="${esc(c)}"><span aria-hidden="true">${langFlag(c)}</span> ${esc(langName(c))}</button>`).join('');
      tabs.querySelectorAll('[data-lang]').forEach((b) => b.addEventListener('click', () => {
        stash();
        cur = b.getAttribute('data-lang');
        show();
        paint();
        fields[0].area.focus();
      }));
      const rest = ORDER.filter((c) => !langs.has(c));
      addSel.innerHTML = `<option value="">${esc(t('Add a language…'))}</option>`
        // The space lives inside the interpolation on purpose - see the
        // template-literal note in scripts/minify_static.py.
        + rest.map((c) => `<option value="${esc(c)}">${langFlag(c) + ' ' + esc(langName(c))}</option>`).join('');
      addSel.hidden = !rest.length;
    }
    addSel.addEventListener('change', () => {
      const code = addSel.value;
      if (!code) return;
      stash();
      langs.add(code);
      cur = code;
      show();
      paint();
      fields[0].area.focus();
    });
    show();
    paint();

    return function collect() {
      stash();
      return drafts.map((d) => {
        const out = {};
        Object.keys(d).forEach((c) => { if (c !== 'en') out[c] = d[c]; });
        return { base: d.en, translations: out };
      });
    };
  }

  window.ModsI18n = { LANGS, BY_LOWER, langName, langFlag, sortLangs, tabsHTML, editorHTML, wireEditor };
})();

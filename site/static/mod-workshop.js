/* ═══════════════════════════════════════════════════════════════════════
   /mod-workshop - build a Trove mod from your own files, or open one up
   ───────────────────────────────────────────────────────────────────────
   The browser only ever holds File handles - nothing is read into memory
   until there is something to send, and nothing is sent until the person
   at the keyboard asks for it.

   Placement is the whole point of the page. A file only overrides the
   game if it sits at the exact path the game keeps the original at, and
   only the server knows what those paths are (it reads the archived game
   tree), so the check is a round trip. To keep the "put things in the
   right place" switch instant, the server sends BOTH answers up front -
   what happens to each file if it is moved, and what happens if it is
   left alone - and flipping the switch just picks between them.

   The build re-runs the whole check server-side regardless, so what
   comes back is always what the page described.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const { esc, apiUrl } = window.BTTUtil;

  // Mirrors app/core/config.mod_workshop_max_request_body_bytes - caught here so
  // an oversized pick fails with a sentence instead of a 413.
  const MAX_BYTES = 32 * 1024 * 1024;
  const MAX_FILES = 4000;

  const state = {
    tab: 'make',
    source: null,      // {kind:'files'|'zip'|'tmod', files:[File], paths:[], archive:File, header:{}}
    plan: null,
    fix: true,
    keep: new Set(),   // original paths to leave exactly where they are
    filter: 'all',
    busy: false,
    open: null,        // {file, data} - shared by the Open and Analyze tabs
    openFilter: '',
  };

  const $ = (id) => document.getElementById(id);

  // The four tabs, in order: two that build a mod (sharing everything after their
  // own file picker) and two that read one (sharing the mod they loaded).
  const TABS = [
    { name: 'make', tab: $('mw-tab-make'), panel: $('mw-panel-make') },
    { name: 'zip', tab: $('mw-tab-zip'), panel: $('mw-panel-zip') },
    { name: 'open', tab: $('mw-tab-open'), panel: $('mw-panel-open') },
    { name: 'analyze', tab: $('mw-tab-analyze'), panel: $('mw-panel-analyze') },
  ];
  const BUILD_TABS = ['make', 'zip'];

  const $flow = $('mw-flow');
  const $drop = $('mw-drop');
  const $zipDrop = $('mw-zip-drop');
  const $source = $('mw-source');
  const $sourceName = $('mw-source-name');
  const $sourceMeta = $('mw-source-meta');
  const $sourceError = $('mw-source-error');
  const $stepCheck = $('mw-step-check');
  const $stepBuild = $('mw-step-build');
  const $summary = $('mw-summary');
  const $fix = $('mw-fix');
  const $files = $('mw-files');
  const $goCount = $('mw-go-count');
  const $configBox = $('mw-config-box');
  const $config = $('mw-config');
  const $build = $('mw-build');
  const $status = $('mw-status');

  const $openError = $('mw-open-error');
  const $openDrop = $('mw-open-drop');
  const $openResult = $('mw-open-result');
  const $openHint = $('mw-open-hint');
  const $openFiles = $('mw-open-files');
  const $openFilter = $('mw-open-filter');
  const $openZip = $('mw-open-zip');
  const $openStatus = $('mw-open-status');

  const $anDrop = $('mw-an-drop');
  const $anResult = $('mw-an-result');
  const $anHint = $('mw-an-hint');
  const $anMeta = $('mw-an-meta');
  const $anSummary = $('mw-an-summary');
  const $anFiles = $('mw-an-files');
  const $anRepair = $('mw-an-repair');
  const $anStatus = $('mw-an-status');

  const $inputFiles = $('mw-input-files');
  const $inputFolder = $('mw-input-folder');
  const $inputZip = $('mw-input-zip');
  const $inputOpen = $('mw-input-open');

  wire();

  // ─── Tabs ──────────────────────────────────────────────────────────

  function showTab(which) {
    state.tab = which;
    for (const entry of TABS) {
      const on = entry.name === which;
      entry.tab.classList.toggle('active', on);
      entry.tab.setAttribute('aria-selected', String(on));
      entry.tab.tabIndex = on ? 0 : -1;
      entry.panel.hidden = !on;
    }
    // The build steps are identical whichever way the files arrived, so one copy
    // follows the active picker instead of a second set going quietly out of sync.
    if (BUILD_TABS.includes(which)) {
      TABS.find((e) => e.name === which).panel.appendChild($flow);
      $flow.hidden = !state.source;
    } else {
      $flow.hidden = true;
    }
  }

  // ─── Picking files ─────────────────────────────────────────────────

  /** One .zip or .tmod on its own is an archive to unpack; anything else is a
      set of loose files, keyed by the path the picker gave them. */
  function setSource(list) {
    const picked = Array.from(list || []).filter((f) => f && f.size >= 0);
    if (!picked.length) return;
    reset();
    $flow.hidden = false;

    const total = picked.reduce((sum, f) => sum + f.size, 0);
    if (total > MAX_BYTES) {
      showSourceError(t('That is bigger than this page can take in one go') +
        ` (${formatBytes(total)} / ${formatBytes(MAX_BYTES)}).`);
      return;
    }
    if (picked.length > MAX_FILES) {
      showSourceError(t('That is more files than one mod can hold.'));
      return;
    }

    const only = picked.length === 1 ? picked[0] : null;
    const ext = only ? only.name.toLowerCase().slice(only.name.lastIndexOf('.')) : '';
    // The zip tab asks one question, so it only takes one kind of answer - saying
    // so beats letting a folder through and reporting nothing useful afterwards.
    if (state.tab === 'zip' && ext !== '.zip') {
      showSourceError(t('That isn’t a zip. Use “Make a mod” for loose files, or “Open a mod” for a finished one.'));
      return;
    }
    if (only && (ext === '.zip' || ext === '.tmod')) {
      state.source = { kind: ext === '.zip' ? 'zip' : 'tmod', archive: only,
                       files: [only], paths: [], header: {} };
    } else {
      state.source = {
        kind: 'files', files: picked, archive: null, header: {},
        paths: picked.map((f) => f.webkitRelativePath || f.name),
      };
    }
    describeSource();
    refresh();
  }

  function describeSource() {
    const s = state.source;
    if (!s) { $source.hidden = true; return; }
    const total = s.files.reduce((sum, f) => sum + f.size, 0);
    $source.hidden = false;
    $sourceName.textContent = s.archive
      ? s.archive.name
      : (rootFolder(s.paths) || t('%n files').replace('%n', String(s.files.length)));
    $sourceMeta.textContent = s.archive
      ? formatBytes(total)
      : `${s.files.length} · ${formatBytes(total)}`;
  }

  function rootFolder(paths) {
    const first = (paths[0] || '').split('/')[0];
    return first && paths.every((p) => p.split('/')[0] === first) && paths[0].includes('/')
      ? first : '';
  }

  function reset() {
    state.source = null;
    state.plan = null;
    state.keep.clear();
    state.filter = 'all';
    $flow.hidden = true;
    $source.hidden = true;
    $sourceError.hidden = true;
    $stepCheck.hidden = true;
    $stepBuild.hidden = true;
    setStatus('');
    for (const chip of document.querySelectorAll('.mw-chip')) {
      chip.classList.toggle('active', chip.dataset.filter === 'all');
    }
  }

  function showSourceError(message) {
    $flow.hidden = false;
    $sourceError.textContent = message;
    $sourceError.hidden = false;
  }

  // ─── The placement check ───────────────────────────────────────────

  async function refresh() {
    const s = state.source;
    if (!s) return;
    $stepCheck.hidden = false;
    $summary.hidden = false;
    $summary.innerHTML = `<p class="mw-loading">${esc(t('Checking your files…'))}</p>`;
    $files.innerHTML = '';

    const form = new FormData();
    if (s.archive) form.append('archive', s.archive, s.archive.name);
    else form.append('paths', JSON.stringify(s.paths));

    try {
      const res = await fetch(apiUrl('/site/mod-workshop/inspect'),
                              { method: 'POST', body: form });
      const data = await readJSON(res);
      state.plan = data;
      s.header = data.properties || {};
      s.configCandidates = data.config_candidates || [];
      prefillDetails(s.header);
      render();
    } catch (err) {
      state.plan = null;
      $stepCheck.hidden = true;
      showSourceError(message(err));
    }
  }

  /** What each file ends up doing, after the switch and any per-file opt-outs.
      A file that lands where another one already landed loses - same rule, same
      order, as the server's own pass. */
  function resolve() {
    const entries = (state.plan && state.plan.entries) || [];
    const taken = new Map();
    return entries.map((entry) => {
      const alt = entry.alt;
      const leaveAlone = !state.fix || state.keep.has(entry.path);
      const chosen = (alt && leaveAlone) ? Object.assign({}, entry, alt) : entry;
      const out = {
        index: entry.index, path: entry.path, name: entry.name, size: entry.size,
        expected: entry.expected, movable: !!alt || entry.status === 'moved',
        status: chosen.status, final: chosen.final, reason: chosen.reason,
        kept: leaveAlone && !!entry.expected,
      };
      if (out.status !== 'skipped') {
        const clash = taken.get(out.final);
        if (clash !== undefined) {
          out.status = 'skipped';
          out.reason = t('another file already lands there');
        } else {
          taken.set(out.final, out.index);
        }
      }
      return out;
    });
  }

  function render() {
    if (!state.plan) return;
    const rows = resolve();
    const counts = { ready: 0, moved: 0, misplaced: 0, skipped: 0 };
    for (const r of rows) counts[r.status] = (counts[r.status] || 0) + 1;
    const packed = rows.length - counts.skipped;

    renderSummary($summary, counts, packed, state.plan, 'make');
    renderRows($files, rows);

    $stepBuild.hidden = packed === 0;
    $build.disabled = packed === 0 || state.busy;
    $goCount.innerHTML = packed
      ? `<strong>${esc(formatInt(packed))}</strong> ${esc(packed === 1 ? t('file goes in') : t('files go in'))}`
      : '';
    renderConfigPicker(rows);
  }

  function renderSummary(target, counts, packed, plan, mode) {
    const making = mode === 'make';
    const bits = [];
    // Building, the count going in IS the outcome. Reading, it isn't worth a
    // headline - every file already wears a tick or a cross - so only the ones
    // that need doing something about get called out, and a mod with nothing
    // wrong says nothing at all.
    if (packed && making) {
      bits.push(pill('good', 'fa-circle-check',
        t('%n going in').replace('%n', formatInt(packed))));
    }
    if (counts.moved) {
      bits.push(pill('move', 'fa-arrow-right-arrow-left',
        t('%n put in place').replace('%n', formatInt(counts.moved))));
    }
    if (counts.misplaced) {
      bits.push(pill('warn', 'fa-triangle-exclamation',
        t('%n in the wrong place').replace('%n', formatInt(counts.misplaced))));
    }
    if (counts.skipped) {
      bits.push(pill('mute', 'fa-circle-minus',
        (making ? t('%n left out') : t('%n the game ignores'))
          .replace('%n', formatInt(counts.skipped))));
    }

    const notes = [];
    if (plan.wrapper) {
      notes.push(t('Everything was inside “%s” — that folder is ignored, the mod starts below it.')
        .replace('%s', plan.wrapper.replace(/\/$/, '')));
    }
    if (!plan.game_index_available) {
      notes.push(t("The game's own file list isn't available right now, so files can only be checked against Trove's folder rules — not against where the game actually keeps them."));
    }
    if (!packed && making) {
      notes.push(t('Nothing here would load in-game. Trove only reads files inside its own folders — blueprints, ui, prefabs, textures and the rest.'));
    }

    target.hidden = !bits.length && !notes.length;   // nothing to say, no empty box
    target.innerHTML =
      (bits.length ? `<div class="mw-pills">${bits.join('')}</div>` : '') +
      notes.map((n) => `<p class="mw-note">${esc(n)}</p>`).join('');
  }

  function pill(kind, icon, label) {
    return `<span class="mw-pill ${kind}"><i class="fa-solid ${icon}" aria-hidden="true"></i>${esc(label)}</span>`;
  }

  function renderRows(target, rows) {
    const shown = rows.filter((r) => {
      if (state.filter === 'packed') return r.status !== 'skipped';
      if (state.filter === 'moved') return r.status === 'moved' || r.status === 'misplaced';
      if (state.filter === 'skipped') return r.status === 'skipped';
      return true;
    });
    if (!shown.length) {
      target.innerHTML = `<p class="mw-empty">${esc(t('Nothing to show here.'))}</p>`;
      return;
    }
    target.innerHTML = shown.map((r) => {
      const moved = r.status === 'moved';
      const label = {
        ready: t('Ready'), moved: t('Moved'),
        misplaced: t('Wrong place'), skipped: t('Left out'),
      }[r.status] || r.status;
      const detail = moved
        ? `<span class="mw-row-to"><i class="fa-solid fa-arrow-right" aria-hidden="true"></i> ${esc(r.final)}</span>`
        : r.status === 'misplaced'
          ? `<span class="mw-row-warn">${esc(t('the game keeps it at'))} ${esc(r.expected || '')}</span>`
          : r.status === 'skipped'
            ? `<span class="mw-row-why">${esc(r.reason || '')}</span>`
            : '';
      const keep = r.movable
        ? `<label class="mw-row-keep">
             <input type="checkbox" data-keep="${esc(r.path)}"${r.kept ? ' checked' : ''}>
             <span>${esc(t('leave it'))}</span>
           </label>`
        : '';
      return `<div class="mw-row ${esc(r.status)}">
        <span class="mw-row-status">${esc(label)}</span>
        <span class="mw-row-path">
          <span class="mw-row-name">${esc(r.path)}</span>
          ${detail}
        </span>
        <span class="mw-row-size">${esc(r.size == null ? '' : formatBytes(r.size))}</span>
        ${keep}
      </div>`;
    }).join('');
  }

  // ─── Details + build ───────────────────────────────────────────────

  /** A .tmod opened for repair already knows what it is; don't make anyone retype it. */
  function prefillDetails(header) {
    if (!header) return;
    const set = (el, value) => { if (value && !el.value) el.value = value; };
    set($('mw-title'), header.title);
    set($('mw-author'), header.author);
    set($('mw-version'), header.modVersion);
    set($('mw-notes'), header.notes);
    if (!$('mw-title').value && state.source && state.source.kind === 'files') {
      $('mw-title').value = rootFolder(state.source.paths) || '';
    }
  }

  /** A settings file only means anything to a mod with a Flash interface, and the
      server enforces that too - this just stops the question being asked otherwise. */
  function renderConfigPicker(rows) {
    const candidates = (state.source && state.source.configCandidates) || [];
    const usable = candidates.filter((p) => rows.some((r) => r.path === p));
    $configBox.hidden = !usable.length;
    if (!usable.length) { $config.innerHTML = ''; return; }
    $config.innerHTML = [`<option value="">${esc(t("Don't include one"))}</option>`]
      .concat(usable.map((p) => `<option value="${esc(p)}">${esc(p)}</option>`)).join('');
    $config.value = usable[0];
  }

  async function build() {
    const s = state.source;
    if (!s || state.busy) return;
    state.busy = true;
    $build.disabled = true;
    setStatus(t('Building…'));

    const properties = {
      title: $('mw-title').value.trim(),
      author: $('mw-author').value.trim(),
      modVersion: $('mw-version').value.trim(),
      notes: $('mw-notes').value.trim(),
    };
    const form = new FormData();
    form.append('spec', JSON.stringify({
      properties, fix: state.fix, keep: Array.from(state.keep),
      config_path: $configBox.hidden ? '' : ($config.value || ''),
    }));
    if (s.archive) {
      form.append('archive', s.archive, s.archive.name);
    } else {
      form.append('paths', JSON.stringify(s.paths));
      s.files.forEach((f, i) => form.append('files', f, s.paths[i].split('/').pop()));
    }

    try {
      const res = await fetch(apiUrl('/site/mod-workshop/build'),
                              { method: 'POST', body: form });
      if (!res.ok) throw new Error(await errorText(res));
      const blob = await res.blob();
      save(blob, filenameFrom(res, `${properties.title || 'My Mod'}.tmod`));
      // The server ran the check again on its way to packing; report ITS numbers,
      // not the preview's, so the two can never quietly disagree.
      const packed = Number(res.headers.get('X-Kiwi-Packed') || 0);
      const moved = Number(res.headers.get('X-Kiwi-Moved') || 0);
      const what = (packed === 1 ? t('%n file') : t('%n files')).replace('%n', formatInt(packed));
      setStatus(moved
        ? t('Done — %s packed, %m put in the right place.')
            .replace('%s', what).replace('%m', formatInt(moved))
        : t('Done — %s packed. Check your downloads.').replace('%s', what));
    } catch (err) {
      setStatus(message(err), true);
    } finally {
      state.busy = false;
      $build.disabled = false;
    }
  }

  // ─── Reading a mod (the Open and Analyze tabs) ─────────────────────
  // One mod, two questions. Open answers "give me the files"; Analyze answers
  // "tell me what this is". Both come off the SAME read, so a mod dropped on
  // either tab is already loaded when you switch to the other.

  async function openMod(file) {
    if (!file) return;
    $openError.hidden = true;
    $openResult.hidden = false;
    $anResult.hidden = false;
    $openFiles.innerHTML = `<p class="mw-loading">${esc(t('Opening…'))}</p>`;
    $anMeta.innerHTML = '';
    $anFiles.innerHTML = '';
    $anSummary.hidden = false;
    $anSummary.innerHTML = `<p class="mw-loading">${esc(t('Opening…'))}</p>`;
    setOpenStatus('');
    setAnStatus('');

    const form = new FormData();
    form.append('file', file, file.name);
    try {
      const res = await fetch(apiUrl('/site/mod-workshop/extract'),
                              { method: 'POST', body: form });
      const data = await readJSON(res);
      state.open = { file, data };
      state.openFilter = '';
      $openFilter.value = '';
      renderExtract();
      renderAnalysis();
    } catch (err) {
      state.open = null;
      $openResult.hidden = true;
      $anResult.hidden = true;
      $openError.textContent = message(err);
      $openError.hidden = false;
    }
  }

  /** Open: a flat list of everything packed inside, each one savable. */
  function renderExtract() {
    const { file, data } = state.open;
    const files = data.files || [];
    const title = (data.properties || {}).title || file.name;
    $openHint.textContent =
      t('%s — %n files, %z. Save the lot, or just the one you came for.')
        .replace('%s', title)
        .replace('%n', formatInt(data.file_count))
        .replace('%z', formatBytes(data.total_size || data.size));

    const needle = state.openFilter.toLowerCase();
    const shown = needle
      ? files.filter((f) => String(f.path).toLowerCase().includes(needle))
      : files;
    if (!shown.length) {
      $openFiles.innerHTML = `<p class="mw-empty">${esc(t('Nothing matches that.'))}</p>`;
      return;
    }
    $openFiles.innerHTML = shown.map((f) => `<div class="mw-row plain">
      <span class="mw-row-path"><span class="mw-row-name">${esc(f.path)}</span></span>
      <span class="mw-row-size">${esc(formatBytes(f.size))}</span>
      <button type="button" class="mw-icon-btn" data-get="${esc(f.path)}"
              title="${esc(t('Save this file'))}" aria-label="${esc(t('Save this file'))} ${esc(f.path)}">
        <i class="fa-solid fa-download" aria-hidden="true"></i></button>
    </div>`).join('');
  }

  /** Analyze: what the mod IS - its header, whether the game will read it, and a
      browsable tree you can look inside without installing anything. */
  function renderAnalysis() {
    const { file, data } = state.open;
    const props = data.properties || {};
    const rows = (data.entries || []).map((e) => ({
      index: e.index, path: e.path, name: e.name, size: e.size,
      status: e.status, final: e.final, reason: e.reason, expected: e.expected,
    }));
    state.open.rows = rows;
    const byPath = new Map(rows.map((r) => [r.path, r]));
    const off = rows.filter((r) => r.status === 'misplaced').length;

    $anHint.textContent = off
      ? (off === 1
          ? t('One file sits somewhere the game will never look.')
          : t('%n files sit somewhere the game will never look.').replace('%n', formatInt(off)))
      : t('Every file in here sits where the game will read it.');

    $anMeta.innerHTML =
      `<h3 class="mw-open-name">${esc(props.title || file.name)}</h3>` +
      chipsHTML(data) + headerHTML(data);

    const counts = { ready: 0, moved: 0, misplaced: 0, skipped: 0 };
    for (const r of rows) counts[r.status] = (counts[r.status] || 0) + 1;
    renderSummary($anSummary, counts, counts.ready, data, 'open');

    $anFiles.innerHTML = `<h3 class="mw-h">${esc(t('Files'))}</h3>` +
      `<div class="mw-tree">${treeHTML(fileTree(data.files || []), data, byPath, 0)}</div>`;
    $anRepair.hidden = !off || !data.game_index_available;
  }

  /* ── The contents breakdown ──────────────────────────────────────────
     The same three blocks a Mods Hub release shows: what the artifact IS,
     the header the game reads off it, and a browsable tree of what's packed
     inside. Placement is folded into the tree rather than listed twice. */

  function chipsHTML(d) {
    const chip = (icon, text) =>
      `<span class="mw-chip-static"><i class="fa-solid ${icon}" aria-hidden="true"></i>${esc(text)}</span>`;
    return `<div class="mw-chips">
      ${chip('fa-file-zipper', `.tmod · ${formatBytes(d.size)}`)}
      ${chip('fa-folder-tree', `${formatInt(d.file_count)} ${d.file_count === 1 ? t('file') : t('files')}`)}
      ${d.total_size ? chip('fa-box-open', `${t('Unpacked')} ${formatBytes(d.total_size)}`) : ''}
      ${d.version != null ? chip('fa-code-branch', `${t('Format version')} ${d.version}`) : ''}
    </div>`;
  }

  function headerHTML(d) {
    const props = d.properties || {};
    const keys = Object.keys(props).sort((a, b) => (a.toLowerCase() < b.toLowerCase() ? -1 : 1));
    const cats = (d.categories || []).length
      ? `<p class="mw-cats">${d.categories.map((c) => `<span class="mw-tag">${esc(c)}</span>`).join('')}</p>`
      : '';
    if (!keys.length) {
      return `<h3 class="mw-h">${esc(t('Header'))}</h3>` +
        `<p class="mw-empty">${esc(t('This mod carries no header.'))}</p>`;
    }
    return `<h3 class="mw-h">${esc(t('Header'))}</h3>${cats}` +
      `<table class="mw-props"><tbody>${keys.map((k) =>
        `<tr><th>${esc(k)}</th><td>${esc(props[k])}</td></tr>`).join('')}</tbody></table>`;
  }

  /** Flat paths → nested folders. A folder keeps its own total so a collapsed one
      still says how much is under it. */
  function fileTree(files) {
    const root = { dirs: new Map(), files: [], size: 0 };
    for (const f of files) {
      const parts = String(f.path || '').split('/').filter(Boolean);
      let node = root;
      node.size += Number(f.size || 0);
      for (const part of parts.slice(0, -1)) {
        if (!node.dirs.has(part)) node.dirs.set(part, { dirs: new Map(), files: [], size: 0 });
        node = node.dirs.get(part);
        node.size += Number(f.size || 0);
      }
      node.files.push(f);
    }
    return root;
  }

  function treeHTML(node, d, byPath, depth) {
    const dirs = [...node.dirs.entries()]
      .sort((a, b) => (a[0].toLowerCase() < b[0].toLowerCase() ? -1 : 1));
    const folders = dirs.map(([name, child]) => `
      <details class="mw-dir"${depth < 1 ? ' open' : ''}>
        <summary>
          <i class="fa-solid fa-folder" aria-hidden="true"></i>
          <span class="mw-dir-name">${esc(name)}</span>
          <span class="mw-dir-meta">${esc(formatInt(countFiles(child)))} · ${esc(formatBytes(child.size))}</span>
        </summary>
        <div class="mw-dir-kids">${treeHTML(child, d, byPath, depth + 1)}</div>
      </details>`).join('');

    const files = node.files.map((f) => {
      const path = String(f.path);
      const low = path.toLowerCase();
      const row = byPath.get(path) || {};
      const role = d.preview_path && low === d.preview_path ? t('Preview image')
        : d.config_path && low === d.config_path ? t('Settings') : '';

      // One question per file, answered before its name: will the game read this?
      // A tick means yes. A cross means no - and where we know the path the game
      // uses instead, it's spelled out rather than hidden in a tooltip.
      const ok = row.status === 'ready';
      const verdict = ok ? t('The game will read this file')
        : row.expected ? t("The game won't read it here")
          : (row.reason || t('The game ignores this file'));
      const mark = `<i class="fa-solid ${ok ? 'fa-circle-check mw-ok' : 'fa-circle-xmark mw-bad'}"
                       title="${esc(verdict)}" aria-hidden="true"></i>
                    <span class="sr-only">${esc(verdict)}. </span>`;
      const detail = row.expected
        ? `<span class="mw-file-warn">${esc(t('should be'))} <code>${esc(row.expected)}</code></span>`
        : (!ok && row.reason ? `<span class="mw-file-mute">${esc(row.reason)}</span>` : '');

      const kind = previewKind(path);
      const preview = kind
        ? `<button type="button" class="mw-icon-btn" data-preview="${esc(path)}"
             title="${esc(t('Preview'))}" aria-label="${esc(t('Preview'))} ${esc(path)}">
             <i class="fa-solid ${kind === 'model' ? 'fa-cube' : kind === 'text' ? 'fa-file-lines' : 'fa-image'}" aria-hidden="true"></i></button>`
        : '';
      return `<div class="mw-file ${esc(row.status || '')}">
        ${mark}
        <span class="mw-file-id">
          <span class="mw-file-name" title="${esc(path)}">${esc(path.split('/').pop())}</span>
          ${role ? `<span class="mw-file-role">${esc(role)}</span>` : ''}
          ${detail}
        </span>
        <span class="mw-file-size">${esc(formatBytes(f.size))}</span>
        ${preview}
        <button type="button" class="mw-icon-btn" data-get="${esc(path)}"
                title="${esc(t('Save this file'))}" aria-label="${esc(t('Save this file'))} ${esc(path)}">
          <i class="fa-solid fa-download" aria-hidden="true"></i></button>
      </div>`;
    }).join('');

    return folders + files;
  }

  function countFiles(node) {
    let n = node.files.length;
    node.dirs.forEach((child) => { n += countFiles(child); });
    return n;
  }

  /* ── Previewing one asset ────────────────────────────────────────────
     Everything a browser can show on its own is shown on its own: a picture
     is a picture, a .dds is decoded in the tab (dds_ready.js), text is text.
     A .blueprint is the exception - only the server can turn one into voxels,
     so the model preview posts the mod back and gets the payload the Mods Hub
     viewer already knows how to draw. */

  const IMAGE_EXT = ['.png', '.jpg', '.jpeg', '.gif', '.webp', '.bmp'];
  const TEXT_EXT = ['.txt', '.cfg', '.ini', '.json', '.xml', '.csv', '.md',
                    '.lua', '.js', '.html', '.htm', '.yaml', '.yml', '.log'];

  function previewKind(path) {
    const low = path.toLowerCase();
    if (low.endsWith('.blueprint')) return 'model';
    if (low.endsWith('.dds')) return 'dds';
    if (IMAGE_EXT.some((e) => low.endsWith(e))) return 'image';
    if (TEXT_EXT.some((e) => low.endsWith(e))) return 'text';
    return '';
  }

  async function preview(path) {
    if (!state.open) return;
    const kind = previewKind(path);
    if (kind === 'model') {
      if (!window.BlueprintViewer) { setOpenStatus(t('The 3D viewer is unavailable.'), true); return; }
      const form = new FormData();
      form.append('file', state.open.file, state.open.file.name);
      form.append('path', path);
      window.BlueprintViewer.open({
        url: apiUrl('/site/mod-workshop/preview/blueprint'),
        title: path.split('/').pop(),
        fetchInit: { method: 'POST', body: form },
      });
      return;
    }

    const box = openPreview(path);
    try {
      const buf = await fileBytes(path);
      if (kind === 'text') {
        const text = new TextDecoder().decode(buf).slice(0, 400000);
        box.innerHTML = `<pre class="mw-pre">${esc(text)}</pre>`;
        return;
      }
      if (kind === 'dds') {
        const decodeDDS = await ensureDDS();
        if (!decodeDDS) throw new Error(t('This image format needs a moment — try again.'));
        const img = decodeDDS(buf);
        const canvas = document.createElement('canvas');
        canvas.width = img.width;
        canvas.height = img.height;
        canvas.getContext('2d').putImageData(new ImageData(img.rgba, img.width, img.height), 0, 0);
        canvas.className = 'mw-preview-art';
        box.innerHTML = '';
        box.appendChild(canvas);
        return;
      }
      const url = URL.createObjectURL(new Blob([buf]));
      const image = new Image();
      image.className = 'mw-preview-art';
      image.alt = path;
      image.onload = () => { box.innerHTML = ''; box.appendChild(image); };
      image.onerror = () => { box.innerHTML = `<p class="mw-empty">${esc(t("This file can't be shown here."))}</p>`; };
      image.src = url;
      setTimeout(() => URL.revokeObjectURL(url), 30000);
    } catch (err) {
      box.innerHTML = `<p class="mw-alert bad">${esc(message(err))}</p>`;
    }
  }

  /** One file's bytes, pulled back out of the mod that is still sitting in the tab. */
  async function fileBytes(path) {
    const form = new FormData();
    form.append('file', state.open.file, state.open.file.name);
    form.append('path', path);
    const res = await fetch(apiUrl('/site/mod-workshop/extract/download'),
                            { method: 'POST', body: form });
    if (!res.ok) throw new Error(await errorText(res));
    return res.arrayBuffer();
  }

  let ddsReady = null;
  function ensureDDS() {
    if (window.decodeDDS) return Promise.resolve(window.decodeDDS);
    if (ddsReady) return ddsReady;
    ddsReady = new Promise((done) => {
      document.addEventListener('btt-dds-ready', () => done(window.decodeDDS || null), { once: true });
      setTimeout(() => done(window.decodeDDS || null), 4000);
    });
    return ddsReady;
  }

  /** A plain modal for the flat previews; the 3D one brings its own. */
  function openPreview(path) {
    const overlay = document.createElement('div');
    overlay.className = 'mw-preview';
    overlay.setAttribute('role', 'dialog');
    overlay.setAttribute('aria-modal', 'true');
    overlay.setAttribute('aria-label', path);
    overlay.innerHTML = `<div class="mw-preview-box">
      <div class="mw-preview-head">
        <span class="mw-preview-title"></span>
        <button type="button" class="mw-preview-x" aria-label="${esc(t('Close'))}">
          <i class="fa-solid fa-xmark" aria-hidden="true"></i></button>
      </div>
      <div class="mw-preview-body"><p class="mw-loading">${esc(t('Loading…'))}</p></div>
    </div>`;
    overlay.querySelector('.mw-preview-title').textContent = path;
    document.body.appendChild(overlay);

    let release = null;
    const close = () => {
      document.removeEventListener('keydown', onKey);
      if (release) release();
      overlay.remove();
    };
    const onKey = (e) => { if (e.key === 'Escape') close(); };
    overlay.querySelector('.mw-preview-x').addEventListener('click', close);
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) close(); });
    document.addEventListener('keydown', onKey);
    if (window.BTTUtil && window.BTTUtil.trapFocus) {
      release = window.BTTUtil.trapFocus(overlay.querySelector('.mw-preview-box'));
    }
    return overlay.querySelector('.mw-preview-body');
  }

  /** Saving a file out reports on whichever tab asked for it. */
  async function openDownload(path) {
    if (!state.open) return;
    const report = state.tab === 'analyze' ? setAnStatus : setOpenStatus;
    report(path ? t('Saving…') : t('Packing it up…'));
    const form = new FormData();
    form.append('file', state.open.file, state.open.file.name);
    if (path) form.append('path', path);
    try {
      const res = await fetch(apiUrl('/site/mod-workshop/extract/download'),
                              { method: 'POST', body: form });
      if (!res.ok) throw new Error(await errorText(res));
      const blob = await res.blob();
      const fallback = path ? path.split('/').pop()
        : `${state.open.file.name.replace(/\.tmod$/i, '')}.zip`;
      save(blob, filenameFrom(res, fallback));
      report(t('Done — check your downloads.'));
    } catch (err) {
      report(message(err), true);
    }
  }

  /** Hand the analyzed mod to the build side, files, header and all. */
  function repair() {
    if (!state.open) return;
    const file = state.open.file;
    showTab('make');
    reset();
    for (const id of ['mw-title', 'mw-author', 'mw-version', 'mw-notes']) $(id).value = '';
    $flow.hidden = false;
    state.source = { kind: 'tmod', archive: file, files: [file], paths: [], header: {} };
    state.fix = true;
    $fix.checked = true;
    describeSource();
    refresh();
    TABS[0].panel.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }

  // ─── Wiring ────────────────────────────────────────────────────────

  function wire() {
    TABS.forEach((entry, i) => {
      entry.tab.addEventListener('click', () => showTab(entry.name));
      entry.tab.addEventListener('keydown', (e) => {
        const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
        if (!step) return;
        e.preventDefault();
        const next = TABS[(i + step + TABS.length) % TABS.length];
        showTab(next.name);
        next.tab.focus();
      });
    });

    $('mw-pick-files').addEventListener('click', () => $inputFiles.click());
    $('mw-pick-folder').addEventListener('click', () => $inputFolder.click());
    $('mw-zip-pick').addEventListener('click', () => $inputZip.click());
    $('mw-open-pick').addEventListener('click', () => $inputOpen.click());
    $('mw-an-pick').addEventListener('click', () => $inputOpen.click());
    $inputFiles.addEventListener('change', () => { setSource($inputFiles.files); $inputFiles.value = ''; });
    $inputFolder.addEventListener('change', () => { setSource($inputFolder.files); $inputFolder.value = ''; });
    $inputZip.addEventListener('change', () => { setSource($inputZip.files); $inputZip.value = ''; });
    $inputOpen.addEventListener('change', () => {
      openMod($inputOpen.files && $inputOpen.files[0]);
      $inputOpen.value = '';
    });
    $openFilter.addEventListener('input', () => {
      state.openFilter = $openFilter.value.trim();
      if (state.open) renderExtract();
    });

    dropTarget($drop, (files) => setSource(files));
    dropTarget($zipDrop, (files) => setSource(files));
    dropTarget($openDrop, (files) => openMod(files[0]));
    dropTarget($anDrop, (files) => openMod(files[0]));

    $('mw-clear').addEventListener('click', () => {
      reset();
      for (const id of ['mw-title', 'mw-author', 'mw-version', 'mw-notes']) $(id).value = '';
    });

    $fix.addEventListener('change', () => {
      state.fix = $fix.checked;
      if (!state.fix) state.keep.clear();
      render();
    });

    document.addEventListener('click', (e) => {
      const chip = e.target.closest('.mw-chip');
      if (chip) {
        state.filter = chip.dataset.filter;
        for (const c of document.querySelectorAll('.mw-chip')) c.classList.toggle('active', c === chip);
        render();
        return;
      }
      const get = e.target.closest('[data-get]');
      if (get) { openDownload(get.getAttribute('data-get')); return; }
      const look = e.target.closest('[data-preview]');
      if (look) preview(look.getAttribute('data-preview'));
    });

    $files.addEventListener('change', (e) => {
      const box = e.target.closest('[data-keep]');
      if (!box) return;
      const path = box.getAttribute('data-keep');
      if (box.checked) state.keep.add(path); else state.keep.delete(path);
      render();
    });

    $build.addEventListener('click', build);
    $openZip.addEventListener('click', () => openDownload(''));
    $anRepair.addEventListener('click', repair);

    showTab('make');     // also parks the shared build steps in the first panel
  }

  /** Drag and drop, including a dropped folder - the natural gesture for this
      page, and the only way to get paths out of a drop at all. */
  function dropTarget(el, onFiles) {
    let depth = 0;
    el.addEventListener('dragenter', (e) => {
      e.preventDefault();
      depth += 1;
      el.classList.add('over');
    });
    el.addEventListener('dragover', (e) => { e.preventDefault(); });
    el.addEventListener('dragleave', () => {
      depth = Math.max(0, depth - 1);
      if (!depth) el.classList.remove('over');
    });
    el.addEventListener('drop', async (e) => {
      e.preventDefault();
      depth = 0;
      el.classList.remove('over');
      const items = e.dataTransfer && e.dataTransfer.items;
      const entries = items ? Array.from(items)
        .map((i) => (i.webkitGetAsEntry ? i.webkitGetAsEntry() : null))
        .filter(Boolean) : [];
      if (entries.some((entry) => entry.isDirectory)) {
        const collected = [];
        for (const entry of entries) await walk(entry, '', collected);
        onFiles(collected);
        return;
      }
      onFiles(Array.from((e.dataTransfer && e.dataTransfer.files) || []));
    });
  }

  /** Read a dropped directory tree into Files carrying their relative path -
      the same shape the folder picker produces. */
  async function walk(entry, prefix, out) {
    if (out.length > MAX_FILES) return;
    if (entry.isFile) {
      const file = await new Promise((ok, fail) => entry.file(ok, fail));
      // webkitRelativePath is read-only on a dropped File, so carry the path
      // alongside it; setSource reads this first.
      try {
        Object.defineProperty(file, 'webkitRelativePath',
                              { value: prefix + entry.name, configurable: true });
      } catch (_) { /* older engines: the bare name still works */ }
      out.push(file);
      return;
    }
    if (!entry.isDirectory) return;
    const reader = entry.createReader();
    for (;;) {
      const batch = await new Promise((ok) => reader.readEntries(ok, () => ok([])));
      if (!batch.length) break;
      for (const child of batch) await walk(child, `${prefix + entry.name}/`, out);
    }
  }

  // ─── Util ──────────────────────────────────────────────────────────

  async function readJSON(res) {
    if (!res.ok) throw new Error(await errorText(res));
    return res.json();
  }

  async function errorText(res) {
    try {
      const body = await res.json();
      return (body.error && body.error.message) || body.detail || `HTTP ${res.status}`;
    } catch (_) {
      return `HTTP ${res.status}`;
    }
  }

  function filenameFrom(res, fallback) {
    const header = res.headers.get('Content-Disposition') || '';
    const star = /filename\*=UTF-8''([^;]+)/i.exec(header);
    if (star) { try { return decodeURIComponent(star[1]); } catch (_) { /* fall through */ } }
    const plain = /filename="([^"]+)"/i.exec(header);
    return (plain && plain[1]) || fallback;
  }

  function save(blob, filename) {
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = filename;
    document.body.appendChild(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 30000);
  }

  function setStatus(text, bad) {
    $status.textContent = text || '';
    $status.classList.toggle('bad', !!bad);
  }

  function setOpenStatus(text, bad) {
    $openStatus.textContent = text || '';
    $openStatus.classList.toggle('bad', !!bad);
  }

  function setAnStatus(text, bad) {
    $anStatus.textContent = text || '';
    $anStatus.classList.toggle('bad', !!bad);
  }

  function message(err) {
    return (err && err.message) || String(err);
  }

  function t(s) {
    return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s;
  }

  function formatInt(n) { return Number(n || 0).toLocaleString(); }

  function formatBytes(n) {
    n = Number(n || 0);
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  }
})();

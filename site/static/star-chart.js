/* ═══════════════════════════════════════════════════════════════════════
   Star Chart planner — /star-chart
   ---------------------------------------------------------------------------
   A self-contained, dependency-free (no Vue, no eval) port of the Better Trove
   Tools desktop star-chart builder. The geometry is a deterministic function
   of /static/star_chart.json, so we port the desktop backend's coordinate math
   (star_chart.py) straight to JS and compute client-side — no API needed.

   Build codes use the EXACT same `SC:` base64 format as the desktop app, so a
   code copied here pastes into the desktop app and vice-versa. Builds are also
   shareable by URL (?b=<payload>): the link is kept in sync as you select.
   ═══════════════════════════════════════════════════════════════════════ */
(function () {
  "use strict";

  const toast = window.BTTToast.show;

  const SVGNS = "http://www.w3.org/2000/svg";
  const COLORS = {
    Combat:    { minor: "#FF8F00", major: "#D84315" },
    Gathering: { minor: "#00695C", major: "#558B2F" },
    Pve:       { minor: "#6A1B9A", major: "#283593" },
  };
  const CODE_PREFIX = "SC:";
  const ROOT_TO_ABBREV = { combat: "c", gathering: "g", pve: "p" };
  const ABBREV_TO_ROOT = { c: "combat", g: "gathering", p: "pve" };
  const STORAGE_KEY = "btt_star_chart_build";
  const CHEAT_KEY = "btt_star_chart_cheat";
  const PLAIN_KEY = "btt_star_chart_plain";

  // Presentation transform (matches the desktop app's chart offset/scale).
  const CHART_BASE = [500, 500];
  const CHART_Y_OFFSET = -130;
  const CHART_SPACING_SCALE = 0.92;

  // Pan/zoom viewBox.
  const DEFAULT_VB = { x: 0, y: 0, w: 1000, h: 800 };
  const VB_ASPECT = DEFAULT_VB.w / DEFAULT_VB.h;
  const MIN_VW = 250;
  const MAX_VW = 1400;

  // i18n: reuse the site catalog for dynamic strings; fall back to English.
  const t = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
  const fmt = (s, vars) => {
    let out = t(s);
    for (const k in vars) out = out.split("{" + k + "}").join(vars[k]);
    return out;
  };

  // ── State ───────────────────────────────────────────────────────────────
  const nodeMap = {};          // path -> raw node (with parentPath/constellName)
  const nodeEls = [];          // { path, el, data } render records
  const lineEls = [];          // { pathId, el }
  const selected = new Set();  // selected paths
  let originPt = [500, 370];   // display-space centre
  let cheatMode = localStorage.getItem(CHEAT_KEY) === "1";
  let plainMode = localStorage.getItem(PLAIN_KEY) === "1";
  let codecMaps = null;
  let suppressCodeInput = false;
  let statFilter = "";
  let searchQuery = "";
  const sectionOpen = { stats: true, abilities: true, obtainables: true };
  const viewBox = { ...DEFAULT_VB };

  // ── DOM refs (resolved on init) ──────────────────────────────────────────
  let svg, gLines, gReplace, gNodes, gSky, gStars, anchorEl, tooltipEl;
  let elCode, elSummary, elMetaNodes, elMetaState, elCheat, elSearch, elStat, elStatMeta, elSearchMeta;

  // ── Geometry (port of backend/gems_and_builds/star_chart.py) ──────────────
  const rad = (deg) => (deg * Math.PI) / 180;
  function rotate(origin, point, angle) {
    const [ox, oy] = origin, [px, py] = point;
    return [
      ox + Math.cos(angle) * (px - ox) - Math.sin(angle) * (py - oy),
      oy + Math.sin(angle) * (px - ox) + Math.cos(angle) * (py - oy),
    ];
  }
  function buildBranch(backRotate, lastPos, distance, stars) {
    const totalAngle = 193;
    const division = totalAngle / (stars.length + 1);
    stars.forEach((child, idx) => {
      const finalRotation = division * (idx + 1) + backRotate;
      const childPos = [lastPos[0] - distance, lastPos[1]];
      const rotated = rotate(lastPos, childPos, rad(finalRotation));
      child.Coords = rotated;
      if (child.Stars && child.Stars.length) {
        buildBranch(-(totalAngle / 2 - finalRotation), rotated, distance, child.Stars);
      }
    });
  }
  function rotateBranch(star, origin, angle) {
    if (!star.Stars) return;
    star.Stars.forEach((child) => {
      child.Coords = rotate(origin, child.Coords || [0, 0], angle);
      rotateBranch(child, origin, angle);
    });
  }
  function computeCoords(chart) {
    const origin = [500, 500];
    const pointDistance = 60;
    const names = ["Combat", "Gathering", "Pve"];
    const backs = [0, -2, -4];
    names.forEach((name, i) => {
      const constell = chart[name];
      if (!constell) return;
      const branchRotation = (360 / names.length) * i;
      const position = [origin[0], origin[1] - pointDistance];
      constell.Coords = rotate(origin, position, rad(branchRotation));
      buildBranch(backs[i], position, 47, constell.Stars || []);
      rotateBranch(constell, origin, rad(branchRotation));
    });
    return origin;
  }
  function withOffset(coords) {
    if (!Array.isArray(coords) || coords.length < 2) return [500, 500 + CHART_Y_OFFSET];
    const dx = coords[0] - CHART_BASE[0];
    const dy = coords[1] - CHART_BASE[1];
    return [
      CHART_BASE[0] + dx * CHART_SPACING_SCALE,
      CHART_BASE[1] + dy * CHART_SPACING_SCALE + CHART_Y_OFFSET,
    ];
  }

  // ── Colour + shape helpers ────────────────────────────────────────────────
  function parseHex(hex) {
    const v = String(hex || "").replace("#", "");
    const n = v.length === 3 ? v.split("").map((c) => c + c).join("") : v.padEnd(6, "0").slice(0, 6);
    return [parseInt(n.slice(0, 2), 16), parseInt(n.slice(2, 4), 16), parseInt(n.slice(4, 6), 16)];
  }
  function lighten(hex, w) { const [r, g, b] = parseHex(hex); const bl = (s) => Math.round(s * (1 - w) + 255 * w); return `rgb(${bl(r)}, ${bl(g)}, ${bl(b)})`; }
  function toRgba(hex, a) { const [r, g, b] = parseHex(hex); return `rgba(${r}, ${g}, ${b}, ${a})`; }
  // Points string for an N-pointed star centred at (cx,cy) — the "sparkle" shape.
  function starPts(cx, cy, spikes, outer, inner) {
    const pts = []; const step = Math.PI / spikes; let rot = -Math.PI / 2;
    for (let i = 0; i < spikes * 2; i++) {
      const r = i % 2 ? inner : outer;
      pts.push((cx + Math.cos(rot) * r).toFixed(2) + "," + (cy + Math.sin(rot) * r).toFixed(2));
      rot += step;
    }
    return pts.join(" ");
  }
  const svgEl = (tag, attrs) => { const e = document.createElementNS(SVGNS, tag); if (attrs) for (const k in attrs) e.setAttribute(k, attrs[k]); return e; };

  // ── Build the node/line records from the tree ─────────────────────────────
  function registerNode(star, constellName, parentPath) {
    star.parentPath = parentPath;
    star.constellName = constellName;
    nodeMap[star.Path] = star;
    const isRoot = star.Type === "Root";
    const isMajor = star.Type === "Major";

    if (star.Coords) {
      const [cx, cy] = withOffset(star.Coords);
      const pal = COLORS[constellName] || { minor: "#fff", major: "#fff" };
      // Simple view reproduces the original chart: diamond roots, circle major/
      // minor nodes with flat solid fills. Star view uses glowing sparkle stars
      // with radial-gradient (hot-core) fills.
      const hue = isRoot || isMajor ? pal.major : pal.minor;
      const cls = ["sc-node"];
      let el, r;
      if (isRoot) {
        r = plainMode ? 14 : 16; cls.push("root");
        if (plainMode) {
          // Original look: a hollow dark diamond with a coloured outline.
          el = svgEl("polygon", { points: `${cx},${cy - r} ${cx + r},${cy} ${cx},${cy + r} ${cx - r},${cy}`, fill: "var(--bg-deep)", stroke: hue, "stroke-width": 3.5 });
        } else {
          el = svgEl("polygon", { points: starPts(cx, cy, 4, r, 5.5), fill: `url(#scg-${constellName}-major)` });
          cls.push("star");
        }
      } else if (isMajor) {
        cls.push("major");
        if (plainMode) { r = 13; el = svgEl("circle", { cx, cy, r, fill: hue }); }
        else { r = 12; el = svgEl("polygon", { points: starPts(cx, cy, 4, r, 3.9), fill: `url(#scg-${constellName}-major)` }); cls.push("star"); }
      } else {
        r = plainMode ? 8 : 6.5;
        el = svgEl("circle", { cx, cy, r, fill: plainMode ? hue : `url(#scg-${constellName}-minor)` });
      }
      el.classList.add(...cls);
      el.style.setProperty("--glow", hue);
      const data = { path: star.Path, node: star, cx, cy, r, isRoot };
      const rec = { path: star.Path, el, data };
      nodeEls.push(rec);
      gNodes.appendChild(el);
      wireNode(rec);
    }

    if (star.Stars && star.Stars.length) {
      star.Stars.forEach((child) => {
        if (star.Coords && child.Coords) {
          const [x1, y1] = withOffset(star.Coords);
          const [x2, y2] = withOffset(child.Coords);
          const line = document.createElementNS(SVGNS, "line");
          line.setAttribute("x1", x1); line.setAttribute("y1", y1);
          line.setAttribute("x2", x2); line.setAttribute("y2", y2);
          line.classList.add("sc-line");
          gLines.appendChild(line);
          lineEls.push({ pathId: child.Path, el: line });
        }
        registerNode(child, constellName, star.Path);
      });
    }
  }

  // ── Selection helpers ─────────────────────────────────────────────────────
  const maxNodes = () => (cheatMode ? 120 : 40);

  function ancestorsToSelect(path, acc) {
    acc = acc || [];
    if (!path || selected.has(path)) return acc;
    acc.push(path);
    const node = nodeMap[path];
    if (node && node.parentPath && nodeMap[node.parentPath] && nodeMap[node.parentPath].Type !== "Root") {
      return ancestorsToSelect(node.parentPath, acc);
    }
    return acc;
  }
  function deselectWithChildren(path) {
    if (!path) return;
    selected.delete(path);
    for (const p in nodeMap) if (nodeMap[p].parentPath === path) deselectWithChildren(p);
  }
  function selectConstellation(rootPath) {
    const root = nodeMap[rootPath];
    if (!root) return;
    let hit = false;
    for (const p in nodeMap) {
      const n = nodeMap[p];
      if (n.constellName === root.constellName && n.Type !== "Root" && !selected.has(p)) {
        if (selected.size >= maxNodes()) { hit = true; break; }
        selected.add(p);
      }
    }
    if (hit) toast(fmt("Can't exceed the maximum of {limit} active nodes.", { limit: maxNodes() }), true);
  }
  function selectAll() {
    let hit = false;
    for (const p in nodeMap) {
      const n = nodeMap[p];
      if (n.Type !== "Root" && !selected.has(p)) {
        if (selected.size >= maxNodes()) { hit = true; break; }
        selected.add(p);
      }
    }
    if (hit) toast(fmt("Can't exceed the maximum of {limit} active nodes.", { limit: maxNodes() }), true);
  }
  function onNodeClick(rec) {
    const path = rec.path;
    if (selected.has(path)) {
      deselectWithChildren(path);
    } else {
      const add = ancestorsToSelect(path, []);
      if (selected.size + add.length > maxNodes()) {
        toast(fmt("Can't exceed the maximum of {limit} active nodes.", { limit: maxNodes() }), true);
        return;
      }
      add.forEach((p) => selected.add(p));
    }
    onSelectionChanged();
  }

  // ── Overwrite / replacement analysis ──────────────────────────────────────
  function computeOverwrites() {
    const ow = new Set();
    selected.forEach((p) => {
      const n = nodeMap[p];
      if (n && n.Overwrites) n.Overwrites.forEach((o) => ow.add(o));
    });
    return ow;
  }
  function computeReplacement() {
    const sel = new Set(selected);
    const overwritten = new Set();
    const edges = [];
    sel.forEach((path) => {
      const node = nodeMap[path];
      const direct = (node && node.Overwrites || []).filter((o) => sel.has(o));
      if (!direct.length) return;
      direct.forEach((o) => overwritten.add(o));
      const parents = direct.filter((cand) => !direct.some((other) => {
        if (other === cand) return false;
        const on = nodeMap[other];
        return on && Array.isArray(on.Overwrites) && on.Overwrites.includes(cand);
      }));
      parents.forEach((from) => edges.push({ from, to: path }));
    });
    const tips = new Set();
    sel.forEach((path) => {
      const node = nodeMap[path];
      const owSel = (node && node.Overwrites || []).some((o) => sel.has(o));
      if (owSel && !overwritten.has(path)) tips.add(path);
    });
    return { edges, tips };
  }

  // ── Rendering / visual update ─────────────────────────────────────────────
  const nodeCenter = (path) => {
    const n = nodeMap[path];
    if (!n || !n.Coords) return null;
    const [x, y] = withOffset(n.Coords);
    return { x, y };
  };
  function highlightedPaths() {
    const name = String(statFilter || "").trim();
    if (!name) return new Set();
    const set = new Set();
    nodeEls.forEach(({ data }) => {
      if (data.isRoot || !Array.isArray(data.node.Stats)) return;
      if (data.node.Stats.some((s) => String(s && s.name || "").trim() === name)) set.add(data.path);
    });
    return set;
  }
  function searchPaths() {
    const q = String(searchQuery || "").trim().toLowerCase();
    if (q.length < 2) return new Set();
    const set = new Set();
    nodeEls.forEach(({ data }) => {
      if (data.isRoot) return;
      const n = data.node;
      const hay = [];
      if (n.Name) hay.push(t(n.Name), n.Name);
      if (n.Description) hay.push(t(n.Description), n.Description);
      (n.Abilities || []).forEach((a) => hay.push(t(a), a));
      (n.Obtainables || []).forEach((o) => hay.push(t(o), o));
      (n.Stats || []).forEach((s) => { if (s && s.name) hay.push(t(s.name), s.name); });
      if (hay.some((x) => String(x || "").toLowerCase().includes(q))) set.add(data.path);
    });
    return set;
  }
  function updateVisuals() {
    const ow = computeOverwrites();
    const { edges, tips } = computeReplacement();
    const statPaths = highlightedPaths();
    const foundPaths = searchPaths();
    const filterActive = Boolean(statFilter) || String(searchQuery || "").trim().length >= 2;

    nodeEls.forEach(({ el, data }) => {
      const path = data.path;
      const isSel = selected.has(path);
      const isOw = ow.has(path);
      const isTip = tips.has(path);
      const isHl = statPaths.has(path) || foundPaths.has(path);
      el.classList.toggle("selected", isSel && !data.isRoot);
      el.classList.toggle("overwritten", isOw);
      el.classList.toggle("muted", !isSel && !isOw && !isHl && !data.isRoot);
      el.classList.toggle("replacement-tip", isTip);
      el.classList.toggle("hl", isHl && !data.isRoot);
      el.classList.toggle("on", isHl && isSel);
      el.classList.toggle("off", isHl && !isSel);
      // While a filter/search is active, push the non-matches far into the background.
      el.classList.toggle("dim", filterActive && !isSel && !isHl && !isTip && !data.isRoot);
      if (data.isRoot) {
        let rootActive = false;
        for (const p of selected) { if (nodeMap[p] && nodeMap[p].constellName === data.node.constellName) { rootActive = true; break; } }
        el.classList.toggle("root-active", rootActive);
      }
    });

    lineEls.forEach(({ pathId, el }) => el.classList.toggle("selected", selected.has(pathId)));
    anchorEl.classList.toggle("active", selected.size > 0);
    renderReplacementCurves(edges, tips);
  }
  function renderReplacementCurves(edges, tips) {
    gReplace.textContent = "";
    edges.forEach((edge) => {
      const from = nodeCenter(edge.from), to = nodeCenter(edge.to);
      if (!from || !to) return;
      const dx = to.x - from.x, dy = to.y - from.y;
      const dist = Math.hypot(dx, dy) || 1;
      const nX = -dy / dist, nY = dx / dist;
      const bend = Math.max(30, Math.min(72, dist * 0.2));
      const mid = { x: (from.x + to.x) / 2, y: (from.y + to.y) / 2 };
      let best = null;
      [-1, 1].forEach((sign) => {
        const ctrl = { x: mid.x + nX * bend * sign, y: mid.y + nY * bend * sign };
        let score = 0;
        for (let step = 1; step < 12; step++) {
          const tt = step / 12, mt = 1 - tt;
          const px = mt * mt * from.x + 2 * mt * tt * ctrl.x + tt * tt * to.x;
          const py = mt * mt * from.y + 2 * mt * tt * ctrl.y + tt * tt * to.y;
          nodeEls.forEach(({ data }) => {
            if (data.path === edge.from || data.path === edge.to) return;
            const rr = (data.r || 8) + 8;
            const nd = Math.hypot(px - data.cx, py - data.cy);
            if (nd < rr) score += (rr - nd) * 10;
          });
        }
        score -= Math.hypot(ctrl.x - originPt[0], ctrl.y - originPt[1]) * 0.02;
        if (!best || score < best.score) best = { ctrl, score };
      });
      const p = document.createElementNS(SVGNS, "path");
      p.setAttribute("d", `M ${from.x} ${from.y} Q ${best.ctrl.x} ${best.ctrl.y} ${to.x} ${to.y}`);
      p.classList.add("sc-replacement");
      if (tips.has(edge.to)) p.classList.add("tip");
      gReplace.appendChild(p);
    });
  }

  // ── Summary panel ─────────────────────────────────────────────────────────
  function activePaths() {
    const ow = computeOverwrites();
    return Array.from(selected).filter((p) => !ow.has(p));
  }
  function renderSummary() {
    const active = activePaths();
    elMetaNodes.textContent = fmt("Nodes: {n}/{max}", { n: selected.size, max: maxNodes() });
    elMetaState.classList.toggle("active", selected.size > 0);
    elMetaState.innerHTML = selected.size > 0
      ? `<i class="fa-solid fa-circle-check"></i> ${t("Active")}`
      : `<i class="fa-regular fa-circle"></i> ${t("Empty")}`;
    elCheat.classList.toggle("active", cheatMode);
    elCheat.innerHTML = `<i class="fa-solid ${cheatMode ? "fa-bolt" : "fa-lock"}"></i> ${t("Cheat mode")}: ${cheatMode ? t("On") : t("Off")}`;

    if (!selected.size) { elSummary.innerHTML = `<p class="sc-empty">${t("Select nodes to see the combined stats, abilities and rewards.")}</p>`; return; }

    const statsObj = {}, abilities = [], obtain = {};
    active.forEach((p) => {
      const n = nodeMap[p];
      if (!n) return;
      (n.Stats || []).forEach((s) => {
        const k = s.name + (s.percentage ? "_pct" : "_flat");
        if (!statsObj[k]) statsObj[k] = { name: s.name, percentage: s.percentage, value: 0 };
        statsObj[k].value += s.value;
      });
      (n.Abilities || []).forEach((a) => abilities.push(a));
      (n.Obtainables || []).forEach((o) => { obtain[o] = (obtain[o] || 0) + 1; });
    });
    const stats = Object.values(statsObj);
    const obtainables = Object.entries(obtain).map(([name, count]) => ({ name, count }));

    const esc = (s) => String(s).replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));
    const section = (key, title, itemsHtml) => itemsHtml ? `
      <div class="sc-section ${sectionOpen[key] ? "" : "collapsed"}" data-section="${key}">
        <button type="button" class="sc-section-toggle">
          <span>${t(title)}</span>
          <i class="fa-solid ${sectionOpen[key] ? "fa-chevron-up" : "fa-chevron-down"}"></i>
        </button>
        <ul>${itemsHtml}</ul>
      </div>` : "";

    let html = `<div style="margin-bottom:12px;color:var(--text-mute);font-size:12px;">${fmt("{count} of {limit} nodes active", { count: selected.size, limit: maxNodes() })}</div>`;
    html += section("stats", "Aggregated stats",
      stats.map((s) => `<li><strong>${esc(t(s.name))}:</strong> +${s.value}${s.percentage ? "%" : ""}</li>`).join(""));
    html += section("abilities", "Active abilities",
      abilities.map((a) => `<li>${esc(t(a))}</li>`).join(""));
    html += section("obtainables", "Obtainables",
      obtainables.map((o) => `<li>${o.count > 1 ? o.count + "x " : ""}${esc(t(o.name))}</li>`).join(""));
    elSummary.innerHTML = html;

    elSummary.querySelectorAll(".sc-section-toggle").forEach((btn) => {
      btn.addEventListener("click", () => {
        const key = btn.parentElement.getAttribute("data-section");
        sectionOpen[key] = !sectionOpen[key];
        renderSummary();
      });
    });
  }

  // ── Build code (byte-identical to the desktop app) ────────────────────────
  function getCodecMaps() {
    if (codecMaps) return codecMaps;
    const paths = Object.keys(nodeMap).filter((p) => nodeMap[p] && nodeMap[p].Type !== "Root").sort();
    const pathToId = new Map();
    paths.forEach((p, i) => pathToId.set(p, i));
    const maps = { paths, pathToId };
    if (paths.length) codecMaps = maps;
    return maps;
  }
  const toB64Url = (bin) => btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
  const fromB64Url = (payload) => {
    const norm = String(payload || "").replace(/-/g, "+").replace(/_/g, "/");
    const pad = (4 - (norm.length % 4)) % 4;
    return atob(norm + "=".repeat(pad));
  };
  function terminalPaths(sel) {
    const arr = Array.from(sel).filter((p) => nodeMap[p] && nodeMap[p].Type !== "Root");
    return arr.filter((p) => !arr.some((o) => o !== p && o.startsWith(p + "."))).sort();
  }
  function encodePayload() {
    const { pathToId } = getCodecMaps();
    const ids = terminalPaths(selected).map((p) => pathToId.get(p))
      .filter((id) => Number.isInteger(id)).sort((a, b) => a - b);
    if (!ids.length) return "";
    return toB64Url(String.fromCharCode(...ids));
  }
  function encodeCode() {
    const payload = encodePayload();
    return payload ? CODE_PREFIX + payload : "";
  }
  function expandSelection(path, into) {
    let cur = path;
    while (cur && nodeMap[cur] && nodeMap[cur].Type !== "Root") {
      if (into.has(cur)) break;
      into.add(cur);
      const parent = nodeMap[cur].parentPath;
      if (!parent || !nodeMap[parent] || nodeMap[parent].Type === "Root") break;
      cur = parent;
    }
    return into;
  }
  function decodeCompactPath(token) {
    const tk = String(token || "").trim().toLowerCase();
    if (!tk) return null;
    const root = ABBREV_TO_ROOT[tk[0]];
    if (!root) return null;
    const segs = tk.slice(1).match(/[a-z]+|\d+/g) || [];
    const full = [root, ...segs].join(".");
    return nodeMap[full] && nodeMap[full].Type !== "Root" ? full : null;
  }
  function decodeToPaths(code) {
    const trimmed = String(code || "").trim();
    const out = new Set();
    if (!trimmed) return out;
    if (trimmed.startsWith(CODE_PREFIX) || trimmed.startsWith("v2:")) {
      const payload = trimmed.slice(trimmed.indexOf(":") + 1);
      if (payload.includes("|")) {
        payload.split("|").forEach((tok) => { const p = decodeCompactPath(tok); if (p) expandSelection(p, out); });
        return out;
      }
      try {
        const { paths } = getCodecMaps();
        const bin = fromB64Url(payload);
        Array.from(bin).forEach((ch) => { const p = paths[ch.charCodeAt(0)]; if (p) expandSelection(p, out); });
      } catch (e) { return new Set(); }
      return out;
    }
    try {
      atob(trimmed).split("$").forEach((p) => {
        const np = String(p || "").trim();
        if (nodeMap[np] && nodeMap[np].Type !== "Root") out.add(np);
      });
    } catch (e) { return new Set(); }
    return out;
  }
  function applyCode(code, silent) {
    const paths = Array.from(decodeToPaths(code));
    if (!paths.length) { if (!silent) toast(t("No valid nodes found in that build code."), true); return false; }
    selected.clear();
    let loaded = 0, skipped = 0;
    paths.sort((a, b) => a.split(".").length - b.split(".").length);
    paths.forEach((p) => {
      if (nodeMap[p] && nodeMap[p].Type !== "Root") {
        if (selected.size < maxNodes() && !selected.has(p)) { selected.add(p); loaded++; }
        else skipped++;
      }
    });
    if (!silent && skipped > 0) toast(fmt("Loaded {loaded}, skipped {skipped} (max {limit}).", { loaded, skipped, limit: maxNodes() }), true);
    return true;
  }

  // ── URL + persistence ─────────────────────────────────────────────────────
  function syncUrlAndStorage() {
    const payload = encodePayload();
    const url = new URL(window.location.href);
    if (payload) url.searchParams.set("b", payload);
    else url.searchParams.delete("b");
    window.history.replaceState(null, "", url);
    try { localStorage.setItem(STORAGE_KEY, payload); } catch (e) {}
  }
  function onSelectionChanged() {
    if (!suppressCodeInput) elCode.value = encodeCode();
    updateVisuals();
    renderSummary();
    syncUrlAndStorage();
  }

  // ── Interaction wiring ────────────────────────────────────────────────────
  function wireNode(rec) {
    const { el, data } = rec;
    if (data.isRoot) {
      let clickTimer = null;
      el.addEventListener("click", (e) => {
        if (e.detail === 1) {
          clickTimer = setTimeout(() => { deselectWithChildren(data.path); onSelectionChanged(); }, 220);
        } else if (e.detail === 2) {
          clearTimeout(clickTimer);
          selectConstellation(data.path);
          onSelectionChanged();
        }
      });
    } else {
      el.addEventListener("click", () => onNodeClick(rec));
    }
    el.addEventListener("mouseenter", (e) => showTooltip(e, data.node));
    el.addEventListener("mousemove", moveTooltip);
    el.addEventListener("mouseleave", hideTooltip);
  }

  function showTooltip(e, node) {
    if (!node) return;
    const stats = (node.Stats || []).map((s) => `<li><strong>${t(s.name)}:</strong> +${s.value}${s.percentage ? "%" : ""}</li>`).join("");
    const abil = (node.Abilities || []).map((a) => `<li>${t(a)}</li>`).join("");
    let html = `<h3>${t(node.Name || node.Constellation || "")}</h3>`;
    html += `<span class="type">${fmt("Type: {type}", { type: t(node.Type || "") })}</span>`;
    if (node.Description) html += `<p>${t(node.Description)}</p><hr/>`;
    if (stats) html += `<ul>${stats}</ul>`;
    if (abil) html += `${stats ? "<hr/>" : ""}<ul>${abil}</ul>`;
    tooltipEl.innerHTML = html;
    tooltipEl.classList.add("show");
    moveTooltip(e);
  }
  function moveTooltip(e) {
    if (!tooltipEl.classList.contains("show")) return;
    let x = e.clientX + 15, y = e.clientY + 15;
    if (x + tooltipEl.offsetWidth > window.innerWidth) x = e.clientX - tooltipEl.offsetWidth - 15;
    if (y + tooltipEl.offsetHeight > window.innerHeight) y = e.clientY - tooltipEl.offsetHeight - 15;
    tooltipEl.style.left = x + "px";
    tooltipEl.style.top = y + "px";
  }
  function hideTooltip() { tooltipEl.classList.remove("show"); }

  // Pan / zoom
  function applyViewBox() { svg.setAttribute("viewBox", `${viewBox.x} ${viewBox.y} ${viewBox.w} ${viewBox.h}`); }
  function svgPoint(e) {
    const rect = svg.getBoundingClientRect();
    if (!rect.width || !rect.height) return null;
    const relX = (e.clientX - rect.left) / rect.width;
    const relY = (e.clientY - rect.top) / rect.height;
    return { x: viewBox.x + relX * viewBox.w, y: viewBox.y + relY * viewBox.h, relX, relY };
  }
  function onWheel(e) {
    e.preventDefault();
    const pt = svgPoint(e);
    if (!pt) return;
    const factor = e.deltaY < 0 ? 0.85 : 1 / 0.85;
    const w = Math.min(MAX_VW, Math.max(MIN_VW, viewBox.w * factor));
    viewBox.x = pt.x - pt.relX * w;
    viewBox.y = pt.y - pt.relY * (w / VB_ASPECT);
    viewBox.w = w; viewBox.h = w / VB_ASPECT;
    applyViewBox();
  }
  let pan = null;
  function onPanMove(e) {
    if (!pan) return;
    viewBox.x = pan.vbX - (e.clientX - pan.cx) * pan.upx;
    viewBox.y = pan.vbY - (e.clientY - pan.cy) * pan.upy;
    applyViewBox();
  }
  function onPanUp() {
    window.removeEventListener("mousemove", onPanMove);
    window.removeEventListener("mouseup", onPanUp);
    svg.classList.remove("panning");
    pan = null;
  }
  function onMouseDown(e) {
    const tgt = e.target;
    if (e.button !== 0 || (tgt && tgt.classList && (tgt.classList.contains("sc-node") || tgt.classList.contains("sc-anchor")))) return;
    const rect = svg.getBoundingClientRect();
    if (!rect.width || !rect.height) return;
    e.preventDefault();
    pan = { cx: e.clientX, cy: e.clientY, vbX: viewBox.x, vbY: viewBox.y, upx: viewBox.w / rect.width, upy: viewBox.h / rect.height };
    svg.classList.add("panning");
    window.addEventListener("mousemove", onPanMove, { passive: true });
    window.addEventListener("mouseup", onPanUp, { passive: true });
  }
  // Touch
  let touch = null;
  const touchDist = (a, b) => Math.hypot(b.clientX - a.clientX, b.clientY - a.clientY);
  function onTouchStart(e) {
    const rect = svg.getBoundingClientRect();
    if (!rect.width || !rect.height) return;
    if (e.touches.length === 1) {
      const tgt = e.target;
      if (tgt && tgt.classList && (tgt.classList.contains("sc-node") || tgt.classList.contains("sc-anchor"))) { touch = null; return; }
      const c = e.touches[0];
      touch = { mode: "pan", cx: c.clientX, cy: c.clientY, vbX: viewBox.x, vbY: viewBox.y, upx: viewBox.w / rect.width, upy: viewBox.h / rect.height };
      svg.classList.add("panning");
    } else if (e.touches.length >= 2) {
      const a = e.touches[0], b = e.touches[1];
      const relX = ((a.clientX + b.clientX) / 2 - rect.left) / rect.width;
      const relY = ((a.clientY + b.clientY) / 2 - rect.top) / rect.height;
      touch = { mode: "pinch", d0: touchDist(a, b) || 1, w0: viewBox.w, ax: viewBox.x + relX * viewBox.w, ay: viewBox.y + relY * viewBox.h, relX, relY };
      svg.classList.add("panning");
    }
  }
  function onTouchMove(e) {
    if (!touch) return;
    if (touch.mode === "pan" && e.touches.length === 1) {
      e.preventDefault();
      const c = e.touches[0];
      viewBox.x = touch.vbX - (c.clientX - touch.cx) * touch.upx;
      viewBox.y = touch.vbY - (c.clientY - touch.cy) * touch.upy;
      applyViewBox();
    } else if (touch.mode === "pinch" && e.touches.length >= 2) {
      e.preventDefault();
      const d = touchDist(e.touches[0], e.touches[1]) || 1;
      const w = Math.min(MAX_VW, Math.max(MIN_VW, touch.w0 * (touch.d0 / d)));
      viewBox.x = touch.ax - touch.relX * w;
      viewBox.y = touch.ay - touch.relY * (w / VB_ASPECT);
      viewBox.w = w; viewBox.h = w / VB_ASPECT;
      applyViewBox();
    }
  }
  function onTouchEnd(e) {
    if (e.touches.length === 0) { svg.classList.remove("panning"); touch = null; return; }
    if (e.touches.length === 1) {
      const rect = svg.getBoundingClientRect();
      const c = e.touches[0];
      touch = { mode: "pan", cx: c.clientX, cy: c.clientY, vbX: viewBox.x, vbY: viewBox.y, upx: viewBox.w / rect.width, upy: viewBox.h / rect.height };
    }
  }
  function zoomBy(factor) {
    const cx = viewBox.x + viewBox.w / 2, cy = viewBox.y + viewBox.h / 2;
    const w = Math.min(MAX_VW, Math.max(MIN_VW, viewBox.w * factor));
    viewBox.w = w; viewBox.h = w / VB_ASPECT;
    viewBox.x = cx - w / 2; viewBox.y = cy - viewBox.h / 2;
    applyViewBox();
  }
  function resetView() { Object.assign(viewBox, DEFAULT_VB); applyViewBox(); }

  // Clipboard
  function copy(text, okMsg) {
    if (!text) { toast(t("Nothing selected to copy."), true); return; }
    navigator.clipboard.writeText(text).then(() => toast(okMsg)).catch(() => toast(t("Copy failed."), true));
  }

  // Stat-filter dropdown options
  function fillStatFilter() {
    const labels = new Set();
    nodeEls.forEach(({ data }) => (data.node.Stats || []).forEach((s) => { const n = String(s && s.name || "").trim(); if (n) labels.add(n); }));
    const opts = [`<option value="">${t("Highlight a stat…")}</option>`];
    Array.from(labels).sort((a, b) => t(a).localeCompare(t(b))).forEach((n) => { opts.push(`<option value="${n.replace(/"/g, "&quot;")}">${t(n)}</option>`); });
    elStat.innerHTML = opts.join("");
  }

  // ── Night-sky scenery: gradients, twinkling starfield, nebula clouds ───────
  function buildDefs() {
    const defs = svgEl("defs");
    const grad = (id, core, mid, edge) => {
      const g = svgEl("radialGradient", { id, cx: "50%", cy: "50%", r: "50%" });
      [["0%", core], ["38%", mid], ["100%", edge]].forEach(([o, c]) => g.appendChild(svgEl("stop", { offset: o, "stop-color": c })));
      defs.appendChild(g);
    };
    Object.keys(COLORS).forEach((name) => {
      const c = COLORS[name];
      grad(`scg-${name}-minor`, "#ffffff", lighten(c.minor, 0.5), c.minor);
      grad(`scg-${name}-major`, "#ffffff", lighten(c.major, 0.55), c.major);
      // Nebula: constellation colour at the core, fading to nothing at the edge.
      const neb = svgEl("radialGradient", { id: `scn-${name}`, cx: "50%", cy: "50%", r: "50%" });
      [["0%", toRgba(c.minor, 0.4)], ["45%", toRgba(c.major, 0.16)], ["100%", toRgba(c.major, 0)]]
        .forEach(([o, col]) => neb.appendChild(svgEl("stop", { offset: o, "stop-color": col })));
      defs.appendChild(neb);
    });
    svg.insertBefore(defs, svg.firstChild);
  }
  function buildStarfield() {
    const frag = document.createDocumentFragment();
    for (let i = 0; i < 230; i++) {
      const o = 0.15 + Math.random() * 0.7;
      const c = svgEl("circle", {
        cx: (-300 + Math.random() * 1600).toFixed(1),
        cy: (-300 + Math.random() * 1400).toFixed(1),
        r: (0.3 + Math.random() * 1.4).toFixed(2),
      });
      c.setAttribute("class", "sc-bgstar");
      c.style.setProperty("--o", o.toFixed(2));
      c.style.opacity = o.toFixed(2);
      if (Math.random() < 0.55) {
        c.classList.add("tw");
        c.style.animationDelay = (Math.random() * 5).toFixed(2) + "s";
        c.style.animationDuration = (2.4 + Math.random() * 3.6).toFixed(2) + "s";
      }
      frag.appendChild(c);
    }
    gStars.appendChild(frag);
  }
  function buildNebula() {
    const groups = {};
    nodeEls.forEach(({ data }) => {
      const k = data.node.constellName;
      (groups[k] || (groups[k] = { sx: 0, sy: 0, n: 0 }));
      groups[k].sx += data.cx; groups[k].sy += data.cy; groups[k].n++;
    });
    Object.keys(groups).forEach((name) => {
      const g = groups[name];
      gSky.appendChild(svgEl("circle", {
        cx: (g.sx / g.n).toFixed(1), cy: (g.sy / g.n).toFixed(1), r: 250,
        fill: `url(#scn-${name})`, class: "sc-nebula",
      }));
    });
  }

  // ── Init ──────────────────────────────────────────────────────────────────
  async function init() {
    svg = document.getElementById("sc-svg");
    gLines = document.getElementById("sc-g-lines");
    gReplace = document.getElementById("sc-g-replace");
    gNodes = document.getElementById("sc-g-nodes");
    tooltipEl = document.getElementById("sc-tooltip");
    elCode = document.getElementById("sc-code");
    elSummary = document.getElementById("sc-summary");
    elMetaNodes = document.getElementById("sc-meta-nodes");
    elMetaState = document.getElementById("sc-meta-state");
    elCheat = document.getElementById("sc-meta-cheat");
    elSearch = document.getElementById("sc-search");
    elStat = document.getElementById("sc-stat");
    elStatMeta = document.getElementById("sc-stat-meta");
    elSearchMeta = document.getElementById("sc-search-meta");

    let chart;
    try {
      const res = await fetch("/static/star_chart.json", { cache: "force-cache" });
      if (!res.ok) throw new Error("HTTP " + res.status);
      chart = await res.json();
    } catch (e) {
      document.querySelector(".sc-loading").innerHTML = `<i class="fa-solid fa-triangle-exclamation"></i> ${t("Couldn't load the star chart data.")}`;
      return;
    }

    const origin = computeCoords(chart);
    originPt = withOffset(origin);

    // Gradient defs + the sky/starfield groups are created once; buildScene()
    // (re)builds the constellations and (in star view) the starfield + nebula,
    // and is re-run when the view toggles between star view and simple view.
    buildDefs();
    gSky = svgEl("g", { id: "sc-g-sky" }); svg.insertBefore(gSky, gLines);
    gStars = svgEl("g", { id: "sc-g-stars" }); svg.insertBefore(gStars, gLines);

    // Centre anchor (click = clear all; dbl in cheat = select all)
    anchorEl = document.createElementNS(SVGNS, "circle");
    anchorEl.setAttribute("cx", originPt[0]);
    anchorEl.setAttribute("cy", originPt[1]);
    anchorEl.setAttribute("r", 18);
    anchorEl.setAttribute("fill", "var(--bg-card)");
    anchorEl.setAttribute("stroke", "var(--bg-line)");
    anchorEl.setAttribute("stroke-width", 4);
    anchorEl.classList.add("sc-anchor");
    let anchorTimer = null;
    anchorEl.addEventListener("click", (e) => {
      if (e.detail === 1) {
        anchorTimer = setTimeout(() => { if (selected.size) { selected.clear(); onSelectionChanged(); toast(t("Cleared all active nodes.")); } }, 220);
      } else if (e.detail === 2) {
        clearTimeout(anchorTimer);
        if (cheatMode) { selectAll(); onSelectionChanged(); }
      }
    });

    // (Re)build the whole scene for the current view mode. The selection is a
    // set of paths, so it survives a rebuild untouched.
    const buildScene = () => {
      [gSky, gStars, gLines, gReplace, gNodes].forEach((g) => (g.textContent = ""));
      nodeEls.length = 0; lineEls.length = 0;
      if (!plainMode) buildStarfield();
      Object.keys(COLORS).forEach((name) => {
        if (!chart[name]) return;
        registerNode(chart[name], name, null); // shape/fill picked from plainMode
        if (chart[name].Coords) {
          const [rx, ry] = withOffset(chart[name].Coords);
          const line = svgEl("line", { x1: originPt[0], y1: originPt[1], x2: rx, y2: ry });
          line.classList.add("sc-line");
          gLines.appendChild(line);
          lineEls.push({ pathId: chart[name].Path + "_root", el: line });
        }
      });
      gNodes.appendChild(anchorEl);
      if (!plainMode) buildNebula(); // needs node centroids → after constellations
      codecMaps = null;
      updateVisuals();
    };
    buildScene();

    applyViewBox();
    fillStatFilter();

    // Chart interactions
    svg.addEventListener("wheel", onWheel, { passive: false });
    svg.addEventListener("mousedown", onMouseDown);
    svg.addEventListener("touchstart", onTouchStart, { passive: true });
    svg.addEventListener("touchmove", onTouchMove, { passive: false });
    svg.addEventListener("touchend", onTouchEnd);
    svg.addEventListener("touchcancel", onTouchEnd);

    // Toolbar buttons
    document.getElementById("sc-zoom-in").addEventListener("click", () => zoomBy(0.85));
    document.getElementById("sc-zoom-out").addEventListener("click", () => zoomBy(1 / 0.85));
    document.getElementById("sc-reset").addEventListener("click", resetView);

    // Star view ⇄ Simple view: flatten the night-sky treatment back to a plain
    // chart. Pure CSS class on the wrapper, so it toggles instantly and persists.
    const wrap = document.getElementById("sc-chart-wrapper");
    const viewBtn = document.getElementById("sc-view-toggle");
    const applyViewMode = () => {
      wrap.classList.toggle("sc-plain", plainMode);
      const icon = viewBtn.querySelector("i");
      icon.className = plainMode ? "fa-solid fa-wand-magic-sparkles" : "fa-solid fa-circle-half-stroke";
      viewBtn.title = plainMode ? t("Star view") : t("Simple view");
      viewBtn.setAttribute("aria-pressed", String(plainMode));
    };
    viewBtn.addEventListener("click", () => {
      plainMode = !plainMode;
      try { localStorage.setItem(PLAIN_KEY, plainMode ? "1" : "0"); } catch (e) {}
      buildScene();     // re-render nodes/lines in the other style
      applyViewMode();  // flip the wrapper class, button icon + title
      toast(plainMode ? t("Simple view.") : t("Star view."));
    });
    applyViewMode();

    // Share controls
    document.getElementById("sc-copy-code").addEventListener("click", () => copy(encodeCode(), t("Build code copied — paste it in the desktop app.")));
    document.getElementById("sc-copy-link").addEventListener("click", () => {
      const payload = encodePayload();
      const url = new URL(window.location.href);
      if (payload) url.searchParams.set("b", payload); else url.searchParams.delete("b");
      copy(url.toString(), t("Share link copied to clipboard."));
    });
    elCode.addEventListener("input", () => {
      suppressCodeInput = true;
      const ok = applyCode(elCode.value, true);
      if (ok) { updateVisuals(); renderSummary(); syncUrlAndStorage(); }
      suppressCodeInput = false;
    });

    // Cheat toggle
    elCheat.addEventListener("click", () => {
      if (cheatMode && selected.size > 40) { toast(t("Reduce active nodes to 40 or fewer before disabling cheat mode."), true); return; }
      cheatMode = !cheatMode;
      try { localStorage.setItem(CHEAT_KEY, cheatMode ? "1" : "0"); } catch (e) {}
      renderSummary();
      toast(cheatMode ? t("Cheat mode on — node limit raised to 120.") : t("Cheat mode off — node limit back to 40."));
    });

    // Search + stat highlight
    elSearch.addEventListener("input", () => {
      searchQuery = elSearch.value;
      const n = searchPaths().size;
      elSearchMeta.textContent = searchQuery.trim().length >= 2 ? fmt("Matches: {n}", { n }) : "";
      updateVisuals();
    });
    elStat.addEventListener("change", () => {
      statFilter = elStat.value;
      const n = highlightedPaths().size;
      elStatMeta.textContent = statFilter ? fmt("Highlighted nodes: {n}", { n }) : "";
      updateVisuals();
    });

    document.querySelector(".sc-chart-wrapper").classList.add("ready");

    // Load an initial build: URL ?b= wins, else localStorage.
    const params = new URLSearchParams(window.location.search);
    const urlB = params.get("b");
    let initialCode = "";
    if (urlB) initialCode = CODE_PREFIX + urlB;
    else { try { const saved = localStorage.getItem(STORAGE_KEY); if (saved) initialCode = CODE_PREFIX + saved; } catch (e) {} }
    if (initialCode) applyCode(initialCode, true);

    elCode.value = encodeCode();
    updateVisuals();
    renderSummary();

    // Re-translate dynamic strings when the language changes (i18n.js fires
    // `btt-lang-changed` on document after swapping the active locale).
    document.addEventListener("btt-lang-changed", () => { fillStatFilter(); renderSummary(); });
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();

/* Kiwi VFX preview — plays a mod release's PopcornFX .pkfx effect in WebGL2.

   The server (/site/mods/releases/<id>/vfx/*) hands us the .pkfx text plus every
   asset it references, resolving missing textures/meshes from the live game tree.
   We parse, simulate on the CPU, and render billboards + ribbons here.

   Public API (assigned to window.PkfxViewer for classic-script callers):
     PkfxViewer.mount(container, { releaseId, path }) -> { dispose() }
     PkfxViewer.mount(container, { endpoint: {base, query}, path })  // embeddable viewer

   Renders billboard + ribbon particles. Mesh/Light renderers and parent-emitted
   trails are not yet drawn (surfaced as a "partial preview" note).  */
import { parsePkfx } from './parser.js';
import { buildEffect } from './model.js';
import { System } from './sim.js';
import { decodeDDS } from './dds.js';
import { Renderer, makeTexture, FLOATS_PER_INSTANCE, RIBBON_FLOATS_PER_VERT, MESH_FLOATS_PER_INSTANCE } from './renderer.js';

const SITE = (id) => `/site/mods/releases/${encodeURIComponent(id)}/vfx`;

/* Where the .pkfx text + its assets come from. A hub release resolves to the Mods
   Hub proxies; the embeddable viewer (/embed/viewer) passes `endpoint` instead,
   since its source may be an uploaded .tmod or a game file rather than a release.
   Both surfaces speak the same two shapes - `<base>/manifest?path=` and
   `<base>/asset?path=` - so only the base (and its source query) differs. */
function endpointsFor({ releaseId, endpoint }) {
  const base = endpoint ? endpoint.base : SITE(releaseId);
  const extra = endpoint && endpoint.query ? `&${endpoint.query}` : '';
  return {
    manifest: (p) => `${base}/manifest?path=${encodeURIComponent(p)}${extra}`,
    asset: (ref) => `${base}/asset?path=${encodeURIComponent(ref)}${extra}`,
  };
}

export function mount(container, { releaseId, path, endpoint }) {
  const urls = endpointsFor({ releaseId, endpoint });
  const canvas = document.createElement('canvas');
  canvas.className = 'pkfx-canvas';
  container.appendChild(canvas);
  const note = document.createElement('div');
  note.className = 'pkfx-note';
  container.appendChild(note);
  const loading = document.createElement('div');
  loading.className = 'pkfx-loading';
  loading.innerHTML = '<span class="pkfx-spinner"></span> Loading VFX preview…';
  container.appendChild(loading);

  let renderer, system, current, raf = 0, disposed = false, glowTex = null;
  const texCache = new Map(), atlasCache = new Map();

  const assetUrl = (ref) => urls.asset(ref);

  async function loadTexture(ref) {
    if (!ref) return renderer.white;
    if (texCache.has(ref)) return texCache.get(ref);
    const p = (async () => {
      try {
        const res = await fetch(assetUrl(ref));
        if (!res.ok) throw new Error(res.status);
        if (/\.dds$/i.test(ref)) {
          const { width, height, rgba } = decodeDDS(await res.arrayBuffer());
          return makeTexture(renderer.gl, width, height, rgba);
        }
        const bmp = await createImageBitmap(await res.blob());
        const cv = new OffscreenCanvas(bmp.width, bmp.height), cx = cv.getContext('2d');
        cx.drawImage(bmp, 0, 0);
        return makeTexture(renderer.gl, bmp.width, bmp.height, cx.getImageData(0, 0, bmp.width, bmp.height).data);
      } catch (e) { return renderer.white; }
    })();
    texCache.set(ref, p);
    const tex = await p; texCache.set(ref, tex); return tex;
  }

  async function loadAtlas(ref) {
    if (!ref) return null;
    if (atlasCache.has(ref)) return atlasCache.get(ref);
    try {
      const txt = await (await fetch(assetUrl(ref))).text();
      const rects = txt.trim().split(/\r?\n/).map((l) => l.split(',').map((n) => parseFloat(n.trim())));
      atlasCache.set(ref, rects); return rects;
    } catch { atlasCache.set(ref, null); return null; }
  }

  async function load() {
    renderer = new Renderer(canvas);
    glowTex = makeGlowTexture(renderer.gl);
    const man = await (await fetch(urls.manifest(path))).json();
    if (disposed) return;
    const doc = parsePkfx(man.pkfx);
    const effect = buildEffect(doc, Math.random);

    const unsupported = new Set();
    for (const layer of effect.layers) {
      for (const r of layer.renderers) {
        if (r.kind === 'billboard' || r.kind === 'ribbon') {
          r._tex = await loadTexture(r.diffuse);
          r._atlas = await loadAtlas(r.atlas);
          r._blend = /Additive/.test(r.material) ? 'add' : 'alpha';
        } else if (r.kind === 'light') {
          r._tex = glowTex; r._blend = 'add';
        } else if (r.kind === 'mesh') {
          // box proxy, no texture
        } else if (r.cls) unsupported.add(r.cls.replace('CParticleRenderer_', ''));
      }
    }
    if (disposed) return;
    system = new System(effect, Math.random);
    // Effects meant to be dragged through the world (trails/wakes/streaks) need the emitter
    // to move or they just clump at the origin. Ribbon renderers always imply this; for
    // billboard trails the name is the reliable signal (the .pkfx is named *_trail_*, *_wake_*).
    const hasRibbon = effect.layers.some((l) => l.renderers.some((r) => r.kind === 'ribbon'));
    const trail = hasRibbon || /trail|wake|streak|trailing/i.test(path);
    current = { effect, unsupported: [...unsupported], missing: man.missing || [], trail };

    const partials = [];
    if (current.missing.length) partials.push(`${current.missing.length} asset(s) missing`);
    if (current.unsupported.length) partials.push(`${current.unsupported.join('/')} not shown`);
    if (!man.game_available && current.missing.length) partials.push('game assets unavailable');
    note.textContent = partials.length ? 'Partial preview — ' + partials.join(' · ') : '';
    note.style.display = partials.length ? 'block' : 'none';

    autofit.active = true; autofit.scale = 0; autofit.t = 0;
    loading.style.display = 'none';
    raf = requestAnimationFrame(frame);
  }

  const inst = new Float32Array(20000 * FLOATS_PER_INSTANCE);
  const rib = new Float32Array(60000 * RIBBON_FLOATS_PER_VERT);
  const mbuf = new Float32Array(20000 * MESH_FLOATS_PER_INSTANCE);

  function packMesh(ls, r, meshes) {
    const n = ls.count; if (!n) return;
    let o = 0;
    for (let i = 0; i < n; i++) {
      const p = ls.getAt(i, 'Position'); const sf = ls.getAt(i, r.scaleField);
      const s = (sf[0] || 1);
      const hx = Math.abs(r.scale[0] * (sf[0] ?? s)) * 0.5, hy = Math.abs(r.scale[1] * (sf[1] ?? s)) * 0.5, hz = Math.abs(r.scale[2] * (sf[2] ?? s)) * 0.5;
      const col = ls.getAt(i, r.colorField);
      mbuf[o++] = p[0]; mbuf[o++] = p[1]; mbuf[o++] = p[2];
      mbuf[o++] = hx || 0.05; mbuf[o++] = hy || 0.05; mbuf[o++] = hz || 0.05;
      mbuf[o++] = (col[0] ?? 1) * r.diffuseColor[0]; mbuf[o++] = (col[1] ?? 1) * r.diffuseColor[1];
      mbuf[o++] = (col[2] ?? 1) * r.diffuseColor[2]; mbuf[o++] = col[3] ?? 1;
    }
    meshes.push({ instances: mbuf.slice(0, o), count: n, drawOrder: r.drawOrder });
  }

  function packLight(ls, r, draws) {
    const n = ls.count; if (!n) return;
    let o = 0;
    for (let i = 0; i < n; i++) {
      const p = ls.getAt(i, 'Position'); const col = ls.getAt(i, r.colorField);
      const rad = (r.radiusField ? ls.getAt(i, r.radiusField)[0] : r.radius) || 1;
      const sz = rad * 0.5;
      inst[o++] = p[0]; inst[o++] = p[1]; inst[o++] = p[2];
      inst[o++] = sz; inst[o++] = sz;
      inst[o++] = col[0] ?? 1; inst[o++] = col[1] ?? 1; inst[o++] = col[2] ?? 1; inst[o++] = Math.min(1, (col[3] ?? 1));
      inst[o++] = 0; inst[o++] = 0; inst[o++] = 0; inst[o++] = 1; inst[o++] = 1;
    }
    draws.push({ texture: glowTex, blend: 'add', instances: inst.slice(0, o), count: n, drawOrder: r.drawOrder });
  }

  function packBillboards(ls, r, draws) {
    const n = ls.count; if (!n) return;
    let o = 0; const atlas = r._atlas, alen = atlas ? atlas.length : 0;
    for (let i = 0; i < n; i++) {
      const p = ls.getAt(i, 'Position'), sz = ls.getAt(i, r.sizeField), col = ls.getAt(i, r.colorField);
      const rot = ls.getAt(i, r.rotationField)[0] || 0;
      let u0 = 0, v0 = 0, du = 1, dv = 1;
      if (alen) {
        let f = Math.floor(ls.getAt(i, 'TextureID')[0] || 0); f = Math.max(0, Math.min(alen - 1, f));
        const rc = atlas[f]; u0 = rc[0]; v0 = rc[1]; du = rc[2] - rc[0]; dv = rc[3] - rc[1];
      }
      inst[o++] = p[0]; inst[o++] = p[1]; inst[o++] = p[2];
      inst[o++] = sz[0] ?? 1; inst[o++] = sz[1] ?? sz[0] ?? 1;
      inst[o++] = col[0] ?? 1; inst[o++] = col[1] ?? 1; inst[o++] = col[2] ?? 1; inst[o++] = col[3] ?? 1;
      inst[o++] = rot; inst[o++] = u0; inst[o++] = v0; inst[o++] = du; inst[o++] = dv;
    }
    draws.push({ texture: r._tex, blend: r._blend, instances: inst.slice(0, o), count: n, drawOrder: r.drawOrder });
  }

  function packRibbon(ls, r, eye, ribbons) {
    const n = ls.count; if (n < 2) return;
    // order the layer's particles oldest -> newest (the ribbon follows their path)
    const order = Array.from({ length: n }, (_, i) => i).sort((a, b) => ls.getAt(b, 'Age')[0] - ls.getAt(a, 'Age')[0]);
    const C = order.map((i) => ls.getAt(i, 'Position'));
    let o = 0;
    const push = (p, u, v, c) => { rib[o++] = p[0]; rib[o++] = p[1]; rib[o++] = p[2]; rib[o++] = u; rib[o++] = v; rib[o++] = c[0] ?? 1; rib[o++] = c[1] ?? 1; rib[o++] = c[2] ?? 1; rib[o++] = c[3] ?? 1; };
    const edge = (k) => {
      const i = order[k], c = C[k];
      const prev = C[k - 1] || c, next = C[k + 1] || c;
      const tan = norm(sub(next, prev));
      const toEye = norm(sub(eye, c));
      let side = norm(cross(tan, toEye));
      if (!isFinite(side[0]) || (side[0] === 0 && side[1] === 0 && side[2] === 0)) side = [1, 0, 0];
      const hw = (ls.getAt(i, r.sizeField)[0] || 0.2) * 0.5;
      const life = ls.getAt(i, 'Age')[0] / (ls.getAt(i, 'Life')[0] || 1);
      const u = r.textureUField === 'LifeRatio' ? life : k / (n - 1);
      const col = ls.getAt(i, r.colorField);
      return { L: add(c, mul(side, hw)), R: sub2(c, mul(side, hw)), u, col };
    };
    let a = edge(0);
    for (let k = 1; k < n; k++) {
      const b = edge(k);
      push(a.L, a.u, 0, a.col); push(a.R, a.u, 1, a.col); push(b.L, b.u, 0, b.col);
      push(a.R, a.u, 1, a.col); push(b.R, b.u, 1, b.col); push(b.L, b.u, 0, b.col);
      a = b;
    }
    ribbons.push({ texture: r._tex, blend: r._blend, vertices: rib.slice(0, o), count: o / RIBBON_FLOATS_PER_VERT, drawOrder: r.drawOrder });
  }

  const autofit = { active: true, scale: 0, t: 0 };
  function frame() {
    if (disposed) return;
    const dt = Math.min(0.05, 1 / 60);
    autofit.t += dt;
    // brief static phase up front to gauge the effect's own footprint (before any sweep)
    const measuring = autofit.t < 0.8;
    const trail = !!(current && current.trail);

    // Move the emitter through space so a trail forms; the camera then FOLLOWS it (below),
    // so older particles stream behind it like a comet — the way you'd swing the effect
    // around in the PopcornFX editor, rather than the whole thing spinning in a ring.
    if (system && trail && !measuring) {
      const ts = autofit.t, R = autofit.scale * 1.2 + 0.3;
      system.emitter[0] = Math.sin(ts * 1.6) * R;
      system.emitter[2] = Math.sin(ts * 3.2) * R * 0.5;
    }
    if (system) system.update(dt);

    const draws = [], ribbons = [], meshes = [];
    const eye = renderer.eyePosition();
    let sumY = 0, cnt = 0, maxR2 = 0;
    if (system) {
      for (const ls of system.layers) {
        for (let i = 0; i < ls.count; i++) {
          const p = ls.getAt(i, 'Position'); sumY += p[1]; cnt++;
          const r2 = p[0] * p[0] + p[1] * p[1] + p[2] * p[2]; if (r2 > maxR2) maxR2 = r2;
        }
        for (const r of ls.L.renderers) {
          if (r.kind === 'billboard') packBillboards(ls, r, draws);
          else if (r.kind === 'ribbon') packRibbon(ls, r, eye, ribbons);
          else if (r.kind === 'mesh') packMesh(ls, r, meshes);
          else if (r.kind === 'light') packLight(ls, r, draws);
        }
      }
    }
    if (cnt && measuring) autofit.scale = Math.max(autofit.scale, Math.sqrt(maxR2));

    // Camera: trail effects keep the moving emitter framed (so it stays on screen and the
    // trail streams behind it) — this follow persists through user orbit/zoom. Otherwise
    // centre on the particle centroid. Auto-distance only until the user interacts.
    if (cnt) {
      if (trail && !measuring) {
        const e = system.emitter;
        renderer.cam.target[0] += (e[0] - renderer.cam.target[0]) * 0.1;
        renderer.cam.target[1] += (e[1] - renderer.cam.target[1]) * 0.1;
        renderer.cam.target[2] += (e[2] - renderer.cam.target[2]) * 0.1;
        if (autofit.active) renderer.cam.dist += (clamp(autofit.scale * 3 + 0.8, 2, 60) - renderer.cam.dist) * 0.05;
      } else if (autofit.active) {
        renderer.cam.dist += (clamp((autofit.scale || Math.sqrt(maxR2)) * 2.2 + 0.6, 2, 60) - renderer.cam.dist) * 0.1;
        renderer.cam.target[1] += (sumY / cnt - renderer.cam.target[1]) * 0.1;
      }
    }
    renderer.draw(draws, ribbons, meshes);
    raf = requestAnimationFrame(frame);
  }

  // orbit controls
  let drag = false, px = 0, py = 0;
  const onDown = (e) => { drag = true; px = e.clientX; py = e.clientY; autofit.active = false; };
  const onUp = () => { drag = false; };
  const onMove = (e) => { if (!drag) return; renderer.cam.az -= (e.clientX - px) * 0.01; renderer.cam.el = clamp(renderer.cam.el + (e.clientY - py) * 0.01, -1.5, 1.5); px = e.clientX; py = e.clientY; };
  const onWheel = (e) => { e.preventDefault(); autofit.active = false; renderer.cam.dist = clamp(renderer.cam.dist * (1 + Math.sign(e.deltaY) * 0.1), 1, 120); };
  canvas.addEventListener('pointerdown', onDown);
  window.addEventListener('pointerup', onUp);
  window.addEventListener('pointermove', onMove);
  canvas.addEventListener('wheel', onWheel, { passive: false });

  load().catch((e) => {
    loading.style.display = 'none';
    note.style.display = 'block'; note.textContent = 'Preview failed: ' + e.message;
  });

  return {
    dispose() {
      disposed = true;
      cancelAnimationFrame(raf);
      window.removeEventListener('pointerup', onUp);
      window.removeEventListener('pointermove', onMove);
      const gl = renderer && renderer.gl;
      if (gl) { const ext = gl.getExtension('WEBGL_lose_context'); if (ext) ext.loseContext(); }
      container.innerHTML = '';
    },
  };
}

// Modal wrapper; UX matches the blueprint viewer.
let _stylesDone = false;
function injectStyles() {
  if (_stylesDone) return; _stylesDone = true;
  const css =
    '.pkfxv-overlay{position:fixed;inset:0;z-index:9999;background:rgba(4,7,12,.78);display:flex;' +
      'align-items:center;justify-content:center;padding:20px;backdrop-filter:blur(2px)}' +
    '.pkfxv-modal{display:flex;flex-direction:column;width:min(900px,94vw);height:min(680px,88vh);' +
      'background:#10151c;border:1px solid #232a33;border-radius:14px;overflow:hidden;box-shadow:0 24px 60px rgba(0,0,0,.5)}' +
    '.pkfxv-head{display:flex;align-items:center;gap:12px;padding:11px 14px;border-bottom:1px solid #232a33;flex:0 0 auto}' +
    '.pkfxv-title{font-weight:700;color:#e6edf3;font-size:.98rem;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;flex:1 1 auto}' +
    '.pkfxv-close{flex:0 0 auto;background:transparent;border:0;color:#9aa4b2;font-size:1.5rem;line-height:1;cursor:pointer;padding:0 4px}' +
    '.pkfxv-close:hover{color:#e6edf3}' +
    '.pkfxv-body{position:relative;flex:1 1 auto;min-height:0;background:radial-gradient(120% 120% at 50% 30%,#15151d,#0b0b10)}' +
    '.pkfx-canvas{display:block;width:100%;height:100%;cursor:grab;touch-action:none}' +
    '.pkfx-canvas:active{cursor:grabbing}' +
    '.pkfx-note{position:absolute;left:10px;bottom:10px;right:10px;font-size:.74rem;color:#c7b6a0;' +
      'background:rgba(10,10,14,.55);padding:5px 9px;border-radius:6px;pointer-events:none;display:none}' +
    '.pkfxv-hint{position:absolute;right:10px;bottom:34px;color:#5a6270;font-size:.7rem;pointer-events:none}' +
    '.pkfxv-foot{flex:0 0 auto;padding:7px 14px;border-top:1px solid #232a33;color:#7c8696;font-size:.72rem;' +
      'display:flex;align-items:center;gap:7px}' +
    '.pkfxv-foot i{color:#c79a52}' +
    '.pkfx-loading{position:absolute;inset:0;display:flex;align-items:center;justify-content:center;gap:10px;' +
      'color:#aab;font-size:.85rem;background:rgba(11,11,16,.6)}' +
    '.pkfx-spinner{width:16px;height:16px;border:2px solid #2c3340;border-top-color:#7aa7ff;border-radius:50%;' +
      'display:inline-block;animation:pkfxspin .7s linear infinite}' +
    '@keyframes pkfxspin{to{transform:rotate(360deg)}}';
  const s = document.createElement('style'); s.textContent = css; document.head.appendChild(s);
}

export function open({ releaseId, path, title, endpoint }) {
  injectStyles();
  const ov = document.createElement('div');
  ov.className = 'pkfxv-overlay';
  ov.innerHTML =
    '<div class="pkfxv-modal">' +
      '<div class="pkfxv-head"><span class="pkfxv-title"></span>' +
        '<button class="pkfxv-close" type="button" aria-label="Close">×</button></div>' +
      '<div class="pkfxv-body"><div class="pkfxv-hint">drag to orbit · scroll to zoom</div></div>' +
      '<div class="pkfxv-foot"><i class="fa-solid fa-circle-info"></i>' +
        '<span>VFX may not render completely — when in doubt, test it in game.</span></div>' +
    '</div>';
  ov.querySelector('.pkfxv-title').textContent = title || (path || '').split('/').pop() || 'VFX';
  document.body.appendChild(ov);

  const body = ov.querySelector('.pkfxv-body');
  const viewer = mount(body, { releaseId, path, endpoint });

  let closed = false;
  function close() {
    if (closed) return; closed = true;
    document.removeEventListener('keydown', onKey);
    try { viewer.dispose(); } catch (_) {}
    if (ov.parentNode) ov.parentNode.removeChild(ov);
  }
  function onKey(e) { if (e.key === 'Escape') close(); }
  ov.querySelector('.pkfxv-close').addEventListener('click', close);
  ov.addEventListener('mousedown', (e) => { if (e.target === ov) close(); });
  document.addEventListener('keydown', onKey);
  return { close };
}

const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const sub2 = sub;
const add = (a, b) => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
const mul = (a, s) => [a[0] * s, a[1] * s, a[2] * s];
const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const norm = (a) => { const l = Math.hypot(a[0], a[1], a[2]) || 1; return [a[0] / l, a[1] / l, a[2] / l]; };
const clamp = (x, a, b) => Math.min(Math.max(x, a), b);

// soft radial-gradient texture used as the stand-in glow for Light particles
function makeGlowTexture(gl, size = 64) {
  const rgba = new Uint8ClampedArray(size * size * 4);
  const c = (size - 1) / 2;
  for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) {
    const d = Math.hypot(x - c, y - c) / c;
    const a = Math.max(0, 1 - d); const v = a * a * 255;
    const o = (y * size + x) * 4;
    rgba[o] = 255; rgba[o + 1] = 255; rgba[o + 2] = 255; rgba[o + 3] = v;
  }
  return makeTexture(gl, size, size, rgba);
}

if (typeof window !== 'undefined') window.PkfxViewer = { open, mount };

/* Kiwi VFX preview — plays a mod release's PopcornFX .pkfx effect in WebGL2.

   The server (/site/mods/releases/<id>/vfx/*) hands us the .pkfx text plus every
   asset it references, resolving missing textures/meshes from the live game tree.
   We parse, simulate on the CPU, and render billboards + ribbons + meshes here.

   The effect plays in place — nothing sweeps the emitter around on its own, so what you
   see is what the effect does when it is spawned. Shift-drag moves it if you want to see
   how the trails stream.

   Public API (assigned to window.PkfxViewer for classic-script callers):
     PkfxViewer.mount(container, { releaseId, path }) -> { dispose() }
     PkfxViewer.mount(container, { endpoint: {base, query}, path })  // embeddable viewer
     `backdrop` (optional) is a URL returning the model the effect is worn on - a weapon
     or a head, already placed in the effect's own space (app/embed/backdrop.py). */
import { parsePkfx } from './parser.js';
import { buildEffect } from './model.js';
import { System } from './sim.js';
import { decodeDDS } from './dds.js';
import { decodePkmm } from './pkmm.js';
import { AnimTrackSampler, ShapeSampler } from './curves.js';
import { Renderer, makeLevelsTexture, makeImageTexture, MESH_FLOATS_PER_INSTANCE } from './renderer.js';
import { FramePacker, blendKind, mergeMediums } from './packer.js';

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

/* Backdrop parts -> instanced unit cubes: each voxel's centre and the part matrix's
   basis columns, which carry the voxel size. */
function backdropInstances(parts) {
  const n = parts.reduce((a, p) => a + p.x.length, 0);
  const out = new Float32Array(n * MESH_FLOATS_PER_INSTANCE);
  const lo = [Infinity, Infinity, Infinity], hi = [-Infinity, -Infinity, -Infinity];
  let o = 0;
  for (const p of parts) {
    const m = p.m;
    for (let i = 0; i < p.x.length; i++) {
      const x = p.x[i], y = p.y[i], z = p.z[i], rgb = p.rgb[i];
      const c = [m[0] * x + m[4] * y + m[8] * z + m[12], m[1] * x + m[5] * y + m[9] * z + m[13], m[2] * x + m[6] * y + m[10] * z + m[14]];
      for (let k = 0; k < 9; k++) out[o++] = m[k + (k / 3 | 0)];
      out[o++] = c[0]; out[o++] = c[1]; out[o++] = c[2];
      out[o++] = ((rgb >> 16) & 255) / 255; out[o++] = ((rgb >> 8) & 255) / 255; out[o++] = (rgb & 255) / 255; out[o++] = 1;
      for (let k = 0; k < 3; k++) { if (c[k] < lo[k]) lo[k] = c[k]; if (c[k] > hi[k]) hi[k] = c[k]; }
    }
  }
  return { instances: out, count: n, lo, hi };
}

export function mount(container, { releaseId, path, endpoint, backdrop: backdropUrl }) {
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

  let renderer, system, current, raf = 0, disposed = false, backdrop = null;
  const texCache = new Map(), atlasCache = new Map(), meshCache = new Map();

  const assetUrl = (ref) => urls.asset(ref);

  // null when the texture is missing or unreadable; callers choose the stand-in
  async function loadTexture(ref) {
    if (!ref) return null;
    if (texCache.has(ref)) return texCache.get(ref);
    const p = (async () => {
      try {
        const res = await fetch(assetUrl(ref));
        if (!res.ok) throw new Error(res.status);
        // a DDS keeps exactly the mip levels it stores, as Trove uploads it
        if (/\.dds$/i.test(ref)) return makeLevelsTexture(renderer.gl, decodeDDS(await res.arrayBuffer()).levels);
        // straight alpha: a 2D canvas round trip would premultiply it
        const bmp = await createImageBitmap(await res.blob(), { premultiplyAlpha: 'none', colorSpaceConversion: 'none' });
        try { return makeImageTexture(renderer.gl, bmp); } finally { bmp.close(); }
      } catch { return null; }
    })();
    texCache.set(ref, p);
    const tex = await p; texCache.set(ref, tex); return tex;
  }

  async function loadAtlas(ref) {
    if (!ref) return null;
    if (atlasCache.has(ref)) return atlasCache.get(ref);
    try {
      const txt = await (await fetch(assetUrl(ref))).text();
      // each rect is sorted to [umin, vmin, umax, vmax]: a reversed rect does not flip
      const rects = txt.trim().split(/\r?\n/).map((l) => {
        const [a, b, c, d] = l.split(',').map((n) => parseFloat(n.trim()));
        return [Math.min(a, c), Math.min(b, d), Math.max(a, c), Math.max(b, d)];
      });
      atlasCache.set(ref, rects); return rects;
    } catch { atlasCache.set(ref, null); return null; }
  }

  /* .pkmm -> uploaded geometry; `sub` >= 0 picks one submesh (SubMeshId). {missing} when
     it does not load, {empty} when it holds no submesh, null when its layout is not
     recognized (the renderer's cube proxy). */
  async function loadMesh(ref, sub = -1) {
    if (!ref || !/\.pkmm$/i.test(ref)) return null;
    const key = `${ref}#${sub}`;
    if (meshCache.has(key)) return meshCache.get(key);
    const p = (async () => {
      let buf;
      try {
        const res = await fetch(assetUrl(ref));
        if (!res.ok) throw new Error(res.status);
        buf = await res.arrayBuffer();
      } catch { return { missing: true }; }
      const mesh = decodePkmm(buf);
      if (!mesh) return null;
      const part = sub >= 0 ? mesh.blocks[sub] : mesh;
      return !part || mesh.empty ? { empty: true } : renderer.makeMeshGeometry(part);
    })();
    meshCache.set(key, p);
    const g = await p; meshCache.set(key, g); return g;
  }

  // MESH shapes (inside collections too) sample a .pkmm submesh on the CPU
  async function loadShapeMeshes(effect) {
    const shapes = new Set();
    const visit = (s) => {
      if (!(s instanceof ShapeSampler) || shapes.has(s)) return;
      shapes.add(s);
      for (const sub of s.subs || []) visit(sub);
    };
    for (const layer of effect.layers) for (const s of Object.values(layer.samplers)) visit(s);
    await Promise.all([...shapes].map(async (s) => {
      const ref = s.meshResourceRef();
      if (!ref) return;
      try {
        const res = await fetch(assetUrl(ref));
        if (res.ok) { const m = decodePkmm(await res.arrayBuffer()); if (m) s.loadMesh(m); }
      } catch { /* no surface to spawn on; the rest of the effect still plays */ }
    }));
  }

  /* AnimTrack samplers hold a motion path that lives in a separate .pkan, so they
     are inert until it is fetched and parsed. Do that once per sampler object -
     the same instance can be shared by several layers via the global sampler list. */
  async function loadAnimTracks(effect) {
    const seen = new Set();
    const jobs = [];
    for (const layer of effect.layers) {
      for (const s of Object.values(layer.samplers)) {
        if (!(s instanceof AnimTrackSampler) || seen.has(s)) continue;
        seen.add(s);
        const ref = s.resourceRef();
        if (!ref) continue;
        jobs.push((async () => {
          try {
            const res = await fetch(assetUrl(ref));
            if (res.ok) s.load(parsePkfx(await res.text()));
          } catch { /* leave the path inert; the effect still plays */ }
        })());
      }
    }
    await Promise.all(jobs);
  }

  async function load() {
    renderer = new Renderer(canvas);
    const man = await (await fetch(urls.manifest(path))).json();
    if (disposed) return;
    // Mods switch an effect off by shipping it as an empty file; nothing plays in game.
    if (!/\S/.test(man.pkfx || '')) {
      loading.style.display = 'none';
      note.textContent = 'This effect is empty. The mod switches it off, so nothing plays in game.';
      note.style.display = 'block';
      return;
    }
    const doc = parsePkfx(man.pkfx);
    const effect = buildEffect(doc, Math.random);

    /* In the game a renderer whose texture, alpha remapper or mesh does not load draws
       nothing (FUN_1402b3480, FUN_1402b78d0). The server resolves assets from the live
       game tree, so a missing one is missing in game too; stand-ins are only for when
       the game's files are unavailable to the server. */
    const standIns = man.game_available === false;
    const unsupported = new Set();
    for (const layer of effect.layers) {
      for (const r of layer.renderers) {
        if (r.kind === 'billboard' || r.kind === 'ribbon') {
          // No Diffuse at all: the engine falls back to its magenta "missing texture"
          // debug sprite, which the game does not show, so draw nothing.
          if (!r.diffuse) { r._skip = true; continue; }
          const ribbon = r.kind === 'ribbon';
          const [tex, atlas, remap] = await Promise.all([
            loadTexture(r.diffuse), loadAtlas(r.atlas), r.alphaRemap ? loadTexture(r.alphaRemap) : null,
          ]);
          if (!standIns && (!tex || (r.alphaRemap && !remap))) { r._skip = true; continue; }
          r._tex = tex || renderer.placeholder;
          r._atlas = atlas;
          r._remap = remap;
          r._kind = blendKind(r.material, ribbon);
          // a _Soft material fades where it meets opaque geometry; Trove's ribbon shaders never read depth
          r._soft = !ribbon && /_Soft/i.test(r.material) ? Math.max(r.softness, 1e-3) : 0;
        } else if (r.kind === 'mesh') {
          const geom = await loadMesh(r.mesh, r.subMesh);
          if (geom && (geom.empty || (geom.missing && !standIns))) { r._skip = true; continue; }
          r._geom = geom && !geom.missing ? geom : null;
          r._tex = (await loadTexture(r.diffuse)) || renderer.white;   // untextured meshes draw their colour
          r._lit = !/Additive/i.test(r.material);
          r._kind = r._lit ? 0 : 1;
        } else if (r.cls) unsupported.add(r.cls.replace('CParticleRenderer_', ''));
      }
    }
    await Promise.all([loadAnimTracks(effect), loadShapeMeshes(effect)]);
    if (backdropUrl) {
      try {
        const res = await fetch(backdropUrl);
        if (res.ok) backdrop = backdropInstances((await res.json()).parts || []);
      } catch { backdrop = null; }   // the effect still plays on its own
      if (backdrop && !backdrop.count) backdrop = null;
      if (backdrop) {
        // look across a long weapon rather than down its length
        const [lo, hi] = [backdrop.lo, backdrop.hi];
        if (hi[2] - lo[2] > hi[0] - lo[0]) renderer.cam.az = Math.PI / 2;
        renderer.cam.target = [(lo[0] + hi[0]) / 2, (lo[1] + hi[1]) / 2, (lo[2] + hi[2]) / 2];
        // framed on the model it is worn on, not on sparks that drift off it
        renderer.cam.dist = Math.max(Math.hypot(hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]) * 1.6, 0.8);
      }
    }
    if (disposed) return;
    system = new System(effect, Math.random);
    current = { effect, unsupported: [...unsupported], missing: man.missing || [] };

    const partials = [];
    if (current.missing.length) partials.push(`${current.missing.length} asset(s) missing`);
    if (current.unsupported.length) partials.push(`${current.unsupported.join('/')} not shown`);
    if (!man.game_available && current.missing.length) partials.push('game assets unavailable');
    // a value the engine cannot read cuts the file short, in game as here
    if (doc.aborted) partials.push('the game stops reading this file partway');
    note.textContent = partials.length ? 'Partial preview — ' + partials.join(' · ') : '';
    note.style.display = partials.length ? 'block' : 'none';

    autofit.active = !backdrop; autofit.scale = 0; autofit.t = 0; autofit.floor = null; autofit.y1 = null;
    loading.style.display = 'none';
    raf = requestAnimationFrame(frame);
  }

  // billboards, ribbons and meshes -> draw items (packer.js, shared with PopcornFX-Web)
  const packer = new FramePacker();

  const autofit = { active: true, scale: 0, t: 0, floor: null, y1: null };
  // Real elapsed time per frame, so the effect plays at game speed on any refresh rate
  // (a fixed 1/60 per frame ran 2.4x fast at 144 Hz). Capped so a paused tab doesn't jump.
  let lastT = 0;
  function frame(now) {
    if (disposed) return;
    const dt = lastT && now > lastT ? Math.min((now - lastT) / 1000, 0.05) : 1 / 60;
    lastT = now;
    tick(dt);
    raf = requestAnimationFrame(frame);
  }
  function tick(dt = 1 / 60) {
    autofit.t += dt;
    // Opening window used to gauge the effect's footprint for the initial framing.
    const measuring = autofit.t < 1.5;

    // The emitter only ever moves when the user drags it (see the controls below) — the
    // effect plays in place, the way it does in the PopcornFX editor. Trails and
    // localspace-attached layers therefore look exactly as authored.
    if (system) {
      system.camPos = renderer.eyePosition();
      system.camTarget = renderer.cam.target;
      try { system.update(dt); }
      catch (e) { if (!system._crashWarned) { system._crashWarned = true; console.warn('pkfx sim error:', e); } }
    }

    const items = [];
    const eye = renderer.eyePosition();
    const t = renderer.cam.target, vl = Math.hypot(t[0] - eye[0], t[1] - eye[1], t[2] - eye[2]) || 1;
    const viewDir = [(t[0] - eye[0]) / vl, (t[1] - eye[1]) / vl, (t[2] - eye[2]) / vl];
    let cnt = 0, maxR2 = 0, minY = Infinity, maxY = -Infinity;
    if (backdrop) {
      items.push({ type: 'mesh', geom: null, texture: renderer.white, lit: true, shade: true, kind: 0,
                   instances: backdrop.instances, count: backdrop.count, drawOrder: 0 });
    }
    if (system) {
      for (const ls of system.layers) {
        for (let i = 0; i < ls.count; i++) {
          const p = ls.getAt(i, 'Position'); cnt++;
          if (p[1] < minY && isFinite(p[1])) minY = p[1];
          if (p[1] > maxY && isFinite(p[1])) maxY = p[1];
          const r2 = p[0] * p[0] + p[1] * p[1] + p[2] * p[2]; if (r2 > maxR2 && isFinite(r2)) maxR2 = r2;
        }
        for (const r of ls.L.renderers) {
          if (r._skip) continue;
          if (r.kind === 'billboard') packer.billboards(ls, r, eye, viewDir, items);
          else if (r.kind === 'ribbon') packer.ribbon(ls, r, eye, items);
          else if (r.kind === 'mesh') packer.mesh(ls, r, items);
        }
      }
    }
    if (cnt && measuring) {
      autofit.scale = Math.max(autofit.scale, Math.sqrt(maxR2));
      autofit.y1 = Math.max(autofit.y1 ?? -Infinity, maxY);
    }

    /* No ground. It was added to give soft particles something to fade against, but
       an effect is authored to be seen against the world, not against a slab we
       invented - side by side with the game the plane read as a hard-edged wedge cut
       through the portal. Soft particles keep working; with nothing opaque in the
       depth pass the fade is simply a no-op, which is the honest result for a preview
       that has no scene. `autofit.floor` stays measured in case a real backdrop is
       ever added. */
    if (cnt && measuring) autofit.floor = Math.min(autofit.floor ?? Infinity, minY);

    /* Framing: during the opening window only, ease toward the middle of the measured
       height range and a distance from the measured extent, then hold still. Following
       the live particles after that made the whole view bob as sparks rose and fell, so
       static effects looked like they were drifting. */
    if (autofit.active && measuring && autofit.scale > 0 && isFinite(autofit.floor) && isFinite(autofit.y1)) {
      renderer.cam.dist += (clamp(autofit.scale * 2.2 + 0.6, 2, 60) - renderer.cam.dist) * 0.15;
      renderer.cam.target[1] += ((autofit.floor + autofit.y1) / 2 - renderer.cam.target[1]) * 0.15;
    }
    renderer.draw(mergeMediums(items));
  }

  // Controls: drag orbits the camera; shift-drag (or right-drag) pulls the EFFECT through
  // the scene, the way you'd drag the emitter around in the PopcornFX editor. That drag is
  // the only thing that ever moves the emitter, so trails and localspace-attached layers
  // only stream when the user asks them to.
  const ORBIT = 1, MOVE = 2;
  let drag = 0, px = 0, py = 0;
  const onDown = (e) => {
    drag = (e.shiftKey || e.button === 2) ? MOVE : ORBIT;
    px = e.clientX; py = e.clientY;
    autofit.active = false;
  };
  const onUp = () => { drag = 0; };
  const onMove = (e) => {
    if (!drag) return;
    const dx = e.clientX - px, dy = e.clientY - py;
    px = e.clientX; py = e.clientY;
    if (drag === MOVE) {
      if (!system) return;
      // screen delta -> world delta across the camera plane (≈1:1 at the orbit target)
      const { az, el, dist } = renderer.cam;
      const k = 2 * dist * Math.tan(30 * Math.PI / 180) / Math.max(canvas.clientHeight, 1);
      const right = [Math.cos(az), 0, -Math.sin(az)];
      const up = [-Math.sin(el) * Math.sin(az), Math.cos(el), -Math.sin(el) * Math.cos(az)];
      for (let k2 = 0; k2 < 3; k2++) system.emitter[k2] += (right[k2] * dx - up[k2] * dy) * k;
      return;
    }
    renderer.cam.az -= dx * 0.01;
    renderer.cam.el = clamp(renderer.cam.el + dy * 0.01, -1.5, 1.5);
  };
  const onWheel = (e) => { e.preventDefault(); autofit.active = false; renderer.cam.dist = clamp(renderer.cam.dist * (1 + Math.sign(e.deltaY) * 0.1), 0.3, 120); };
  const onCtx = (e) => e.preventDefault();   // right-drag is a control, not a context menu
  canvas.addEventListener('pointerdown', onDown);
  window.addEventListener('pointerup', onUp);
  window.addEventListener('pointermove', onMove);
  canvas.addEventListener('wheel', onWheel, { passive: false });
  canvas.addEventListener('contextmenu', onCtx);

  load().catch((e) => {
    loading.style.display = 'none';
    note.style.display = 'block'; note.textContent = 'Preview failed: ' + e.message;
  });

  return {
    // test/debug hook: advance + draw one frame, report what's alive and on screen
    tick() {
      if (!renderer || !system) return null;
      tick();
      const gl = renderer.gl;
      const w = Math.min(canvas.width, 256), h = Math.min(canvas.height, 256);
      const px = new Uint8Array(w * h * 4);
      gl.readPixels((canvas.width - w) >> 1, (canvas.height - h) >> 1, w, h, gl.RGBA, gl.UNSIGNED_BYTE, px);
      let lit = 0;
      for (let i = 0; i < px.length; i += 4) if (px[i] > 20 || px[i + 1] > 20 || px[i + 2] > 24) lit++;
      let alive = 0;
      for (const ls of system.layers) alive += ls.count;
      return {
        alive, litPixels: lit, sampled: w * h,
        emitter: Array.from(system.emitter),
        layers: system.layers.map((l) => ({ name: l.L.name, count: l.count })),
      };
    },
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
      '<div class="pkfxv-body"><div class="pkfxv-hint">drag to orbit · scroll to zoom · shift-drag to move the effect</div></div>' +
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

const clamp = (x, a, b) => Math.min(Math.max(x, a), b);

if (typeof window !== 'undefined') window.PkfxViewer = { open, mount };

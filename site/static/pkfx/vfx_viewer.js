/* Kiwi VFX preview — plays a mod release's PopcornFX .pkfx effect in WebGL2.

   The server (/site/mods/releases/<id>/vfx/*) hands us the .pkfx text plus every
   asset it references, resolving missing textures/meshes from the live game tree.
   We parse, simulate on the CPU, and render billboards + ribbons + meshes here.

   The effect plays in place — nothing sweeps the emitter around on its own, so what you
   see is what the effect does when it is spawned. Shift-drag moves it if you want to see
   how the trails stream.

   Public API (assigned to window.PkfxViewer for classic-script callers):
     PkfxViewer.mount(container, { releaseId, path }) -> { dispose() }
     PkfxViewer.mount(container, { endpoint: {base, query}, path })  // embeddable viewer */
import { parsePkfx } from './parser.js';
import { buildEffect } from './model.js';
import { System } from './sim.js';
import { decodeDDS } from './dds.js';
import { decodePkmm } from './pkmm.js';
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

// BillboardingMaterial -> blend kind (0 alpha, 1 additive, 2 alphablend+additive, 3 additive-noalpha)
function kindFor(material) {
  if (/Additive_NoAlpha/i.test(material)) return 3;
  if (/^AlphaBlend_Additive/i.test(material)) return 2;
  if (/^Additive/i.test(material)) return 1;
  return 0;
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
  const texCache = new Map(), atlasCache = new Map(), meshCache = new Map();

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

  // .pkmm -> uploaded geometry (null -> the renderer's cube proxy)
  async function loadMesh(ref) {
    if (!ref || !/\.pkmm$/i.test(ref)) return null;
    if (meshCache.has(ref)) return meshCache.get(ref);
    const p = (async () => {
      try {
        const res = await fetch(assetUrl(ref));
        if (!res.ok) throw new Error(res.status);
        const mesh = decodePkmm(await res.arrayBuffer());
        return mesh ? renderer.makeMeshGeometry(mesh) : null;
      } catch { return null; }
    })();
    meshCache.set(ref, p);
    const g = await p; meshCache.set(ref, g); return g;
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
        if (r.kind === 'billboard') {
          r._tex = await loadTexture(r.diffuse);
          r._atlas = await loadAtlas(r.atlas);
          r._remap = r.alphaRemap ? await loadTexture(r.alphaRemap) : null;
          r._kind = kindFor(r.material);
        } else if (r.kind === 'ribbon') {
          r._tex = await loadTexture(r.diffuse);
          r._atlas = await loadAtlas(r.atlas);
          r._kind = kindFor(r.material);
        } else if (r.kind === 'light') {
          r._tex = glowTex; r._kind = 1;
        } else if (r.kind === 'mesh') {
          r._geom = await loadMesh(r.mesh);
          r._tex = await loadTexture(r.diffuse);
          r._lit = !/Additive/i.test(r.material);
          r._kind = /Additive_NoAlpha/i.test(r.material) ? 3 : /Additive/i.test(r.material) ? 1 : 0;
        } else if (r.cls) unsupported.add(r.cls.replace('CParticleRenderer_', ''));
      }
    }
    if (disposed) return;
    system = new System(effect, Math.random);
    current = { effect, unsupported: [...unsupported], missing: man.missing || [] };

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
  const mbuf = new Float32Array(4000 * MESH_FLOATS_PER_INSTANCE);

  function frameRect(r, tid, alen) {
    // atlas rects are [u0,v0,u1,v1]; VFlipUVs mirrors v
    let f = Math.floor(tid); if (!isFinite(f)) f = 0;
    f = ((f % alen) + alen) % alen;
    const rc = r._atlas[f];
    if (r.vflip) return [rc[0], rc[3], rc[2] - rc[0], rc[1] - rc[3]];
    return [rc[0], rc[1], rc[2] - rc[0], rc[3] - rc[1]];
  }

  function packBillboards(ls, r, items) {
    const n = ls.count; if (!n) return;
    let o = 0; const alen = r._atlas ? r._atlas.length : 0;
    const mode = r.mode;
    for (let i = 0; i < n; i++) {
      const p = ls.getAt(i, r.positionField);
      if (!isFinite(p[0]) || !isFinite(p[1]) || !isFinite(p[2])) continue;
      const sz = ls.getAt(i, r.sizeField), col = ls.getAt(i, r.colorField);
      const rot = ls.getAt(i, r.rotationField)[0] || 0;
      let u0 = 0, v0 = r.vflip ? 1 : 0, du = 1, dv = r.vflip ? -1 : 1;
      let u02 = u0, v02 = v0, du2 = du, dv2 = dv, blend = 0;
      if (alen) {
        const tid = ls.getAt(i, r.textureIDField)[0] || 0;
        [u0, v0, du, dv] = frameRect(r, tid, alen);
        if (r.softAnim) {
          [u02, v02, du2, dv2] = frameRect(r, tid + 1, alen);
          blend = tid - Math.floor(tid);
        } else { u02 = u0; v02 = v0; du2 = du; dv2 = dv; }
      }
      // stretch axis: velocity modes use Velocity*AxisScale; planar uses the axis fields
      let ax = 0, ay = 0, az = 0, bx = 0, by = 1, bz = 0;
      if (mode === 2 || mode === 3) {
        // Velocity* modes stretch along the particle velocity; other axis-aligned
        // modes read the axis field
        const useVel = /^Velocity/.test(r.modeName || '');
        const src = !useVel && r.axisField && ls.field(r.axisField) ? ls.getAt(i, r.axisField) : ls.getAt(i, 'Velocity');
        ax = (src[0] || 0) * r.axisScale; ay = (src[1] || 0) * r.axisScale; az = (src[2] || 0) * r.axisScale;
      } else if (mode === 4) {
        const a1 = r.axisField && ls.field(r.axisField) ? ls.getAt(i, r.axisField) : [1, 0, 0];
        const a2 = r.axis2Field && ls.field(r.axis2Field) ? ls.getAt(i, r.axis2Field) : [0, 1, 0];
        ax = a1[0] || 0; ay = a1[1] || 0; az = a1[2] || 0;
        bx = a2[0] || 0; by = a2[1] || 0; bz = a2[2] || 0;
        if (!ax && !ay && !az) ax = 1;
        if (!bx && !by && !bz) by = 1;
      }
      let cursor = 0;
      if (r._remap) {
        cursor = r.alphaCursorField && ls.field(r.alphaCursorField)
          ? (ls.getAt(i, r.alphaCursorField)[0] || 0)
          : (ls.getAt(i, 'Age')[0] / (ls.getAt(i, 'Life')[0] || 1));
      }
      inst[o++] = p[0]; inst[o++] = p[1]; inst[o++] = p[2];
      inst[o++] = sz[0] ?? 1; inst[o++] = (sz[1] ?? sz[0] ?? 1) * r.aspect;
      inst[o++] = col[0] ?? 1; inst[o++] = col[1] ?? 1; inst[o++] = col[2] ?? 1; inst[o++] = col[3] ?? 1;
      inst[o++] = rot;
      inst[o++] = u0; inst[o++] = v0; inst[o++] = du; inst[o++] = dv;
      inst[o++] = u02; inst[o++] = v02; inst[o++] = du2; inst[o++] = dv2;
      inst[o++] = blend;
      inst[o++] = ax; inst[o++] = ay; inst[o++] = az;
      inst[o++] = bx; inst[o++] = by; inst[o++] = bz;
      inst[o++] = cursor;
    }
    const count = o / FLOATS_PER_INSTANCE;
    if (!count) return;
    items.push({ type: 'billboard', texture: r._tex, remapTexture: r._remap, kind: r._kind, mode, instances: inst.slice(0, o), count, drawOrder: r.drawOrder });
  }

  function packLight(ls, r, items) {
    const n = ls.count; if (!n) return;
    let o = 0;
    for (let i = 0; i < n; i++) {
      const p = ls.getAt(i, 'Position'); const col = ls.getAt(i, r.colorField);
      if (!isFinite(p[0])) continue;
      const sz = (r.radius || 1) * 0.5;
      inst[o++] = p[0]; inst[o++] = p[1]; inst[o++] = p[2];
      inst[o++] = sz; inst[o++] = sz;
      inst[o++] = col[0] ?? 1; inst[o++] = col[1] ?? 1; inst[o++] = col[2] ?? 1; inst[o++] = Math.min(1, (col[3] ?? 1));
      inst[o++] = 0;
      inst[o++] = 0; inst[o++] = 0; inst[o++] = 1; inst[o++] = 1;
      inst[o++] = 0; inst[o++] = 0; inst[o++] = 1; inst[o++] = 1;
      inst[o++] = 0;
      inst[o++] = 0; inst[o++] = 0; inst[o++] = 0;
      inst[o++] = 0; inst[o++] = 1; inst[o++] = 0;
      inst[o++] = 0;
    }
    const count = o / FLOATS_PER_INSTANCE;
    if (!count) return;
    items.push({ type: 'billboard', texture: glowTex, kind: 1, mode: 0, instances: inst.slice(0, o), count, drawOrder: r.drawOrder });
  }

  // orientation basis for a mesh particle (rotation columns scaled per-axis)
  function meshBasis(ls, i, r, out) {
    // start from axis fields when present, else identity
    let m = IDENT;
    if (r.forwardAxisField && ls.field(r.forwardAxisField)) {
      const f = ls.getAt(i, r.forwardAxisField);
      const up = r.upAxisField && ls.field(r.upAxisField) ? ls.getAt(i, r.upAxisField) : [0, 1, 0];
      m = basisFromForwardUp(f, up);
    } else if (r.upAxisField && ls.field(r.upAxisField)) {
      m = basisFromForwardUp([0, 0, 1], ls.getAt(i, r.upAxisField));
    }
    if (r.eulerRotationField && ls.field(r.eulerRotationField)) {
      m = mat3mulm(m, eulerRad(ls.getAt(i, r.eulerRotationField))); // scripts write radians
    }
    if (r.rotationAxisField && ls.field(r.rotationAxisField)) {
      const axis = ls.getAt(i, r.rotationAxisField);
      const ang = r.rotationAxisAngleField && ls.field(r.rotationAxisAngleField)
        ? (ls.getAt(i, r.rotationAxisAngleField)[0] || 0)
        : (ls.getAt(i, 'Rotation')[0] || 0);
      m = mat3mulm(m, axisAngle(axis, ang));
    }
    if (r.staticOrientation) m = mat3mulm(m, eulerDeg(r.staticOrientation));
    // scale each column
    let sx = r.scale[0], sy = r.scale[1], sz = r.scale[2];
    if (r.scaleField && ls.field(r.scaleField)) {
      const s = ls.getAt(i, r.scaleField);
      const s0 = s[0] ?? 1;
      sx *= s0; sy *= s[1] ?? s0; sz *= s[2] ?? s0;
    }
    out[0] = m[0] * sx; out[1] = m[3] * sx; out[2] = m[6] * sx;
    out[3] = m[1] * sy; out[4] = m[4] * sy; out[5] = m[7] * sy;
    out[6] = m[2] * sz; out[7] = m[5] * sz; out[8] = m[8] * sz;
  }
  const IDENT = [1, 0, 0, 0, 1, 0, 0, 0, 1];
  const BASIS = new Float32Array(9);

  function packMesh(ls, r, items) {
    const n = ls.count; if (!n) return;
    let o = 0;
    for (let i = 0; i < n && o + MESH_FLOATS_PER_INSTANCE <= mbuf.length; i++) {
      const p = ls.getAt(i, r.positionField);
      if (!isFinite(p[0]) || !isFinite(p[1]) || !isFinite(p[2])) continue;
      meshBasis(ls, i, r, BASIS);
      const col = ls.getAt(i, r.colorField);
      let px = p[0], py = p[1], pz = p[2];
      if (r.staticPosition) {
        // offset is in mesh-local space
        px += BASIS[0] * r.staticPosition[0] + BASIS[3] * r.staticPosition[1] + BASIS[6] * r.staticPosition[2];
        py += BASIS[1] * r.staticPosition[0] + BASIS[4] * r.staticPosition[1] + BASIS[7] * r.staticPosition[2];
        pz += BASIS[2] * r.staticPosition[0] + BASIS[5] * r.staticPosition[1] + BASIS[8] * r.staticPosition[2];
      }
      for (let k = 0; k < 9; k++) mbuf[o++] = BASIS[k];
      mbuf[o++] = px; mbuf[o++] = py; mbuf[o++] = pz;
      mbuf[o++] = (col[0] ?? 1) * r.diffuseColor[0]; mbuf[o++] = (col[1] ?? 1) * r.diffuseColor[1];
      mbuf[o++] = (col[2] ?? 1) * r.diffuseColor[2]; mbuf[o++] = col[3] ?? 1;
    }
    const count = o / MESH_FLOATS_PER_INSTANCE;
    if (!count) return;
    items.push({ type: 'mesh', geom: r._geom, texture: r._tex, lit: r._lit, kind: r._kind, instances: mbuf.slice(0, o), count, drawOrder: r.drawOrder });
  }

  function packRibbon(ls, r, eye, items) {
    const n = ls.count; if (n < 2) return;
    // order the layer's particles oldest -> newest (the ribbon follows their path)
    const order = Array.from({ length: n }, (_, i) => i).sort((a, b) => ls.getAt(b, 'Age')[0] - ls.getAt(a, 'Age')[0]);
    const C = order.map((i) => ls.getAt(i, r.positionField));
    let o = 0;
    const push = (p, u, v, c) => { rib[o++] = p[0]; rib[o++] = p[1]; rib[o++] = p[2]; rib[o++] = u; rib[o++] = v; rib[o++] = c[0] ?? 1; rib[o++] = c[1] ?? 1; rib[o++] = c[2] ?? 1; rib[o++] = c[3] ?? 1; };
    const edge = (k) => {
      const i = order[k], c = C[k];
      const prev = C[k - 1] || c, next = C[k + 1] || c;
      const tan = norm(sub(next, prev));
      const toEye = norm(sub(eye, c));
      let side = norm(cross(tan, toEye));
      if (!isFinite(side[0]) || (side[0] === 0 && side[1] === 0 && side[2] === 0)) side = [1, 0, 0];
      const hw = (ls.getAt(i, r.sizeField)[0] || r.width || 0.2) * 0.5;
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
    items.push({ type: 'ribbon', texture: r._tex, kind: r._kind, vertices: rib.slice(0, o), count: o / RIBBON_FLOATS_PER_VERT, drawOrder: r.drawOrder });
  }

  const autofit = { active: true, scale: 0, t: 0 };
  function frame() {
    if (disposed) return;
    tick();
    raf = requestAnimationFrame(frame);
  }
  function tick() {
    const dt = Math.min(0.05, 1 / 60);
    autofit.t += dt;
    // Opening window used to gauge the effect's footprint for the initial framing.
    const measuring = autofit.t < 1.5;

    // The emitter only ever moves when the user drags it (see the controls below) — the
    // effect plays in place, the way it does in the PopcornFX editor. Trails and
    // localspace-attached layers therefore look exactly as authored.
    if (system) {
      try { system.update(dt); }
      catch (e) { if (!system._crashWarned) { system._crashWarned = true; console.warn('pkfx sim error:', e); } }
    }

    const items = [];
    const eye = renderer.eyePosition();
    let sumY = 0, cnt = 0, maxR2 = 0;
    if (system) {
      for (const ls of system.layers) {
        for (let i = 0; i < ls.count; i++) {
          const p = ls.getAt(i, 'Position'); sumY += p[1]; cnt++;
          const r2 = p[0] * p[0] + p[1] * p[1] + p[2] * p[2]; if (r2 > maxR2 && isFinite(r2)) maxR2 = r2;
        }
        for (const r of ls.L.renderers) {
          if (r.kind === 'billboard') packBillboards(ls, r, items);
          else if (r.kind === 'ribbon') packRibbon(ls, r, eye, items);
          else if (r.kind === 'mesh') packMesh(ls, r, items);
          else if (r.kind === 'light') packLight(ls, r, items);
        }
      }
    }
    if (cnt && measuring) autofit.scale = Math.max(autofit.scale, Math.sqrt(maxR2));

    // Camera centres on the particle centroid; auto-distance only until the user interacts.
    if (cnt && autofit.active) {
      renderer.cam.dist += (clamp((autofit.scale || Math.sqrt(maxR2)) * 2.2 + 0.6, 2, 60) - renderer.cam.dist) * 0.1;
      renderer.cam.target[1] += (sumY / cnt - renderer.cam.target[1]) * 0.1;
    }
    renderer.draw(items);
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
  const onWheel = (e) => { e.preventDefault(); autofit.active = false; renderer.cam.dist = clamp(renderer.cam.dist * (1 + Math.sign(e.deltaY) * 0.1), 1, 120); };
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

const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const sub2 = sub;
const add = (a, b) => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
const mul = (a, s) => [a[0] * s, a[1] * s, a[2] * s];
const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const norm = (a) => { const l = Math.hypot(a[0], a[1], a[2]) || 1; return [a[0] / l, a[1] / l, a[2] / l]; };
const clamp = (x, a, b) => Math.min(Math.max(x, a), b);

// ---- small mat3 helpers for mesh orientation (row-major) ----
function eulerDeg(deg) {
  return eulerRad([(deg[0] || 0) * Math.PI / 180, (deg[1] || 0) * Math.PI / 180, (deg[2] || 0) * Math.PI / 180]);
}
function eulerRad(v) {
  const r = [v[0] || 0, v[1] || 0, v[2] || 0];
  const cx = Math.cos(r[0]), sx = Math.sin(r[0]);
  const cy = Math.cos(r[1]), sy = Math.sin(r[1]);
  const cz = Math.cos(r[2]), sz = Math.sin(r[2]);
  return [
    cy * cz, -cy * sz, sy,
    sx * sy * cz + cx * sz, -sx * sy * sz + cx * cz, -sx * cy,
    -cx * sy * cz + sx * sz, cx * sy * sz + sx * cz, cx * cy,
  ];
}
function axisAngle(axis, ang) {
  const l = Math.hypot(axis[0] || 0, axis[1] || 0, axis[2] || 0) || 1;
  const x = (axis[0] || 0) / l, y = (axis[1] || 0) / l, z = (axis[2] || 0) / l;
  const c = Math.cos(ang), s = Math.sin(ang), t = 1 - c;
  return [
    t * x * x + c, t * x * y - s * z, t * x * z + s * y,
    t * x * y + s * z, t * y * y + c, t * y * z - s * x,
    t * x * z - s * y, t * y * z + s * x, t * z * z + c,
  ];
}
function basisFromForwardUp(fwd, up) {
  let f = norm([fwd[0] || 0, fwd[1] || 0, fwd[2] || 1]);
  let r = cross([up[0] || 0, up[1] || 1, up[2] || 0], f);
  const rl = Math.hypot(r[0], r[1], r[2]);
  r = rl > 1e-5 ? [r[0] / rl, r[1] / rl, r[2] / rl] : [1, 0, 0];
  const u = cross(f, r);
  // columns: X=right, Y=up, Z=forward (row-major rows)
  return [r[0], u[0], f[0], r[1], u[1], f[1], r[2], u[2], f[2]];
}
function mat3mulm(a, b) {
  const o = new Array(9);
  for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) {
    o[i * 3 + j] = a[i * 3] * b[j] + a[i * 3 + 1] * b[3 + j] + a[i * 3 + 2] * b[6 + j];
  }
  return o;
}

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

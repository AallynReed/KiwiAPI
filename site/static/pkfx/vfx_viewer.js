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
import { AnimTrackSampler } from './curves.js';
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

/* Draw order within one billboard batch. Particles composite in the order they are
   written, so an alpha-blended layer has to run back-to-front or nearer particles
   wrongly occlude the ones behind them; additive blending is commutative and needs
   no sort at all. Returns null to keep simulation order. The scratch buffers are
   module-level because this runs per layer per frame over every live particle.
   Exported for tests. */
let sortIdx = new Int32Array(0), sortKey = new Float32Array(0);
export function billboardOrder(ls, r, n, eye) {
  if (r._kind === 1 || r._kind === 3) return null;            // additive: order-free
  // Every field-sort in the corpus keys on LifeRatio, which is virtual (Age/Life)
  // rather than a stored field, so resolve it the way the script context does. A
  // field we cannot resolve falls back to camera distance, not to no sort at all.
  let field = /^Field/.test(r.sortMode) ? r.sortField : null;
  if (field && field !== 'LifeRatio' && !ls.field(field)) field = null;
  if (!field && r.sortMode !== 'CameraDistance' && !/^Field/.test(r.sortMode)) return null;
  if (sortIdx.length < n) { sortIdx = new Int32Array(n); sortKey = new Float32Array(n); }
  for (let i = 0; i < n; i++) {
    sortIdx[i] = i;
    if (field === 'LifeRatio') sortKey[i] = ls.getAt(i, 'Age')[0] / (ls.getAt(i, 'Life')[0] || 1);
    else if (field) sortKey[i] = ls.getAt(i, field)[0] || 0;
    else {
      const p = ls.getAt(i, r.positionField);
      const dx = p[0] - eye[0], dy = p[1] - eye[1], dz = p[2] - eye[2];
      sortKey[i] = dx * dx + dy * dy + dz * dz;
    }
  }
  const order = sortIdx.subarray(0, n);
  // camera distance draws farthest first; an explicit field sort follows its name
  if (field && r.sortMode === 'FieldAscending') order.sort((a, b) => sortKey[a] - sortKey[b]);
  else order.sort((a, b) => sortKey[b] - sortKey[a]);
  return order;
}

// the engine stores vertex colours as RGBA8, so every channel saturates at 0..1
const sat = (x) => (x > 1 ? 1 : x < 0 ? 0 : x);

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

  let renderer, system, current, raf = 0, disposed = false;
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
      // each rect is sorted to [umin, vmin, umax, vmax]: a reversed rect does not flip
      const rects = txt.trim().split(/\r?\n/).map((l) => {
        const [a, b, c, d] = l.split(',').map((n) => parseFloat(n.trim()));
        return [Math.min(a, c), Math.min(b, d), Math.max(a, c), Math.max(b, d)];
      });
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
    const doc = parsePkfx(man.pkfx);
    const effect = buildEffect(doc, Math.random);

    const unsupported = new Set();
    for (const layer of effect.layers) {
      for (const r of layer.renderers) {
        if (r.kind === 'billboard') {
          // distortion only offsets the scene behind it; it writes no colour of its own
          if (/Distortion/i.test(r.material)) { r._skip = true; continue; }
          r._tex = await loadTexture(r.diffuse);
          r._atlas = await loadAtlas(r.atlas);
          r._remap = r.alphaRemap ? await loadTexture(r.alphaRemap) : null;
          r._kind = kindFor(r.material);
          // a _Soft material fades where it meets the ground; anything else is hard-edged
          r._soft = /_Soft/i.test(r.material) ? Math.max(r.softness, 1e-3) : 0;
        } else if (r.kind === 'ribbon') {
          r._tex = await loadTexture(r.diffuse);
          r._atlas = await loadAtlas(r.atlas);
          r._remap = r.alphaRemap ? await loadTexture(r.alphaRemap) : null;
          r._kind = kindFor(r.material);
          r._soft = /_Soft/i.test(r.material) ? Math.max(r.softness, 1e-3) : 0;
        } else if (r.kind === 'mesh') {
          r._geom = await loadMesh(r.mesh);
          r._tex = await loadTexture(r.diffuse);
          r._lit = !/Additive/i.test(r.material);
          r._kind = /Additive_NoAlpha/i.test(r.material) ? 3 : /Additive/i.test(r.material) ? 1 : 0;
        } else if (r.cls) unsupported.add(r.cls.replace('CParticleRenderer_', ''));
      }
    }
    await loadAnimTracks(effect);
    if (disposed) return;
    system = new System(effect, Math.random);
    current = { effect, unsupported: [...unsupported], missing: man.missing || [] };

    const partials = [];
    if (current.missing.length) partials.push(`${current.missing.length} asset(s) missing`);
    if (current.unsupported.length) partials.push(`${current.unsupported.join('/')} not shown`);
    if (!man.game_available && current.missing.length) partials.push('game assets unavailable');
    note.textContent = partials.length ? 'Partial preview — ' + partials.join(' · ') : '';
    note.style.display = partials.length ? 'block' : 'none';

    autofit.active = true; autofit.scale = 0; autofit.t = 0; autofit.floor = null;
    loading.style.display = 'none';
    raf = requestAnimationFrame(frame);
  }

  const inst = new Float32Array(20000 * FLOATS_PER_INSTANCE);
  const rib = new Float32Array(60000 * RIBBON_FLOATS_PER_VERT);
  const mbuf = new Float32Array(4000 * MESH_FLOATS_PER_INSTANCE);

  // TextureID -> atlas frame, clamped the way CBillboarder::FillTexcoordsFromAtlas does
  function atlasFrame(tid, alen) {
    let f = Math.abs(tid); if (!isFinite(f)) f = 0;
    return Math.min(f, alen - 1);
  }
  function frameRect(r, f, alen) {
    // atlas rects are [u0,v0,u1,v1]; VFlipUVs mirrors v
    const rc = r._atlas[Math.min(f | 0, alen - 1)];
    if (r.vflip) return [rc[0], rc[3], rc[2] - rc[0], rc[1] - rc[3]];
    return [rc[0], rc[1], rc[2] - rc[0], rc[3] - rc[1]];
  }

  function packBillboards(ls, r, eye, items) {
    const n = ls.count; if (!n) return;
    let o = 0; const alen = r._atlas ? r._atlas.length : 0;
    const mode = r.mode;
    const order = billboardOrder(ls, r, n, eye);
    for (let k = 0; k < n; k++) {
      const i = order ? order[k] : k;
      const p = ls.getAt(i, r.positionField);
      if (!isFinite(p[0]) || !isFinite(p[1]) || !isFinite(p[2])) continue;
      const sz = r.constantRadius > 0 ? [r.constantRadius, r.constantRadius] : ls.getAt(i, r.sizeField);
      const col = ls.getAt(i, r.colorField);
      const rot = ls.getAt(i, r.rotationField)[0] || 0;
      let u0 = 0, v0 = r.vflip ? 1 : 0, du = 1, dv = r.vflip ? -1 : 1;
      let u02 = u0, v02 = v0, du2 = du, dv2 = dv, blend = 0;
      if (alen) {
        const t = atlasFrame(ls.getAt(i, r.textureIDField)[0] || 0, alen), fa = t | 0;
        [u0, v0, du, dv] = frameRect(r, fa, alen);
        if (r.softAnim) {
          [u02, v02, du2, dv2] = frameRect(r, Math.min(fa + 1, alen - 1), alen);
          blend = t - fa;
        } else { u02 = u0; v02 = v0; du2 = du; dv2 = dv; }
      }
      // stretch axis: the axis-aligned modes stretch along AxisField, planar uses both axis fields
      let ax = 0, ay = 0, az = 0, bx = 0, by = 1, bz = 0, sxScale = 1;
      if (mode === 2 || mode === 3) {
        /* BillboardMode picks the billboarder; AxisField picks the data it stretches along,
           and Velocity is only its default. Forcing Velocity whenever the mode name started
           with "Velocity" threw away 2,307 authored AxisFields - and those layers usually have
           no velocity at all, so the axis came out zero. */
        const src = r.axisField && ls.field(r.axisField) ? ls.getAt(i, r.axisField) : ls.getAt(i, 'Velocity');
        ax = (src[0] || 0) * r.axisScale; ay = (src[1] || 0) * r.axisScale; az = (src[2] || 0) * r.axisScale;
      } else if (mode === 4) {
        const a1 = r.axisField && ls.field(r.axisField) ? ls.getAt(i, r.axisField) : [1, 0, 0];
        const a2 = r.axis2Field && ls.field(r.axis2Field) ? ls.getAt(i, r.axis2Field) : [0, 1, 0];
        ax = a1[0] || 0; ay = a1[1] || 0; az = a1[2] || 0;
        bx = a2[0] || 0; by = a2[1] || 0; bz = a2[2] || 0;
        if (!ax && !ay && !az) ax = 1;
        if (!bx && !by && !bz) by = 1;
        /* The planar worker scales its X basis by 0.5*AxisScale and its Y by a flat 0.5
           (billboarding request +0xac/+0xb0; AxisScale is the property at renderer+0x150), so
           AxisScale is the quad's width-to-height ratio here, not a stretch of the axis vector
           the way the velocity modes use it. 459 corpus effects set it, the portal ring at 0.5.
           X is the cross-axis direction - see the mode 4 branch in renderer.js. */
        sxScale = r.axisScale;
      }
      let cursor = 0;
      if (r._remap) {
        cursor = r.alphaCursorField && ls.field(r.alphaCursorField)
          ? (ls.getAt(i, r.alphaCursorField)[0] || 0)
          : (ls.getAt(i, 'Age')[0] / (ls.getAt(i, 'Life')[0] || 1));
      }
      inst[o++] = p[0]; inst[o++] = p[1]; inst[o++] = p[2];
      // AspectRatio only shapes screen-aligned quads: a <= 1 narrows X, a > 1 shortens Y
      let ar = 1, br = 1;
      if (mode === 0) { const a = Math.max(r.aspect, 0); if (a <= 1) ar = a; else br = 1 / a; }
      inst[o++] = (sz[0] ?? 1) * sxScale * ar; inst[o++] = (sz[1] ?? sz[0] ?? 1) * br;
      /* Saturate to 0..1. CBillboarder::FillColors packs the vertex colour to RGBA8
         with a saturating byte pack, so the engine can never see a channel above 1.
         Scripts and curves routinely produce more - the portals carry alpha 1.26 and
         red 1.44 - and passing that through float attributes multiplied every texel's
         alpha by 1.26, turning soft sprites into hard discs. */
      inst[o++] = sat(col[0] ?? 1); inst[o++] = sat(col[1] ?? 1); inst[o++] = sat(col[2] ?? 1); inst[o++] = sat(col[3] ?? 1);
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
    items.push({ type: 'billboard', texture: r._tex, remapTexture: r._remap, kind: r._kind, mode, instances: inst.slice(0, o), count, drawOrder: r.drawOrder, soft: r._soft, dissolve: r.dissolve });
  }

  /* Mesh orientation, composed as SMatrixBuilder::BuildWorldMatrix does:
     world = Forward * AxisAngle * Euler * StaticOrientation * scale, the static position
     offset rotated but not scaled. Writes the scaled basis to `out`, the unscaled
     rotation to `rot`. */
  function meshBasis(ls, i, r, out, rot) {
    let m = IDENT;
    if (r.forwardAxisField && ls.field(r.forwardAxisField)) {
      const f = ls.getAt(i, r.forwardAxisField);
      const up = r.upAxisField && ls.field(r.upAxisField) ? ls.getAt(i, r.upAxisField) : [0, 1, 0];
      m = basisFromForwardUp(f, up);
    }
    const axis = r.rotationAxisField && ls.field(r.rotationAxisField) ? ls.getAt(i, r.rotationAxisField) : r.staticRotationAxis;
    if (axis && (axis[0] || axis[1] || axis[2])) {
      const ang = r.rotationAxisAngleField && ls.field(r.rotationAxisAngleField)
        ? (ls.getAt(i, r.rotationAxisAngleField)[0] || 0)
        : (ls.getAt(i, 'Rotation')[0] || 0);
      m = mat3mulm(m, axisAngle(axis, ang));
    }
    if (r.eulerRotationField && ls.field(r.eulerRotationField)) {
      m = mat3mulm(m, eulerRad(ls.getAt(i, r.eulerRotationField))); // scripts write radians
    }
    if (r.staticOrientation) m = mat3mulm(m, eulerDeg(r.staticOrientation));
    for (let k = 0; k < 9; k++) rot[k] = m[k];
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
  const BASIS = new Float32Array(9), ROT = new Float32Array(9);
  const WHITE = [1, 1, 1, 1];

  function packMesh(ls, r, items) {
    const n = ls.count; if (!n) return;
    let o = 0;
    for (let i = 0; i < n && o + MESH_FLOATS_PER_INSTANCE <= mbuf.length; i++) {
      const p = ls.getAt(i, r.positionField);
      if (!isFinite(p[0]) || !isFinite(p[1]) || !isFinite(p[2])) continue;
      meshBasis(ls, i, r, BASIS, ROT);
      const col = r.colorField && ls.field(r.colorField) ? ls.getAt(i, r.colorField) : WHITE;
      let px = p[0], py = p[1], pz = p[2];
      if (r.staticPosition) {
        const [a, b, c] = r.staticPosition;
        px += ROT[0] * a + ROT[1] * b + ROT[2] * c;
        py += ROT[3] * a + ROT[4] * b + ROT[5] * c;
        pz += ROT[6] * a + ROT[7] * b + ROT[8] * c;
      }
      for (let k = 0; k < 9; k++) mbuf[o++] = BASIS[k];
      mbuf[o++] = px; mbuf[o++] = py; mbuf[o++] = pz;
      mbuf[o++] = (col[0] ?? 1) * r.diffuseColor[0]; mbuf[o++] = (col[1] ?? 1) * r.diffuseColor[1];
      mbuf[o++] = (col[2] ?? 1) * r.diffuseColor[2]; mbuf[o++] = sat(col[3] ?? 1);
    }
    const count = o / MESH_FLOATS_PER_INSTANCE;
    if (!count) return;
    items.push({ type: 'mesh', geom: r._geom, texture: r._tex, lit: r._lit, kind: r._kind, instances: mbuf.slice(0, o), count, drawOrder: r.drawOrder });
  }

  /* Ribbons (CRibbonBillboarder): one strip per spawner instance / parent particle,
     linked newest first. The width is a half width. Without a TextureUField the texture
     tiles once per segment; with one, U is that field's raw value. */
  const RIB_CORNER = [[0, 1], [1, 1], [1, 0], [0, 0]];
  const RIB_ROWS = [[3, 0, 2, 1], [2, 1, 3, 0], [0, 3, 1, 2], [1, 2, 0, 3], [0, 1, 3, 2], [3, 2, 0, 1], [1, 0, 2, 3], [2, 3, 1, 0]];
  function packRibbon(ls, r, eye, items) {
    const n = ls.count; if (n < 2) return;
    const groups = new Map();
    for (let i = 0; i < n; i++) {
      const g = ls.getAt(i, '__grp')[0];
      let list = groups.get(g); if (!list) groups.set(g, list = []);
      list.push(i);
    }
    const alen = r._atlas ? r._atlas.length : 0;
    const row = RIB_ROWS[(r.flipU ? 1 : 0) + (r.flipV ? 2 : 0) + (r.rotateTexture ? 4 : 0)];
    const lifeRatio = (i) => ls.getAt(i, 'Age')[0] / (ls.getAt(i, 'Life')[0] || 1);
    const readU = r.textureUField === 'LifeRatio' ? lifeRatio
      : r.textureUField && ls.field(r.textureUField) ? (i) => ls.getAt(i, r.textureUField)[0] : null;
    const axisOk = r.axisField && ls.field(r.axisField);
    let o = 0;
    const cap = rib.length - 6 * RIBBON_FLOATS_PER_VERT;
    const push = (p, u, v, c, cur, rc) => {
      if (rc) { u = rc[0] + u * (rc[2] - rc[0]); v = rc[1] + v * (rc[3] - rc[1]); }
      rib[o++] = p[0]; rib[o++] = p[1]; rib[o++] = p[2]; rib[o++] = u; rib[o++] = v;
      rib[o++] = sat(c[0] ?? 1); rib[o++] = sat(c[1] ?? 1); rib[o++] = sat(c[2] ?? 1); rib[o++] = sat(c[3] ?? 1);
      rib[o++] = cur;
    };
    for (const list of groups.values()) {
      if (list.length < 2) continue;
      list.sort((a, b) => ls.getAt(b, '__sid')[0] - ls.getAt(a, '__sid')[0]);
      const C = list.map((i) => ls.getAt(i, r.positionField));
      const E = list.map((i, k) => {
        const c = C[k], tan = sub(C[k + 1] || c, C[k - 1] || c);
        const w = r.widthField ? (ls.getAt(i, r.widthField)[0] || 0) : r.width;
        let side;
        if (r.mode === 'SideAxisAligned' && axisOk) side = norm(ls.getAt(i, r.axisField));
        else if (r.mode === 'NormalAxisAligned' && axisOk) side = norm(cross(tan, ls.getAt(i, r.axisField)));
        else side = norm(cross(sub(c, eye), tan));
        if (!isFinite(side[0])) side = [1, 0, 0];
        const cur = r._remap
          ? (r.alphaCursorField && ls.field(r.alphaCursorField) ? ls.getAt(i, r.alphaCursorField)[0] : lifeRatio(i))
          : 0;
        let rc = null;
        if (alen) {
          const tid = r.textureIDField && ls.field(r.textureIDField) ? ls.getAt(i, r.textureIDField)[0] : r.textureID;
          rc = r._atlas[atlasFrame(tid || 0, alen) | 0];
        }
        return { P: add(c, mul(side, w)), M: sub(c, mul(side, w)), col: ls.getAt(i, r.colorField), cur, rc, t: readU ? readU(i) : 0 };
      });
      for (let k = 0; k + 1 < E.length && o < cap; k++) {
        const a = E[k], b = E[k + 1];
        let uv;
        if (readU) {
          // [a+, a-, b+, b-]
          const f = (t) => (r.flipU ? 1 - t : t);
          const v0 = r.flipV ? 1 : 0, v1 = 1 - v0;
          uv = r.rotateTexture
            ? [[0, 1 - a.t], [1, 1 - a.t], [0, 1 - b.t], [1, 1 - b.t]]
            : [[f(a.t), v0], [f(a.t), v1], [f(b.t), v0], [f(b.t), v1]];
        } else uv = row.map((j) => RIB_CORNER[j]);
        push(a.P, uv[0][0], uv[0][1], a.col, a.cur, a.rc); push(a.M, uv[1][0], uv[1][1], a.col, a.cur, a.rc); push(b.P, uv[2][0], uv[2][1], b.col, b.cur, a.rc);
        push(a.M, uv[1][0], uv[1][1], a.col, a.cur, a.rc); push(b.M, uv[3][0], uv[3][1], b.col, b.cur, a.rc); push(b.P, uv[2][0], uv[2][1], b.col, b.cur, a.rc);
      }
    }
    if (!o) return;
    items.push({ type: 'ribbon', texture: r._tex, remapTexture: r._remap, kind: r._kind, soft: r._soft, repeat: r.repeat, vertices: rib.slice(0, o), count: o / RIBBON_FLOATS_PER_VERT, drawOrder: r.drawOrder });
  }

  const autofit = { active: true, scale: 0, t: 0, floor: null };
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
      system.camPos = renderer.eyePosition();
      try { system.update(dt); }
      catch (e) { if (!system._crashWarned) { system._crashWarned = true; console.warn('pkfx sim error:', e); } }
    }

    const items = [];
    const eye = renderer.eyePosition();
    let sumY = 0, cnt = 0, maxR2 = 0, minY = Infinity;
    if (system) {
      for (const ls of system.layers) {
        for (let i = 0; i < ls.count; i++) {
          const p = ls.getAt(i, 'Position'); sumY += p[1]; cnt++;
          if (p[1] < minY && isFinite(p[1])) minY = p[1];
          const r2 = p[0] * p[0] + p[1] * p[1] + p[2] * p[2]; if (r2 > maxR2 && isFinite(r2)) maxR2 = r2;
        }
        for (const r of ls.L.renderers) {
          if (r._skip) continue;
          if (r.kind === 'billboard') packBillboards(ls, r, eye, items);
          else if (r.kind === 'ribbon') packRibbon(ls, r, eye, items);
          else if (r.kind === 'mesh') packMesh(ls, r, items);
        }
      }
    }
    if (cnt && measuring) autofit.scale = Math.max(autofit.scale, Math.sqrt(maxR2));

    /* No ground. It was added to give soft particles something to fade against, but
       an effect is authored to be seen against the world, not against a slab we
       invented - side by side with the game the plane read as a hard-edged wedge cut
       through the portal. Soft particles keep working; with nothing opaque in the
       depth pass the fade is simply a no-op, which is the honest result for a preview
       that has no scene. `autofit.floor` stays measured in case a real backdrop is
       ever added. */
    if (cnt && measuring) autofit.floor = Math.min(autofit.floor ?? Infinity, minY);

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
// engine Euler builder (FUN_180609e80): yaw about Y first, then X, then Z: Rz * Rx * Ry
function eulerRad(v) {
  const x = v[0] || 0, y = v[1] || 0, z = v[2] || 0;
  const Ry = [Math.cos(y), 0, Math.sin(y), 0, 1, 0, -Math.sin(y), 0, Math.cos(y)];
  const Rx = [1, 0, 0, 0, Math.cos(x), -Math.sin(x), 0, Math.sin(x), Math.cos(x)];
  const Rz = [Math.cos(z), -Math.sin(z), 0, Math.sin(z), Math.cos(z), 0, 0, 0, 1];
  return mat3mulm(mat3mulm(Rz, Rx), Ry);
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

if (typeof window !== 'undefined') window.PkfxViewer = { open, mount };

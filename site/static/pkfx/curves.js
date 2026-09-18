// Samplers referenced by particle scripts and Field evolvers:
//  - CParticleSamplerCurve                keyframed curve (Linear or cubic-Hermite), 1..4 components
//  - CParticleSamplerDoubleCurve          two curves blended by a 0..1 selector (.sample(t, sel))
//  - CParticleSamplerShape                emission shape (.samplePosition()/.sampleNormal())
//  - CParticleSamplerProceduralTurbulence animated vector-noise velocity field (.sampleCurl(pos))
//  - CParticleSamplerAnimTrack            baked motion path (.samplePosition(cursor)) — a .pkan
import { deref, toNums, toSym } from './parser.js';
import { TurbulenceRef, TURB_DEFAULTS } from './turbulence.js';

const COMP = { Float: 1, Float2: 2, Float3: 3, Float4: 4 };

function compCount(valueType, times, values) {
  const s = toSym(valueType);
  if (s && COMP[s]) return COMP[s];
  if (times && times.length) return Math.max(1, Math.round(values.length / times.length));
  return 1;
}

class Curve {
  constructor(times, values, tangents, comp, linear) {
    this.t = times; this.v = values; this.tan = tangents; this.c = comp; this.linear = linear || !tangents || tangents.length === 0;
  }
  sample(tRaw) {
    const t = this.t; const n = t.length; const c = this.c;
    if (n === 0) return new Array(c).fill(0);
    let x = tRaw; if (x <= t[0]) return this.key(0);
    if (x >= t[n - 1]) return this.key(n - 1);
    let i = 0; while (i < n - 1 && t[i + 1] < x) i++;
    const t0 = t[i], t1 = t[i + 1], dt = (t1 - t0) || 1e-9; const u = (x - t0) / dt;
    const p0 = this.key(i), p1 = this.key(i + 1);
    if (this.linear) { const o = new Array(c); for (let k = 0; k < c; k++) o[k] = p0[k] + (p1[k] - p0[k]) * u; return o; }
    const u2 = u * u, u3 = u2 * u;
    const h00 = 2 * u3 - 3 * u2 + 1, h10 = u3 - 2 * u2 + u, h01 = -2 * u3 + 3 * u2, h11 = u3 - u2;
    const o = new Array(c);
    for (let k = 0; k < c; k++) {
      const mOut = this.tanAt(i, k, 1), mIn = this.tanAt(i + 1, k, 0);
      o[k] = h00 * p0[k] + h10 * mOut + h01 * p1[k] + h11 * mIn;   // tangents come pre-scaled to their segment
    }
    return o;
  }
  key(i) { const c = this.c; const o = new Array(c); for (let k = 0; k < c; k++) o[k] = this.v[i * c + k] ?? 0; return o; }
  // tangents stored per key as [in vector, out vector]; which: 0=in, 1=out
  tanAt(i, k, which) { return this.tan[(2 * i + which) * this.c + k] ?? 0; }
}

export class CurveSampler {
  constructor(obj) {
    const times = toNums(obj.props.Times) || [];
    const values = toNums(obj.props.FloatValues) || [];
    const tangents = toNums(obj.props.FloatTangents) || [];
    const comp = compCount(obj.props.ValueType, times, values);
    const linear = toSym(obj.props.Interpolator) === 'Linear';
    this.curve = new Curve(times, values, tangents, comp, linear);
    this.comp = comp;
    // MinLimits/MaxLimits clamp the sampled value (per component; ±Infinity = unbounded)
    this.min = toNums(obj.props.MinLimits) || null;
    this.max = toNums(obj.props.MaxLimits) || null;
  }
  sample(t) {
    return clampLimits(this.curve.sample(t == null ? 0 : t[0] ?? t), this.min, this.max);
  }
  // Spawn-flux helpers on the first component, unclamped. The integral only covers the
  // key range (zero outside it); Simpson per segment is exact for the cubic pieces.
  integral(a, b) {
    const t = this.curve.t, n = t.length;
    if (n < 2) return n ? this.curve.key(0)[0] * Math.max(0, b - a) : 0;
    const lo = Math.max(a, t[0]), hi = Math.min(b, t[n - 1]);
    if (!(hi > lo)) return 0;
    const f = (x) => this.curve.sample(x)[0];
    let sum = 0;
    for (let i = 0; i < n - 1; i++) {
      const x0 = Math.max(lo, t[i]), x1 = Math.min(hi, t[i + 1]);
      if (x1 > x0) sum += (x1 - x0) / 6 * (f(x0) + 4 * f((x0 + x1) / 2) + f(x1));
    }
    return sum;
  }
  // sum of key values whose time lies in [a, b], both ends inclusive
  keySum(a, b) {
    const t = this.curve.t; let s = 0;
    for (let i = 0; i < t.length; i++) if (t[i] >= a && t[i] <= b) s += this.curve.key(i)[0];
    return s;
  }
}

export class DoubleCurveSampler {
  constructor(obj) {
    const p = obj.props;
    const t0 = toNums(p.Times) || [], v0 = toNums(p.FloatValues) || [], g0 = toNums(p.FloatTangents) || [];
    const t1 = toNums(p.Times1) || [], v1 = toNums(p.FloatValues1) || [], g1 = toNums(p.FloatTangents1) || [];
    const comp = compCount(p.ValueType, t0, v0);
    const linear = toSym(p.Interpolator) === 'Linear';
    this.c0 = new Curve(t0, v0, g0, comp, linear);
    this.c1 = new Curve(t1, v1, g1, comp, linear);
    this.comp = comp;
    // curve0's limits clamp BOTH curves before the blend
    this.min = toNums(p.MinLimits) || null;
    this.max = toNums(p.MaxLimits) || null;
  }
  sample(t, sel) {
    const x = t == null ? 0 : (t[0] ?? t); const s = sel == null ? 0 : (sel[0] ?? sel);
    const a = clampLimits(this.c0.sample(x), this.min, this.max), b = clampLimits(this.c1.sample(x), this.min, this.max);
    const o = new Array(this.comp); for (let k = 0; k < this.comp; k++) o[k] = a[k] + (b[k] - a[k]) * s;
    return o;
  }
}

function clampLimits(o, min, max) {
  if (min) for (let k = 0; k < o.length; k++) { const m = min[k]; if (isFinite(m) && o[k] < m) o[k] = m; }
  if (max) for (let k = 0; k < o.length; k++) { const m = max[k]; if (isFinite(m) && o[k] > m) o[k] = m; }
  return o;
}

// A baked motion path (CParticleSamplerAnimTrack): `AnimResource` names a mesh, but the
// animation lives in the sibling `.pkan`, a text HBO file:
//   CAnimationClip { EntityStreams -> CAnimationTrack { Channels -> CSamplerCurve } }
// with one curve per BindingSemantic (Translation / Rotation as euler degrees / Scale).
// The resource arrives over the network: construction records the path, the viewer
// parses the .pkan and calls load(); until then samplePosition returns the placement.
//
// samplePosition(u): time = t0 + u*(t1 - t0) over the track's key range (the curves
// clamp at the ends), point = T(t) + R(t)*(S(t) * 0), then the sampler's own placement
// Position + Euler * (Scale * point); TransformTranslate/Rotate/Scale switch those
// stages off.
export class AnimTrackSampler {
  constructor(obj) {
    const p = obj.props;
    this.resource = typeof p.AnimResource === 'string' ? p.AnimResource : null;
    this.trackIndex = Math.max(0, Math.trunc(num(p.AnimTrackIndex, 0)));
    this.mode = (p.TransformTranslate !== false ? 1 : 0) | (p.TransformRotate !== false ? 2 : 0) | (p.TransformScale !== false ? 4 : 0);
    const eul = toNums(p.EulerOrientation) || [0, 0, 0];
    this.locT = toNums(p.Position) || [0, 0, 0];
    this.locR = eul[0] || eul[1] || eul[2] ? eulerMatrix(eul) : null;
    this.locS = toNums(p.Scale) || [1, 1, 1];
    this.tracks = [];
  }
  resourceRef() {
    return this.resource ? this.resource.replace(/\.[^.\\/]+$/, '.pkan') : null;
  }
  load(doc) {
    let clip = null;
    for (const id of doc.order) if (doc.objects[id].className === 'CAnimationClip') { clip = doc.objects[id]; break; }
    if (!clip) return false;
    this.tracks = [];
    for (const tref of clip.props.EntityStreams || []) {
      const track = deref(doc, tref);
      if (!track) continue;
      const slot = {};
      for (const cref of track.props.Channels || []) {
        const ch = deref(doc, cref);
        if (!ch) continue;
        const sem = ch.props.BindingSemantic;
        if (sem !== 'Translation' && sem !== 'Rotation' && sem !== 'Scale') continue;
        const times = toNums(ch.props.Times) || [];
        const c = new Curve(times, toNums(ch.props.FloatValues) || [], toNums(ch.props.FloatTangents) || [], 3, toSym(ch.props.Interpolator) === 'Linear');
        c.tMin = times.length ? Math.min(...times) : 0;
        c.tMax = times.length ? Math.max(...times) : 1;
        c.ok = times.length > 1;
        slot[sem] = c;
      }
      const cs = [slot.Translation, slot.Rotation, slot.Scale].filter(Boolean);
      if (!cs.length) continue;
      this.tracks.push({ T: slot.Translation, R: slot.Rotation, S: slot.Scale,
        t0: Math.min(...cs.map((c) => c.tMin)), t1: Math.max(...cs.map((c) => c.tMax)) });
    }
    return this.tracks.length > 0;
  }
  _transform(cursor, p0, mode) {
    if (!mode) return p0.slice();
    let p = p0.slice();
    const tr = this.tracks.length ? this.tracks[Math.min(this.trackIndex, this.tracks.length - 1)] : null;
    if (tr) {
      const u = cursor == null ? 0 : (cursor[0] ?? cursor);
      const span = tr.t1 - tr.t0;
      const t = Number.isFinite(span) && span !== 0 ? tr.t0 + span * u : u;
      if (mode & 4 && tr.S && tr.S.ok) { const sc = tr.S.sample(t); p = [p[0] * sc[0], p[1] * sc[1], p[2] * sc[2]]; }
      if (mode & 2 && tr.R && tr.R.ok) p = mat3mul(eulerMatrix(tr.R.sample(t)), p);
      if (mode & 1 && tr.T && tr.T.ok) { const d = tr.T.sample(t); p = [p[0] + d[0], p[1] + d[1], p[2] + d[2]]; }
    }
    if (mode & 4) p = [p[0] * this.locS[0], p[1] * this.locS[1], p[2] * this.locS[2]];
    if (mode & 2 && this.locR) p = mat3mul(this.locR, p);
    if (mode & 1) p = [p[0] + this.locT[0], p[1] + this.locT[1], p[2] + this.locT[2]];
    return p;
  }
  samplePosition(cursor) { return this._transform(cursor, [0, 0, 0], this.mode); }
  // the rotation-only path the axis methods take
  axisSide(c) { return this._transform(c, [1, 0, 0], this.mode & 2); }
  axisUp(c) { return this._transform(c, [0, 1, 0], this.mode & 2); }
  axisForward(c) { return this._transform(c, [0, 0, 1], this.mode & 2); }
  sample(cursor) { return this.samplePosition(cursor); }
}

// Emission shape (CShapeDescriptor_*). Every sample is driven by three uniforms, which
// is also what samplePCoords() hands out, so samplePosition(pc)/sampleNormal(pc) agree
// on one point while plain samplePosition()/sampleNormal() are independent draws.
// Branches (inner shell, capsule cap, box face) rescale the uniform they consume.
// Defaults: BOX of 0.5 per side, Radius 1, InnerRadius 0, Height 0. EulerOrientation
// is degrees applied X, then Y, then Z; Hemisphere/NonUniformScale are ellipsoid-only.
export class ShapeSampler {
  constructor(shape, rng, doc) {
    this.rng = rng;
    const p = shape ? shape.props : {};
    // A CShapeDescriptorCollection emits from its sub-shapes, picked by Weight.
    this.subs = shape && shape.className === 'CShapeDescriptorCollection'
      ? (p.SubShapes || []).map((ref) => new ShapeSampler(deref(doc, ref), rng, doc))
      : null;
    if (this.subs) {
      this.weights = (p.SubShapes || []).map((ref) => Math.max(0, num(deref(doc, ref)?.props?.Weight, 1)));
      this.wsum = this.weights.reduce((x, y) => x + y, 0);
    }
    this.type = toSym(p.ShapeType) || 'BOX';
    this.dim = toNums(p.BoxDimensions) || [0.5, 0.5, 0.5];
    let R = num(p.Radius, 1), Ri = num(p.InnerRadius, 0);
    if (R < Ri) [R, Ri] = [Ri, R];
    this.radius = R; this.inner = Ri;
    this.height = num(p.Height, 0);
    this.pos = toNums(p.Position) || [0, 0, 0];
    this.scale = this.type === 'COMPLEX_ELLIPSOID' ? toNums(p.NonUniformScale) || null : null;
    this.hemisphere = this.type === 'COMPLEX_ELLIPSOID' && p.Hemisphere === true;
    const euler = toNums(p.EulerOrientation);
    this.rot = euler && (euler[0] || euler[1] || euler[2]) ? eulerMatrix(euler) : null;
    // set per sampler (CParticleSamplerShape props)
    this.volume = false;
    this.translate = true;
    this.rotate = true;
  }
  _u() { const r = this.rng; return [r(), r(), r()]; }
  _pick() {
    let x = this.rng() * this.wsum;
    for (let k = 0; k < this.subs.length; k++) { x -= this.weights[k]; if (x < 0) return this.subs[k]; }
    return this.subs[this.subs.length - 1];
  }
  // local-space {p, n} for uniforms u, then the shape transform
  _eval(u) {
    if (this.subs) {
      if (!this.subs.length || !(this.wsum > 0)) return { p: this.pos.slice(), n: [0, 1, 0] };
      const s = this._pick();
      s.volume = this.volume;
      return this._xf(s._eval(u));
    }
    const [u0, u1, u2] = u;
    const R = this.radius, Ri = this.inner, H = this.height;
    let p, n;
    switch (this.type) {
      case 'BOX': {
        const d = this.dim;
        if (this.volume) { p = [(u0 - 0.5) * d[0], (u1 - 0.5) * d[1], (u2 - 0.5) * d[2]]; n = [0, 1, 0]; break; }
        // face weighted by area; u0 picks face and side, u1/u2 place the point on it
        const ax = d[1] * d[2], ay = d[0] * d[2], az = d[0] * d[1], tot = 2 * (ax + ay + az) || 1;
        let x = u0 * tot, axis, a;
        if (x < 2 * ax) { axis = 0; a = ax; } else if ((x -= 2 * ax) < 2 * ay) { axis = 1; a = ay; } else { x -= 2 * ay; axis = 2; a = az; }
        const sgn = x < a ? 0.5 : -0.5;
        const o = axis === 0 ? [1, 2] : axis === 1 ? [0, 2] : [0, 1];
        p = [0, 0, 0]; n = [0, 0, 0];
        p[axis] = sgn * d[axis]; n[axis] = Math.sign(sgn);
        p[o[0]] = (u1 - 0.5) * d[o[0]]; p[o[1]] = (u2 - 0.5) * d[o[1]];
        break;
      }
      case 'CYLINDER': ({ p, n } = cylinderSample(u0, u1, u2, R, Ri, H, this.volume)); break;
      case 'CAPSULE': {
        const vol = this.volume && R > Ri;
        const sph = vol ? 4 / 3 * (R ** 3 - Ri ** 3) : 4 * (R * R + Ri * Ri);
        const cyl = vol ? (R * R - Ri * Ri) * H : 2 * (R + Ri) * H;
        const ps = sph / ((sph + cyl) || 1);
        if (u0 < ps) {
          ({ p, n } = sphereSample(u0 / ps, u1, u2, R, Ri, this.volume));
          p[1] += (p[1] < 0 ? -H : H) * 0.5;
        } else ({ p, n } = cylinderSample((u0 - ps) / (1 - ps), u1, u2, R, Ri, H, this.volume));
        break;
      }
      case 'CONE': {
        // apex at +Height, base disc at y = 0; surface is the lateral wall only
        const ang = u1 * Math.PI * 2, c = Math.cos(ang), sn = Math.sin(ang);
        if (this.volume) {
          const s = Math.cbrt(u0), rr = R * s * Math.sqrt(u2);
          p = [c * rr, (1 - s) * H, sn * rr];
        } else {
          const s = Math.sqrt(u0);
          p = [c * R * s, (1 - s) * H, sn * R * s];
        }
        n = vnorm([c * H, R, sn * H]);
        break;
      }
      default: { // SPHERE, COMPLEX_ELLIPSOID (+ MESH/PLANE fallback)
        ({ p, n } = sphereSample(u0, u1, u2, R, Ri, this.volume));
        if (this.hemisphere && p[1] < 0) { p[1] = -p[1]; n[1] = -n[1]; }
        if (this.scale) {
          p = [p[0] * this.scale[0], p[1] * this.scale[1], p[2] * this.scale[2]];
          n = vnorm([n[0] / (this.scale[0] || 1), n[1] / (this.scale[1] || 1), n[2] / (this.scale[2] || 1)]);
        }
      }
    }
    return this._xf({ p, n });
  }
  _xf({ p, n }) {
    if (this.rotate && this.rot) { p = mat3mul(this.rot, p); n = mat3mul(this.rot, n); }
    if (this.translate) p = [p[0] + this.pos[0], p[1] + this.pos[1], p[2] + this.pos[2]];
    return { p, n };
  }
  samplePosition(pc) { return this._eval(pcoords(pc) || this._u()).p; }
  sampleNormal(pc) { return this._eval(pcoords(pc) || this._u()).n; }
  // pcoords are opaque (our 3 uniforms, the engine's packed int3): .pc keeps int
  // declarations and int fields from truncating them
  samplePCoords() { const u = this._u(); u.pc = true; return u; }
  /* Closest point on the SURFACE (descriptor Project, shared by the Projection and
     Attractor evolvers): [offset xyz, signed distance (negative inside)], plus .pc
     (uniforms that _eval maps back to the same point) when asked. The sphere ignores
     rotation; the cylinder targets its side wall only, never the caps. */
  project(q, wantPC) {
    if (this.subs) return null;
    const T = this.pos, d = [q[0] - T[0], q[1] - T[1], q[2] - T[2]];
    let off, dist, pc = null;
    if (this.type === 'SPHERE' || this.type === 'COMPLEX_ELLIPSOID') {
      const m = Math.hypot(d[0], d[1], d[2]), R = this.radius;
      const n = m * m < 1e-12 ? [0, 1, 0] : [d[0] / m, d[1] / m, d[2] / m];
      dist = m - R;
      off = [n[0] * R - d[0], n[1] * R - d[1], n[2] * R - d[2]];
      if (wantPC) {
        const nl = this.rot ? mat3tmul(this.rot, n) : n;
        pc = [(nl[2] + 1) / 2, fract(Math.atan2(nl[1], nl[0]) / (2 * Math.PI)), 1 - 1e-7];
      }
    } else if (this.type === 'BOX') {
      const l = this.rot ? mat3tmul(this.rot, d) : d, h = this.dim.map((x) => x * 0.5);
      const sg = l.map((x) => (x < 0 || Object.is(x, -0) ? -1 : 1));
      const gap = [h[0] - Math.abs(l[0]), h[1] - Math.abs(l[1]), h[2] - Math.abs(l[2])];
      const m = Math.min(gap[0], gap[1], gap[2]), inside = m > 0;
      // inside: out through the nearest face; outside: clamp onto the box
      const o = inside ? gap.map((g, k) => (g === m ? g : 0) * sg[k])
                       : l.map((x, k) => Math.min(h[k], Math.abs(x)) * sg[k] - x);
      dist = Math.hypot(o[0], o[1], o[2]) * (inside ? -1 : 1);
      off = this.rot ? mat3mul(this.rot, o) : o;
    } else if (this.type === 'CYLINDER') {
      const l = this.rot ? mat3tmul(this.rot, d) : d, hh = this.height * 0.5, R = this.radius;
      const rm = Math.hypot(l[0], l[2]);
      const rx = rm * rm > 1e-12 ? l[0] / rm : 1, rz = rm * rm > 1e-12 ? l[2] / rm : 0;
      const cy = Math.max(-hh, Math.min(hh, l[1]));
      const o = [rx * R - l[0], cy - l[1], rz * R - l[2]];
      dist = Math.hypot(o[0], o[1], o[2]) * (rm < R && Math.abs(l[1]) < hh ? -1 : 1);
      off = this.rot ? mat3mul(this.rot, o) : o;
      if (wantPC) pc = [fract(Math.atan2(rz, rx) / (2 * Math.PI)), this.height > 0 ? Math.min(Math.max(cy / this.height + 0.5, 0), 1 - 1e-7) : 0.5, 1 - 1e-7];
    } else return null;
    const r = [off[0], off[1], off[2], dist];
    if (pc) r.pc = pc;
    return r;
  }
  // signed distance to the surface (descriptor DistanceField); null for other shapes
  distance(q) {
    if (this.subs) return null;
    const T = this.pos, d = [q[0] - T[0], q[1] - T[1], q[2] - T[2]];
    if (this.type === 'SPHERE') {                   // rotation ignored; InnerRadius makes a shell
      const m = Math.hypot(d[0], d[1], d[2]);
      return this.inner > 0 ? Math.max(m - this.radius, this.inner - m) : m - this.radius;
    }
    if (this.type === 'BOX') {
      const l = this.rot ? mat3tmul(this.rot, d) : d;
      const qx = Math.abs(l[0]) - this.dim[0] * 0.5, qy = Math.abs(l[1]) - this.dim[1] * 0.5, qz = Math.abs(l[2]) - this.dim[2] * 0.5;
      return Math.hypot(Math.max(qx, 0), Math.max(qy, 0), Math.max(qz, 0)) + Math.min(Math.max(qx, qy, qz), 0);
    }
    if (this.type === 'CYLINDER') {
      const l = this.rot ? mat3tmul(this.rot, d) : d, r = Math.hypot(l[0], l[2]);
      let v = Math.max(Math.abs(l[1]) - this.height * 0.5, r - this.radius);
      if (this.inner > 0) v = Math.max(v, this.inner - r);
      return v;
    }
    return null;
  }
  projectPCoords(q) { const r = this.project(q, true); const pc = r && r.pc ? r.pc.slice() : [0, 0, 0]; pc.pc = true; return pc; }
  /* Ray query for a Collider shape: nearest hit along o + d*t, t in [0, len], from
     either side of the surface. Sphere/ellipsoid only (the one corpus Collider). */
  intersect(o, d, len) {
    if (this.subs || !(this.type === 'SPHERE' || this.type === 'COMPLEX_ELLIPSOID')) return null;
    const sc = this.scale || [1, 1, 1], R = this.radius;
    let lo = [o[0] - this.pos[0], o[1] - this.pos[1], o[2] - this.pos[2]], ld = d;
    if (this.rotate && this.rot) { lo = mat3tmul(this.rot, lo); ld = mat3tmul(this.rot, d); }
    const so = [lo[0] / sc[0], lo[1] / sc[1], lo[2] / sc[2]], sd = [ld[0] / sc[0], ld[1] / sc[1], ld[2] / sc[2]];
    const A = sd[0] * sd[0] + sd[1] * sd[1] + sd[2] * sd[2];
    const B = so[0] * sd[0] + so[1] * sd[1] + so[2] * sd[2];
    const C = so[0] * so[0] + so[1] * so[1] + so[2] * so[2] - R * R;
    const disc = B * B - A * C;
    if (!(A > 0) || disc < 0) return null;
    const sq = Math.sqrt(disc);
    let t = (-B - sq) / A;
    if (t < 0) t = (-B + sq) / A;
    if (!(t >= 0 && t <= len)) return null;
    const hp = [so[0] + sd[0] * t, so[1] + sd[1] * t, so[2] + sd[2] * t];
    let n = [hp[0] / sc[0], hp[1] / sc[1], hp[2] / sc[2]];
    if (C < 0) n = [-n[0], -n[1], -n[2]];        // hit from inside: the normal faces in
    if (this.rotate && this.rot) n = mat3mul(this.rot, n);
    return { t, n: vnorm(n) };
  }
  position() { return [this.pos[0], this.pos[1], this.pos[2]]; }     // shape centre
  direction() { return this.rot ? mat3mul(this.rot, [0, 1, 0]) : [0, 1, 0]; }
}

// a pcoords argument is the 3 uniforms samplePCoords() handed out
function pcoords(pc) {
  if (!pc || pc.length < 3) return null;
  return [frac01(pc[0]), frac01(pc[1]), frac01(pc[2])];
}
function frac01(x) { x = +x || 0; return Math.min(Math.max(x, 0), 1 - 1e-7); }

function sphereSample(u0, u1, u2, R, Ri, volume) {
  const z = u0 * 2 - 1, a = u1 * Math.PI * 2, s = Math.sqrt(Math.max(0, 1 - z * z));
  const dir = [s * Math.cos(a), s * Math.sin(a), z];
  let rr;
  if (volume && R > Ri) rr = Math.cbrt(Ri ** 3 + (R ** 3 - Ri ** 3) * u2);
  else rr = Ri > 0 && u2 < Ri * Ri / (R * R + Ri * Ri) ? Ri : R;
  return { p: [dir[0] * rr, dir[1] * rr, dir[2] * rr], n: dir };
}
// Y axis, centred; surface is the side wall only (inner wall too when InnerRadius > 0)
function cylinderSample(u0, u1, u2, R, Ri, H, volume) {
  const ang = u0 * Math.PI * 2, c = Math.cos(ang), s = Math.sin(ang);
  const y = H * u1 - H * 0.5;
  let rr, sign = 1;
  if (volume && R > Ri) rr = Math.sqrt(Ri * Ri + (R * R - Ri * Ri) * u2);
  else if (Ri > 0 && u2 < Ri / (R + Ri)) { rr = Ri; sign = -1; }
  else rr = R;
  return { p: [c * rr, y, s * rr], n: [c * sign, 0, s * sign] };
}

// Procedural turbulence: the engine's own noise (turbulence.js). `time` is the medium's
// elapsed seconds, advanced by the simulation each frame.
export class TurbulenceSampler {
  constructor(obj) {
    const p = obj.props, props = {};
    for (const k of Object.keys(TURB_DEFAULTS)) {
      if (p[k] == null) continue;
      if (k === 'Interpolator' || k === 'DefaultSampledField') props[k] = toSym(p[k]);
      else if (k === 'FastFakeFlow') props[k] = p[k] === true;
      else props[k] = num(p[k], TURB_DEFAULTS[k]);
    }
    this.ref = new TurbulenceRef(props);
    this.time = 0;
  }
  sampleCurl(pos) { return this.ref.sampleCurl(pos, this.time); }
  samplePotential(pos) { return this.ref.samplePotential(pos, this.time); }
  sample(pos) { return this.ref.sample(pos || [0, 0, 0], this.time); }
}

function eulerMatrix(deg) {
  // degrees, applied X then Y then Z: M = Rz * Ry * Rx (row-major 3x3)
  const r = deg.map((d) => d * Math.PI / 180);
  const cx = Math.cos(r[0]), sx = Math.sin(r[0]);
  const cy = Math.cos(r[1] ?? 0), sy = Math.sin(r[1] ?? 0);
  const cz = Math.cos(r[2] ?? 0), sz = Math.sin(r[2] ?? 0);
  return [
    cy * cz, sx * sy * cz - cx * sz, cx * sy * cz + sx * sz,
    cy * sz, sx * sy * sz + cx * cz, cx * sy * sz - sx * cz,
    -sy, sx * cy, cx * cy,
  ];
}
function mat3mul(m, v) {
  return [
    m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
    m[3] * v[0] + m[4] * v[1] + m[5] * v[2],
    m[6] * v[0] + m[7] * v[1] + m[8] * v[2],
  ];
}

function mat3tmul(m, v) {
  return [
    m[0] * v[0] + m[3] * v[1] + m[6] * v[2],
    m[1] * v[0] + m[4] * v[1] + m[7] * v[2],
    m[2] * v[0] + m[5] * v[1] + m[8] * v[2],
  ];
}

function fract(x) { return x - Math.floor(x); }
function vnorm(v) { const l = Math.hypot(v[0], v[1], v[2]) || 1; return [v[0] / l, v[1] / l, v[2] / l]; }
function num(v, d) { return typeof v === 'number' ? v : (v == null ? d : (toNums(v)?.[0] ?? d)); }

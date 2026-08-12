// Samplers referenced by particle scripts and Field evolvers:
//  - CParticleSamplerCurve                keyframed curve (Linear or cubic-Hermite), 1..4 components
//  - CParticleSamplerDoubleCurve          two curves blended by a 0..1 selector (.sample(t, sel))
//  - CParticleSamplerShape                emission shape (.samplePosition()/.sampleNormal())
//  - CParticleSamplerProceduralTurbulence animated vector-noise velocity field (.sampleCurl(pos))
//  - CParticleSamplerAnimTrack            baked motion path (.samplePosition(cursor)) — a .pkan
import { deref, toNums, toSym } from './parser.js';

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
      o[k] = h00 * p0[k] + h10 * dt * mOut + h01 * p1[k] + h11 * dt * mIn;
    }
    return o;
  }
  key(i) { const c = this.c; const o = new Array(c); for (let k = 0; k < c; k++) o[k] = this.v[i * c + k] ?? 0; return o; }
  // tangents stored per key as [in_c0, out_c0, in_c1, out_c1, ...]; which: 0=in, 1=out
  tanAt(i, k, which) { const base = i * this.c * 2 + k * 2 + which; return this.tan[base] ?? 0; }
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
    const o = this.curve.sample(t == null ? 0 : t[0] ?? t);
    if (this.min) for (let k = 0; k < o.length; k++) { const m = this.min[k]; if (isFinite(m) && o[k] < m) o[k] = m; }
    if (this.max) for (let k = 0; k < o.length; k++) { const m = this.max[k]; if (isFinite(m) && o[k] > m) o[k] = m; }
    return o;
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
  }
  sample(t, sel) {
    const x = t == null ? 0 : (t[0] ?? t); const s = sel == null ? 0 : (sel[0] ?? sel);
    const a = this.c0.sample(x), b = this.c1.sample(x);
    const o = new Array(this.comp); for (let k = 0; k < this.comp; k++) o[k] = a[k] + (b[k] - a[k]) * s;
    return o;
  }
}

// A baked motion path: `AnimResource` names a mesh, but what the baker actually
// wrote is the sibling `.pkan` (the corpus's spline .hcf configs say
// `Geometry = false; Animation = true;`, so several have no .pkmm at all).
//
// A .pkan is the same text HBO format as a .pkfx, so the effect parser reads it:
//   CAnimationClip { EntityStreams -> CAnimationTrack { Channels -> CSamplerCurve } }
// with one curve per BindingSemantic (Translation / Rotation / Scale).
//
// The resource arrives over the network, so construction only records the path;
// the viewer parses the .pkan and calls `load()`. Until then every channel reads
// zero, which is what the sampler did before it was implemented.
export class AnimTrackSampler {
  constructor(obj) {
    this.resource = typeof obj.props.AnimResource === 'string' ? obj.props.AnimResource : null;
    this.length = 1;      // clip duration in seconds
    this.channels = null;
  }
  // The animation always lives in the .pkan beside the named mesh.
  resourceRef() {
    return this.resource ? this.resource.replace(/\.[^.\\/]+$/, '.pkan') : null;
  }
  load(doc) {
    let clip = null;
    for (const id of doc.order) if (doc.objects[id].className === 'CAnimationClip') { clip = doc.objects[id]; break; }
    if (!clip) return false;
    this.length = Math.max(num(clip.props.LengthInSeconds, 1), 1e-6);
    const channels = {};
    for (const tref of clip.props.EntityStreams || []) {
      const track = deref(doc, tref);
      if (!track) continue;
      for (const cref of track.props.Channels || []) {
        const ch = deref(doc, cref);
        if (!ch) continue;
        const sem = typeof ch.props.BindingSemantic === 'string' ? ch.props.BindingSemantic : '';
        const times = toNums(ch.props.Times) || [];
        if (!sem || !times.length || channels[sem]) continue;   // first track to define a channel wins
        const values = toNums(ch.props.FloatValues) || [];
        const tangents = toNums(ch.props.FloatTangents) || [];
        const comp = compCount(ch.props.ValueType, times, values);
        channels[sem] = new Curve(times, values, tangents, comp, toSym(ch.props.Interpolator) === 'Linear');
      }
    }
    this.channels = channels;
    return Object.keys(channels).length > 0;
  }
  // Scripts pass a 0..1 cursor (`samplePosition(LifeRatio)`) — traverse the whole
  // clip over that range rather than treating the argument as seconds.
  _sample(sem, cursor, absent) {
    const c = this.channels && this.channels[sem];
    if (!c) return absent.slice();
    const u = cursor == null ? 0 : (cursor[0] ?? cursor);
    return c.sample(u * this.length);
  }
  samplePosition(cursor) { return this._sample('Translation', cursor, [0, 0, 0]); }
  sampleRotation(cursor) { return this._sample('Rotation', cursor, [0, 0, 0]); }
  sampleScale(cursor) { return this._sample('Scale', cursor, [1, 1, 1]); }
  sampleNormal() { return [0, 1, 0]; }
  sample(cursor) { return this.samplePosition(cursor); }
}

// Emission shape. Default SampleDimensionality is Surface (the corpus only ever
// writes Volume/Vertex overrides); EulerOrientation/NonUniformScale/Hemisphere
// re-shape the sample; Position offsets it.
export class ShapeSampler {
  constructor(shape, rng, doc) {
    this.rng = rng;
    // A CShapeDescriptorCollection emits from several shapes at once - a campfire's
    // base is the four edges of a square, each a flattened box. The collection object
    // carries no dimensions of its own, so without this it read as the default unit
    // sphere and the emitter changed shape entirely.
    this.subs = shape && shape.className === 'CShapeDescriptorCollection'
      ? (shape.props.SubShapes || []).map((ref) => new ShapeSampler(deref(doc, ref), rng, doc))
      : null;
    const p = shape && !this.subs ? shape.props : {};
    this.dim = toNums(p.BoxDimensions || p.Dimensions) || null;
    // ShapeType is often omitted; infer BOX from BoxDimensions, else default to a sphere
    this.type = toSym(p.ShapeType) || (p.BoxDimensions ? 'BOX' : 'SPHERE');
    this.radius = num(p.Radius, 1);
    this.inner = num(p.InnerRadius, 0);
    this.height = num(p.Height, num(p.Length, 1));
    if (!this.dim) this.dim = [1, 1, 1];
    this.pos = toNums(p.Position) || [0, 0, 0];
    this.scale = toNums(p.NonUniformScale) || null;
    this.hemisphere = p.Hemisphere === true;
    this.volume = false; // set per-sampler from SampleDimensionality
    const euler = toNums(p.EulerOrientation);
    this.rot = euler && (euler[0] || euler[1] || euler[2]) ? eulerMatrix(euler) : null;
    this.lastNormal = [0, 1, 0];
  }
  samplePosition() {
    if (this.subs && this.subs.length) {
      const s = this.subs[Math.min(this.subs.length - 1, Math.floor(this.rng() * this.subs.length))];
      s.volume = this.volume;
      const q = s.samplePosition();
      this.lastNormal = s.lastNormal;
      return q;
    }
    const r = this.rng; let p, n;
    switch (this.type) {
      case 'CYLINDER': case 'CAPSULE': {
        const ang = r() * Math.PI * 2;
        // surface: the side wall; volume: annulus between inner and outer radius
        const rr = this.volume ? Math.sqrt(this.inner * this.inner / (this.radius * this.radius || 1) + (1 - this.inner * this.inner / (this.radius * this.radius || 1)) * r()) * this.radius
                               : this.radius;
        const y = (r() - 0.5) * this.height;
        p = [Math.cos(ang) * rr, y, Math.sin(ang) * rr];
        n = vnorm([p[0], 0, p[2]]); break;
      }
      case 'BOX': {
        if (this.volume) {
          p = [(r() - 0.5) * this.dim[0], (r() - 0.5) * this.dim[1], (r() - 0.5) * this.dim[2]];
          n = [0, 1, 0];
        } else {
          // pick a face weighted by area, sample it
          const [dx, dy, dz] = this.dim;
          const ax = dy * dz, ay = dx * dz, az = dx * dy;
          const pick = r() * (ax + ay + az) * 2;
          const s = (v) => (r() - 0.5) * v;
          if (pick < ax * 2) { const sgn = pick < ax ? 0.5 : -0.5; p = [sgn * dx, s(dy), s(dz)]; n = [Math.sign(sgn), 0, 0]; }
          else if (pick < (ax + ay) * 2) { const sgn = pick < ax * 2 + ay ? 0.5 : -0.5; p = [s(dx), sgn * dy, s(dz)]; n = [0, Math.sign(sgn), 0]; }
          else { const sgn = pick < (ax + ay) * 2 + az ? 0.5 : -0.5; p = [s(dx), s(dy), sgn * dz]; n = [0, 0, Math.sign(sgn)]; }
        }
        break;
      }
      case 'CONE': {
        const ang = r() * Math.PI * 2;
        const h = this.volume ? Math.cbrt(r()) : Math.sqrt(r()); // area-ish weighting toward the base
        const rr = this.radius * h;
        p = [Math.cos(ang) * rr, h * this.height, Math.sin(ang) * rr];
        n = vnorm(p); break;
      }
      default: { // SPHERE / COMPLEX_ELLIPSOID / MESH fallback
        const dir = randDir(r);
        const rr = this.volume
          ? this.inner + (this.radius - this.inner) * Math.cbrt(r())
          : this.radius;
        p = [dir[0] * rr, dir[1] * rr, dir[2] * rr]; n = dir; break;
      }
    }
    if (this.hemisphere && p[1] < 0) { p[1] = -p[1]; n = [n[0], Math.abs(n[1]), n[2]]; }
    if (this.scale) { p = [p[0] * this.scale[0], p[1] * this.scale[1], p[2] * this.scale[2]]; }
    if (this.rot) { p = mat3mul(this.rot, p); n = mat3mul(this.rot, n); }
    this.lastNormal = n;
    return [p[0] + this.pos[0], p[1] + this.pos[1], p[2] + this.pos[2]];
  }
  sampleNormal() { return this.lastNormal; }
  samplePCoords() { return [this.rng(), this.rng()]; }
  position() {                                                        // shape centre
    if (this.subs && this.subs.length) {
      const c = [0, 0, 0];
      for (const s of this.subs) { const q = s.position(); c[0] += q[0]; c[1] += q[1]; c[2] += q[2]; }
      return c.map((v) => v / this.subs.length);
    }
    return [this.pos[0], this.pos[1], this.pos[2]];
  }
  direction() { const d = this.rot ? mat3mul(this.rot, [0, 1, 0]) : [0, 1, 0]; return d; }
}

// Procedural turbulence: fractal value-noise vector field, animated by TimeScale.
// Used by Physics (VelocityFieldSampler) and scripts (sampleCurl). FastFakeFlow
// reads the vector noise directly; otherwise we take a finite-difference curl,
// which is divergence-free (the "swirly" look).
export class TurbulenceSampler {
  constructor(obj) {
    const p = obj.props;
    this.strength = num(p.Strength, 1);
    this.wavelength = Math.max(1e-3, num(p.Wavelength, 1) * num(p.GlobalScale, 1));
    this.timeScale = num(p.TimeScale, 1) * num(p.TimeBase, 1);
    this.octaves = Math.max(1, Math.min(4, Math.round(num(p.Octaves, 1))));
    this.gain = num(p.Gain, 0.5);
    this.fast = p.FastFakeFlow === true;
    this.time = 0; // advanced by the simulation each frame
  }
  // vector noise at world position (already scaled into noise space)
  _vec(x, y, z, out) {
    let amp = 1, freq = 1, total = 0;
    out[0] = 0; out[1] = 0; out[2] = 0;
    for (let o = 0; o < this.octaves; o++) {
      out[0] += vnoise(x * freq, y * freq, z * freq, 0) * amp;
      out[1] += vnoise(x * freq, y * freq, z * freq, 113) * amp;
      out[2] += vnoise(x * freq, y * freq, z * freq, 227) * amp;
      total += amp; amp *= this.gain; freq *= 2;
    }
    const inv = total > 0 ? 1 / total : 1;
    out[0] *= inv; out[1] *= inv; out[2] *= inv;
  }
  // velocity field sample; `pos` is a [x,y,z] array
  sampleCurl(pos) {
    const s = 1 / this.wavelength;
    const t = this.time * this.timeScale;
    const x = (pos[0] ?? 0) * s, y = (pos[1] ?? 0) * s + t, z = (pos[2] ?? 0) * s + t * 0.7;
    const a = TMP0;
    if (this.fast) {
      this._vec(x, y, z, a);
      return [a[0] * this.strength, a[1] * this.strength, a[2] * this.strength];
    }
    // curl via central differences of the vector potential
    const e = 0.25;
    const b = TMP1;
    this._vec(x, y + e, z, a); this._vec(x, y - e, z, b);
    const dPz_dy = (a[2] - b[2]) / (2 * e), dPx_dy = (a[0] - b[0]) / (2 * e);
    this._vec(x, y, z + e, a); this._vec(x, y, z - e, b);
    const dPy_dz = (a[1] - b[1]) / (2 * e), dPx_dz = (a[0] - b[0]) / (2 * e);
    this._vec(x + e, y, z, a); this._vec(x - e, y, z, b);
    const dPz_dx = (a[2] - b[2]) / (2 * e), dPy_dx = (a[1] - b[1]) / (2 * e);
    return [
      (dPz_dy - dPy_dz) * this.strength,
      (dPx_dz - dPz_dx) * this.strength,
      (dPy_dx - dPx_dy) * this.strength,
    ];
  }
  sample(pos) { return this.sampleCurl(pos || [0, 0, 0]); }
}
const TMP0 = [0, 0, 0], TMP1 = [0, 0, 0];

// smooth 3D value noise in [-1,1] (hash-gradient free — cheap and stable)
function vnoise(x, y, z, seed) {
  const xi = Math.floor(x), yi = Math.floor(y), zi = Math.floor(z);
  const xf = x - xi, yf = y - yi, zf = z - zi;
  const u = xf * xf * (3 - 2 * xf), v = yf * yf * (3 - 2 * yf), w = zf * zf * (3 - 2 * zf);
  const h = (i, j, k) => {
    let n = (i + seed) * 374761393 + j * 668265263 + k * 2147483647;
    n = (n ^ (n >>> 13)) >>> 0;
    n = (n * 1274126177) >>> 0;
    return (n & 0xffff) / 32768 - 1;
  };
  const lerp = (a, b, t) => a + (b - a) * t;
  return lerp(
    lerp(lerp(h(xi, yi, zi), h(xi + 1, yi, zi), u), lerp(h(xi, yi + 1, zi), h(xi + 1, yi + 1, zi), u), v),
    lerp(lerp(h(xi, yi, zi + 1), h(xi + 1, yi, zi + 1), u), lerp(h(xi, yi + 1, zi + 1), h(xi + 1, yi + 1, zi + 1), u), v),
    w);
}

function eulerMatrix(deg) {
  // XYZ euler (degrees) -> row-major 3x3
  const r = deg.map((d) => d * Math.PI / 180);
  const cx = Math.cos(r[0]), sx = Math.sin(r[0]);
  const cy = Math.cos(r[1] ?? 0), sy = Math.sin(r[1] ?? 0);
  const cz = Math.cos(r[2] ?? 0), sz = Math.sin(r[2] ?? 0);
  return [
    cy * cz, -cy * sz, sy,
    sx * sy * cz + cx * sz, -sx * sy * sz + cx * cz, -sx * cy,
    -cx * sy * cz + sx * sz, cx * sy * sz + sx * cz, cx * cy,
  ];
}
function mat3mul(m, v) {
  return [
    m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
    m[3] * v[0] + m[4] * v[1] + m[5] * v[2],
    m[6] * v[0] + m[7] * v[1] + m[8] * v[2],
  ];
}

function randDir(r) {
  const z = r() * 2 - 1, a = r() * Math.PI * 2, s = Math.sqrt(1 - z * z);
  return [s * Math.cos(a), s * Math.sin(a), z];
}
function vnorm(v) { const l = Math.hypot(v[0], v[1], v[2]) || 1; return [v[0] / l, v[1] / l, v[2] / l]; }
function num(v, d) { return typeof v === 'number' ? v : (v == null ? d : (toNums(v)?.[0] ?? d)); }

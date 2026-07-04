// Samplers referenced by particle scripts and Field evolvers:
//  - CParticleSamplerCurve        keyframed curve (Linear or cubic-Hermite), 1..4 components
//  - CParticleSamplerDoubleCurve  two curves blended by a 0..1 selector (.sample(t, sel))
//  - CParticleSamplerShape        emission shape (.samplePosition()/.sampleNormal())
import { toNums, toSym } from './parser.js';

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
  }
  sample(t) { return this.curve.sample(t == null ? 0 : t[0] ?? t); }
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

export class ShapeSampler {
  constructor(shape, rng) {
    this.rng = rng;
    const p = shape ? shape.props : {};
    this.dim = toNums(p.BoxDimensions || p.Dimensions) || null;
    // ShapeType is often omitted; infer BOX from BoxDimensions, else default to a sphere
    this.type = toSym(p.ShapeType) || (p.BoxDimensions ? 'BOX' : 'SPHERE');
    this.radius = num(p.Radius, 1);
    this.inner = num(p.InnerRadius, 0);
    this.height = num(p.Height, num(p.Length, 1));
    if (!this.dim) this.dim = [1, 1, 1];
    this.pos = toNums(p.Position) || [0, 0, 0];
    this.lastNormal = [0, 1, 0];
  }
  samplePosition() {
    const r = this.rng; let p;
    switch (this.type) {
      case 'CYLINDER': case 'CAPSULE': {
        const ang = r() * Math.PI * 2;
        const rr = this.inner + (this.radius - this.inner) * Math.sqrt(r());
        const y = (r() - 0.5) * this.height;
        p = [Math.cos(ang) * rr, y, Math.sin(ang) * rr];
        this.lastNormal = vnorm([p[0], 0, p[2]]); break;
      }
      case 'BOX': {
        p = [(r() - 0.5) * this.dim[0], (r() - 0.5) * this.dim[1], (r() - 0.5) * this.dim[2]];
        this.lastNormal = [0, 1, 0]; break;
      }
      case 'CONE': {
        const ang = r() * Math.PI * 2; const h = r(); const rr = this.radius * h;
        p = [Math.cos(ang) * rr, h * this.height, Math.sin(ang) * rr];
        this.lastNormal = vnorm(p); break;
      }
      default: { // SPHERE / COMPLEX_ELLIPSOID
        const dir = randDir(r); const rr = this.inner + (this.radius - this.inner) * Math.cbrt(r());
        p = [dir[0] * rr, dir[1] * rr, dir[2] * rr]; this.lastNormal = dir; break;
      }
    }
    return [p[0] + this.pos[0], p[1] + this.pos[1], p[2] + this.pos[2]];
  }
  sampleNormal() { return this.lastNormal; }
  samplePCoords() { return [this.rng(), this.rng()]; }
}

function randDir(r) {
  const z = r() * 2 - 1, a = r() * Math.PI * 2, s = Math.sqrt(1 - z * z);
  return [s * Math.cos(a), s * Math.sin(a), z];
}
function vnorm(v) { const l = Math.hypot(v[0], v[1], v[2]) || 1; return [v[0] / l, v[1] / l, v[2] / l]; }
function num(v, d) { return typeof v === 'number' ? v : (v == null ? d : (toNums(v)?.[0] ?? d)); }

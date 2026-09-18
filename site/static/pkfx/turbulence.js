// PopcornFX 1.13 CParticleSamplerProceduralTurbulence, ported from HH-Bridge_r.dll: Perlin-
// permutation value noise over an SFMT-seeded lattice whose values rotate with time. Float
// math is rounded to fp32 in the engine's SSE op order.

const f = Math.fround;

// Ken Perlin's reference permutation, duplicated; [512] = 151. Static data at 0x1809c5db0.
const P0 = [151,160,137,91,90,15,131,13,201,95,96,53,194,233,7,225,140,36,103,30,69,142,8,99,37,240,21,10,23,190,6,148,247,120,234,75,0,26,197,62,94,252,219,203,117,35,11,32,57,177,33,88,237,149,56,87,174,20,125,136,171,168,68,175,74,165,71,134,139,48,27,166,77,146,158,231,83,111,229,122,60,211,133,230,220,105,92,41,55,46,245,40,244,102,143,54,65,25,63,161,1,216,80,73,209,76,132,187,208,89,18,169,200,196,135,130,116,188,159,86,164,100,109,198,173,186,3,64,52,217,226,250,124,123,5,202,38,147,118,126,255,82,85,212,207,206,59,227,47,16,58,17,182,189,28,42,223,183,170,213,119,248,152,2,44,154,163,70,221,153,101,155,167,43,172,9,129,22,39,253,19,98,108,110,79,113,224,232,178,185,112,104,218,246,97,228,251,34,242,193,238,210,144,12,191,179,162,241,81,51,145,235,249,14,239,107,49,192,214,31,181,199,106,157,184,84,204,176,115,121,50,45,127,4,150,254,138,236,205,93,222,114,67,29,24,72,243,141,128,195,78,66,215,61,156,180];
export const PERM = new Int32Array(513);
for (let i = 0; i < 512; i++) PERM[i] = P0[i & 255];
PERM[512] = 151;

const PI = f(3.1415927), TWO_PI = f(6.2831855), HALF_PI = f(1.5707964), INV_TWO_PI = f(0.15915494);
const OFF1 = [f(31.416), f(47.853), f(12.793)];
const OFF2 = [f(-233.145), f(-113.408), f(-185.31)];
const FD_H = f(1e-4);      // noise+0x30 (0x38d1b717)
const FD_INV = f(5000.0);  // noise+0x34 (0x459c4000)

// ---- SFMT19937 (FUN_1804e1b10 seed, FUN_1804e1a40 regen, no period certification) ----
export class SFMT {
  constructor(seed) {
    this.s = new Uint32Array(624);
    this.s[0] = seed >>> 0;
    for (let i = 1; i < 624; i++) {
      const p = this.s[i - 1];
      this.s[i] = (Math.imul((p ^ (p >>> 30)) >>> 0, 0x6c078965) + i) >>> 0;
    }
    this.idx = 624;
  }
  _regen() {
    const s = this.s, M = [0xdfffffef, 0xddfecb7f, 0xbffaffff, 0xbffffff6];
    let r1 = 154 * 4, r2 = 155 * 4;
    const o = [0, 0, 0, 0];
    for (let i = 0; i < 156; i++) {
      const a = i * 4, b = ((i + 122) % 156) * 4;
      const a0 = s[a], a1 = s[a + 1], a2 = s[a + 2], a3 = s[a + 3];
      const c0 = s[r1], c1 = s[r1 + 1], c2 = s[r1 + 2], c3 = s[r1 + 3];
      const x = [a0 << 8, (a1 << 8) | (a0 >>> 24), (a2 << 8) | (a1 >>> 24), (a3 << 8) | (a2 >>> 24)];
      const y = [(c0 >>> 8) | (c1 << 24), (c1 >>> 8) | (c2 << 24), (c2 >>> 8) | (c3 << 24), c3 >>> 8];
      for (let k = 0; k < 4; k++) o[k] = (s[a + k] ^ x[k] ^ ((s[b + k] >>> 11) & M[k]) ^ y[k] ^ (s[r2 + k] << 18)) >>> 0;
      s[a] = o[0]; s[a + 1] = o[1]; s[a + 2] = o[2]; s[a + 3] = o[3];
      r1 = r2; r2 = a;
    }
  }
  // FUN_1804e1f90: n floats in [min,max) from raw state words (mantissa trick), 4-aligned reads.
  fillRange(out, n, min, max) {
    const range = f(max - min), base = f(min - range);
    const bits = new Uint32Array(1), fl = new Float32Array(bits.buffer);
    let w = 0;
    while (w < n) {
      let start = (this.idx + 3) & ~3;
      let avail = 624 - start;
      if (avail < 1) { this._regen(); start = 0; avail = 624; }
      const take = Math.min(avail, n - w);
      for (let k = 0; k < take; k++) {
        bits[0] = (this.s[start + k] & 0x7fffff) | 0x3f800000;
        out[w++] = f(f(fl[0] * range) + base);
      }
      this.idx = start + take;
    }
  }
}

// ---- fast trig (FUN_1804de760 sin, FUN_1804d93a0 asin) ----
function cvtt(x) { return (x >= -2147483648 && x < 2147483648) ? Math.trunc(x) : -2147483648; }
function signbit(x) { return x < 0 || Object.is(x, -0); }
export function fastSin(x) {
  const a = f(x * INV_TWO_PI);
  let fr = Math.abs(f(a - cvtt(a)));
  let y = f(f(fr + fr) - 1);
  if (signbit(a)) y = -y;
  let A = f(y * -4);
  A = f(A - f(Math.abs(y) * A));
  return f(f(f(f(Math.abs(A) * A) - A) * f(0.22400817)) + A);
}
export function fastAsin(x) {
  const ax = Math.min(Math.abs(x), 1);
  const s = f(Math.sqrt(f(1 - ax)));
  let p = f(f(ax * f(-0.0043601324)) + f(0.019467467));
  p = f(f(p * ax) + f(-0.045130268));
  p = f(f(p * ax) + f(0.08797309));
  p = f(f(p * ax) + f(-0.21453293));
  p = f(f(p * ax) + HALF_PI);
  let r = f(s * p);
  if (signbit(x)) r = f(PI - r);
  return f(HALF_PI - r);
}

// ---- CRotatableFastNoise3 (FUN_1807e6620 ctor, FUN_1807eb330 TRV, FUN_1807e6d50 SetTime) ----
export class RotatableFastNoise3 {
  constructor(seed) {
    const rng = new SFMT(seed);
    this.V = new Float32Array(513);          // +0x08 lattice values
    this.base = new Float32Array(256);       // +0x10 base angles
    this.speed = new Float32Array(256);      // +0x18 angular speeds
    rng.fillRange(this.V, 256, -1, 1);
    this._dup();
    this.trv = f(0.2);
    const m = f(this.trv * PI);
    rng.fillRange(this.speed, 256, f(TWO_PI - m), f(m + TWO_PI));
    for (let i = 0; i < 256; i++) this.base[i] = fastAsin(this.V[i]);
    for (let i = 1; i < 256; i += 2) this.base[i] = f(this.base[i] + PI);
    this.uniform = false;
    this.time = 0;                           // +0x28 cached phase; phase 0 keeps the raw RNG values
  }
  _dup() { for (let i = 0; i < 256; i++) this.V[256 + i] = this.V[i]; this.V[512] = this.V[0]; }
  setTimeRandomVariation(v) {
    v = f(v);
    if (v < 1e-5) { this.uniform = true; return; }
    const old = f(this.trv * PI);
    this.trv = v; this.uniform = false;
    const vp = f(v * PI);
    const lowNew = f(TWO_PI - vp), k = f(vp / old), lowOld = f(TWO_PI - old);
    for (let i = 0; i < 256; i++) this.speed[i] = f(f(f(this.speed[i] - lowOld) * k) + lowNew);
  }
  setTime(phase) {
    phase = f(phase);
    if (phase === this.time) return;
    this.time = phase;
    if (!this.uniform) {
      for (let i = 0; i < 256; i++) this.V[i] = fastSin(f(f(this.speed[i] * phase) + this.base[i]));
    } else {
      const p2 = f(phase * TWO_PI);
      for (let i = 0; i < 256; i++) this.V[i] = fastSin(f(this.base[i] + p2));
    }
    this._dup();
  }
}

// ---- lattice helpers ----
// Interpolator: 0 Linear, 1 Cubic (engine default), 2 Quintic
function cell(x) {
  const ti = cvtt(x), tf = ti, lt = x < tf;
  return { i: (ti + (lt ? -1 : 0)) & 255, fr: f((lt ? 1 : 0) + f(x - tf)) };
}
function fadeScalar(t, k) {         // FUN_1807e99d0 / 1807e84c0 / 1807eb180
  if (k === 0) return t;
  if (k === 1) return f(f(t * t) * f(3 - f(t + t)));
  return f(f(f(t * t) * t) * f(f(t * f(f(t * 6) - 15)) + 10));
}
function fade4(t, k) {              // FUN_1807e2410 / 1807e2240 / 1807e25a0
  if (k === 0) return t;
  if (k === 1) return f(f(f(3 - f(t + t)) * t) * t);
  return f(f(f(t * t) * t) * f(f(f(f(t * 6) - 15) * t) + 10));
}
function corners(V, X, Y, Z) {
  const A = PERM[X] + Y, B = PERM[X + 1] + Y;
  const AA = PERM[A] + Z, AB = PERM[A + 1] + Z, BA = PERM[B] + Z, BB = PERM[B + 1] + Z;
  return [V[AA], V[BA], V[AB], V[BB], V[AA + 1], V[BA + 1], V[AB + 1], V[BB + 1]]; // c000 c100 c010 c110 c001 c101 c011 c111
}
function lerp(a, b, t) { return f(f(f(b - a) * t) + a); }
function trilerp(c, u, v, w) {
  const x00 = lerp(c[0], c[1], u), x01 = lerp(c[4], c[5], u);
  const y0 = lerp(x00, lerp(c[2], c[3], u), v);
  const y1 = lerp(x01, lerp(c[6], c[7], u), v);
  return lerp(y0, y1, w);
}
export function noise1(V, x, y, z, k) {
  const cx = cell(x), cy = cell(y), cz = cell(z);
  return trilerp(corners(V, cx.i, cy.i, cz.i), fadeScalar(cx.fr, k), fadeScalar(cy.fr, k), fadeScalar(cz.fr, k));
}
// 4 points evaluated in the lattice cell of pts[0] (FUN_1807e2240 family)
export function noise4(V, pts, k) {
  const cx = cell(pts[0][0]), cy = cell(pts[0][1]), cz = cell(pts[0][2]);
  const c = corners(V, cx.i, cy.i, cz.i);
  return pts.map((p) => {
    const lx = f(f(p[0] - pts[0][0]) + cx.fr), ly = f(f(p[1] - pts[0][1]) + cy.fr), lz = f(f(p[2] - pts[0][2]) + cz.fr);
    return trilerp(c, fade4(lx, k), fade4(ly, k), fade4(lz, k));
  });
}
// analytic gradient w.r.t. noise-space coords (FUN_1807e3140 / 1807e2e30 / 1807e3470)
export function grad1(V, x, y, z, k) {
  const cx = cell(x), cy = cell(y), cz = cell(z);
  const c = corners(V, cx.i, cy.i, cz.i);
  const fu = (t) => k === 0 ? t : k === 1 ? f(f(f(3 - f(t + t)) * t) * t)
    : f(f(f(f(f(f(t * 6) - 15) * t) + 10) * f(t * t)) * t);
  const df = (t) => {
    if (k === 0) return 1;
    if (k === 1) { const s = f(t * 6); return f(s - f(s * t)); }
    const t2 = f(t * t);
    return f(f(f(f(t2 - f(t + t)) * t2) * 30) + f(t2 * 30));
  };
  const u = f(fu(cx.fr)), v = f(fu(cy.fr)), w = f(fu(cz.fr));
  const [c000, c100, c010, c110, c001, c101, c011, c111] = c;
  const k1 = f(c100 - c000), k2 = f(c010 - c000), k3 = f(c001 - c000);
  const t14 = f(c011 - c001), t16 = f(c110 - c010), t15 = f(c101 - c100);
  const kxy = f(t16 - k1), kyz = f(t14 - k2), kxz = f(t15 - k3);
  const m = f(f(f(f(c000 - c111) + t16) + t14) + t15);
  const dx = f(f(f(f(f(kxz * w) - f(f(m * w) * v)) + f(v * kxy)) + k1) * df(cx.fr));
  const dy = f(f(f(f(f(kxy * u) - f(f(m * u) * w)) + f(w * kyz)) + k2) * df(cy.fr));
  const dz = f(f(f(f(f(kyz * v) - f(f(m * v) * u)) + f(u * kxz)) + k3) * df(cz.fr));
  return [dx, dy, dz];
}

// ---- the sampler ----
export const TURB_DEFAULTS = {
  GlobalScale: 1, Strength: 0.1, Wavelength: 0.5, Octaves: 2, Lacunarity: 0.5, Gain: 0.5,
  Interpolator: 'Cubic', DefaultSampledField: 'Curl', TimeScale: 0, TimeBase: 0,
  TimeRandomVariation: 0.5, FlowFactor: 1, DivergenceFactor: 0, InitialSeed: 0x4269CAFE,
  FastFakeFlow: false, GainMultiplier: 1,
};
const INTERP = { Linear: 0, Cubic: 1, Quintic: 2 };

export class TurbulenceRef {
  constructor(props = {}) {
    const p = { ...TURB_DEFAULTS, ...props };
    this.p = p;
    this.interp = typeof p.Interpolator === 'number' ? p.Interpolator : INTERP[p.Interpolator] ?? 1;
    this.curlDefault = p.DefaultSampledField !== 'Potential';
    this.noise = new RotatableFastNoise3(p.InitialSeed | 0);
    this._octaves();                                   // FUN_1806a5420
    this.noise.setTimeRandomVariation(p.TimeRandomVariation);
  }
  _octaves() {
    const p = this.p, N = Math.max(0, p.Octaves | 0);
    let wl = f(f(p.GlobalScale) * f(p.Wavelength));
    const freq0 = f(1 / wl);
    const lacMin = f(Math.pow(f(freq0 * f(1e-7)), f(1 / N)));
    const lac = f(p.Lacunarity) <= lacMin ? lacMin : f(p.Lacunarity);
    let amp = f(f(p.GlobalScale) * f(p.Strength));
    const gain = f(f(p.GainMultiplier) * f(p.Gain));
    this.oct = [];
    if (lac === 1 || gain === 0) {
      let sum = 0;
      for (let i = 0; i < N; i++) { sum = f(sum + amp); amp = f(amp * gain); }
      this.oct.push([freq0, sum]);
    } else {
      for (let i = 0; i < Math.min(N, 24); i++) { this.oct.push([f(1 / wl), amp]); wl = f(wl * lac); amp = f(amp * gain); }
    }
  }
  // phase = time*TimeScale + TimeBase; engine time = medium-collection elapsed seconds (double -> float)
  _setTime(t) { this.noise.setTime(f(f(f(t) * f(this.p.TimeScale)) + f(this.p.TimeBase))); }

  _potential(pos) {                                   // FUN_1806932e0 / 180692f40 / 180693110
    const V = this.noise.V, k = this.interp, out = [0, 0, 0];
    for (const [fq, amp] of this.oct) out[0] = f(out[0] + f(noise1(V, f(pos[0] * fq), f(pos[1] * fq), f(pos[2] * fq), k) * amp));
    for (const [fq, amp] of this.oct) out[1] = f(out[1] + f(noise1(V, f(f(pos[0] * fq) + OFF1[0]), f(f(pos[1] * fq) + OFF1[1]), f(f(pos[2] * fq) + OFF1[2]), k) * amp));
    for (const [fq, amp] of this.oct) out[2] = f(out[2] + f(noise1(V, f(f(pos[0] * fq) + OFF2[0]), f(f(pos[1] * fq) + OFF2[1]), f(f(pos[2] * fq) + OFF2[2]), k) * amp));
    return out;
  }
  _curlAnalytic(pos) {                                // FUN_1806929c0 / 180692380 / 1806926a0
    const V = this.noise.V, k = this.interp;
    const G = [[0, 0, 0], [0, 0, 0], [0, 0, 0]];
    for (const [fq, amp] of this.oct) {
      const q = [f(pos[0] * fq), f(pos[1] * fq), f(pos[2] * fq)];
      const qs = [q, q.map((c, i) => f(c + OFF1[i])), q.map((c, i) => f(c + OFF2[i]))];
      for (let s = 0; s < 3; s++) {
        const g = grad1(V, qs[s][0], qs[s][1], qs[s][2], k);
        for (let a = 0; a < 3; a++) G[s][a] = f(G[s][a] + f(g[a] * amp));
      }
    }
    const d2 = (a, b) => { const d = f(a - b); return f(d + d); };
    return [d2(G[2][1], G[1][2]), d2(G[0][2], G[2][0]), d2(G[1][0], G[0][1])];
  }
  _curlFD(pos) {                                      // FUN_180693cf0 / 180693630 / 180693990
    const V = this.noise.V, k = this.interp, h = FD_H;
    const [x, y, z] = pos;
    const pts = (off, list) => (fq) => list.map(([dx, dy, dz]) => [
      f(f(fq * f(x + dx)) + off[0]), f(f(fq * f(y + dy)) + off[1]), f(f(fq * f(z + dz)) + off[2])]);
    const Z3 = [0, 0, 0];
    const px = pts(Z3, [[0, 0, h], [0, 0, -h], [0, h, 0], [0, -h, 0]]);
    const py = pts(OFF1, [[h, 0, 0], [-h, 0, 0], [0, 0, h], [0, 0, -h]]);
    const pz = pts(OFF2, [[0, h, 0], [0, -h, 0], [h, 0, 0], [-h, 0, 0]]);
    const acc = (gen) => {
      const s = [0, 0, 0, 0];
      for (const [fq, amp] of this.oct) { const n = noise4(V, gen(fq), k); for (let i = 0; i < 4; i++) s[i] = f(s[i] + f(n[i] * amp)); }
      return s;
    };
    const X = acc(px), Y = acc(py), Zs = acc(pz);
    return [
      f(f(f(Zs[0] - Zs[1]) + f(Y[3] - Y[2])) * FD_INV),
      f(f(f(Zs[3] - Zs[2]) + f(X[0] - X[1])) * FD_INV),
      f(f(f(Y[0] - Y[1]) + f(X[3] - X[2])) * FD_INV),
    ];
  }
  _fakeFlow(pos) {                                    // FUN_1806918a0 / 180691660 / 180691780
    const V = this.noise.V, k = this.interp, e = f(FD_H * 2);
    const P4 = [[pos[0], pos[1], pos[2]], [f(pos[0] + e), pos[1], pos[2]], [pos[0], f(pos[1] + e), pos[2]], [pos[0], pos[1], f(pos[2] + e)]];
    const s = [0, 0, 0, 0];
    for (const [fq, amp] of this.oct) {
      const n = noise4(V, P4.map((p) => p.map((c, i) => f(f(c * fq) + OFF1[i]))), k);
      for (let i = 0; i < 4; i++) s[i] = f(s[i] + f(n[i] * amp));
    }
    return [f(FD_INV * f(s[1] - s[0])), f(FD_INV * f(s[2] - s[0])), f(FD_INV * f(s[3] - s[0]))];
  }
  sampleCurl(pos, time) {                             // descriptor vf2 mode 1 (FUN_18069de60)
    this._setTime(time);
    const p = this.p;
    if (p.FastFakeFlow) return this._fakeFlow(pos);
    const F = f(p.FlowFactor), D = f(p.DivergenceFactor);
    if (F === 1 && D === 0) return this._curlAnalytic(pos);
    const A = this._curlFD(pos), B = this._potential(pos);
    return A.map((a, i) => f(f(a * F) + f(f(B[i] - a) * D)));
  }
  samplePotential(pos, time) { this._setTime(time); return this._potential(pos); }
  sample(pos, time) { return this.curlDefault ? this.sampleCurl(pos, time) : this.samplePotential(pos, time); }
}

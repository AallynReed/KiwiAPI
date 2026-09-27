// Row-major 3x3 matrix and vec3 helpers for mesh particle orientation.

export const IDENT3 = [1, 0, 0, 0, 1, 0, 0, 0, 1];

export const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
export const add = (a, b) => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
export const mul = (a, s) => [a[0] * s, a[1] * s, a[2] * s];
export const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
export const norm = (a) => { const l = Math.hypot(a[0], a[1], a[2]) || 1; return [a[0] / l, a[1] / l, a[2] / l]; };
export const dist = (p, q) => Math.hypot(p[0] - q[0], p[1] - q[1], p[2] - q[2]);

export function mat3mul(a, b) {
  const o = new Array(9);
  for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) {
    o[i * 3 + j] = a[i * 3] * b[j] + a[i * 3 + 1] * b[3 + j] + a[i * 3 + 2] * b[6 + j];
  }
  return o;
}

export function eulerDeg(deg) {
  return eulerRad([(deg[0] || 0) * Math.PI / 180, (deg[1] || 0) * Math.PI / 180, (deg[2] || 0) * Math.PI / 180]);
}

// engine Euler builder (FUN_180609e80): yaw about Y first, then X, then Z: Rz * Rx * Ry
export function eulerRad(v) {
  const x = v[0] || 0, y = v[1] || 0, z = v[2] || 0;
  const Ry = [Math.cos(y), 0, Math.sin(y), 0, 1, 0, -Math.sin(y), 0, Math.cos(y)];
  const Rx = [1, 0, 0, 0, Math.cos(x), -Math.sin(x), 0, Math.sin(x), Math.cos(x)];
  const Rz = [Math.cos(z), -Math.sin(z), 0, Math.sin(z), Math.cos(z), 0, 0, 0, 1];
  return mat3mul(mat3mul(Rz, Rx), Ry);
}

export function axisAngle(axis, ang) {
  const l = Math.hypot(axis[0] || 0, axis[1] || 0, axis[2] || 0) || 1;
  const x = (axis[0] || 0) / l, y = (axis[1] || 0) / l, z = (axis[2] || 0) / l;
  const c = Math.cos(ang), s = Math.sin(ang), t = 1 - c;
  return [
    t * x * x + c, t * x * y - s * z, t * x * z + s * y,
    t * x * y + s * z, t * y * y + c, t * y * z - s * x,
    t * x * z - s * y, t * y * z + s * x, t * z * z + c,
  ];
}

// columns: X = right, Y = up, Z = forward. A near-zero axis falls back to the frame's
// forward (0,0,1) or up (0,1,0) (FUN_18060a2c0 / FUN_18060a3b0); any other is normalized.
export function basisFromForwardUp(fwd, up) {
  const unit = (v, d) => {
    const x = v[0] || 0, y = v[1] || 0, z = v[2] || 0, l2 = x * x + y * y + z * z;
    return l2 > 1e-13 ? [x / Math.sqrt(l2), y / Math.sqrt(l2), z / Math.sqrt(l2)] : d;
  };
  const f = unit(fwd, [0, 0, 1]);
  let r = cross(unit(up, [0, 1, 0]), f);
  const rl = Math.hypot(r[0], r[1], r[2]);
  r = rl > 1e-5 ? [r[0] / rl, r[1] / rl, r[2] / rl] : [1, 0, 0];
  const u = cross(f, r);
  return [r[0], u[0], f[0], r[1], u[1], f[1], r[2], u[2], f[2]];
}

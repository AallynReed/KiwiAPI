//! `render/voxel.py`'s rasterizer: the catalog camera, flat per-face shading,
//! a z-buffered opaque pass, a back-to-front glass composite and a 4x
//! supersample, in the same float64 operation order as the numba kernels.

use crate::blueprint::Rendered;

const AZ: f64 = 40.0;
const EL: f64 = 29.7;
const FOV_Y: f64 = 42.9;
const DIST: f64 = 2.55;
const SS: usize = 4;
const F_TOP: f64 = 1.109;
const F_X: f64 = 0.929;
const F_Z: f64 = 0.500;

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn unit(a: V3) -> V3 {
    let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    [a[0] / n, a[1] / n, a[2] / n]
}

struct Camera {
    eye: V3,
    rows: [V3; 3],
    focal: f64,
    cx: f64,
    cy: f64,
}

impl Camera {
    fn new(dim: usize) -> Self {
        let (a, e) = (AZ.to_radians(), EL.to_radians());
        let dir = [e.cos() * a.sin(), e.sin(), e.cos() * a.cos()];
        let eye = [dir[0] * DIST, dir[1] * DIST, dir[2] * DIST];
        let f = unit(sub([0.0; 3], eye));
        let s = unit(cross(f, [0.0, 1.0, 0.0]));
        let u = cross(s, f);
        let h = (dim * SS) as f64;
        let half = dim as f64 / 2.0 * SS as f64;
        Camera {
            eye,
            rows: [s, u, [-f[0], -f[1], -f[2]]],
            focal: (h * 0.5) / (FOV_Y.to_radians() * 0.5).tan(),
            cx: half,
            cy: half,
        }
    }

    /// `_project` of one point: screen (u, v) and depth.
    fn project(&self, p: V3) -> ([f64; 2], f64) {
        let d = sub(p, self.eye);
        let cam = self.rows.map(|r| d[0] * r[0] + d[1] * r[1] + d[2] * r[2]);
        let z = (-cam[2]).max(1e-6);
        ([self.cx + self.focal * cam[0] / z, self.cy - self.focal * cam[1] / z], z)
    }
}

/// `_fit_scale`: the catalog's empirical size law.
fn fit_scale(ext: V3) -> f64 {
    let mut d = ext;
    d.sort_by(|a, b| b.total_cmp(a));
    0.1057f64.exp() * d[0].powf(-0.8502) * d[1].powf(-0.0710) * d[2].powf(-0.0459)
}

/// `_fit_tight`: the scale at which the projected box fills the frame.
fn fit_tight(cam: &Camera, center: V3, mn: V3, mx: V3, h: f64, margin: f64) -> f64 {
    let mut corners = Vec::with_capacity(8);
    for a in [mn[0], mx[0] + 1.0] {
        for b in [mn[1], mx[1] + 1.0] {
            for c in [mn[2], mx[2] + 1.0] {
                corners.push([a, b, c]);
            }
        }
    }
    let target = h - 2.0 * margin;
    let (mut lo, mut hi) = (1e-4, 50.0);
    for _ in 0..40 {
        let s = (lo + hi) / 2.0;
        let (mut umin, mut umax, mut vmin, mut vmax) =
            (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
        for c in &corners {
            let ([u, v], _) = cam.project([(c[0] - center[0]) * s, (c[1] - center[1]) * s, (c[2] - center[2]) * s]);
            umin = umin.min(u);
            umax = umax.max(u);
            vmin = vmin.min(v);
            vmax = vmax.max(v);
        }
        if (umax - umin).max(vmax - vmin) <= target {
            lo = s;
        } else {
            hi = s;
        }
    }
    lo
}

struct Tri {
    xy: [[f64; 2]; 3],
    z: [f64; 3],
    rgb: V3,
}

fn bounds(t: &Tri, w: usize, h: usize) -> Option<(usize, usize, usize, usize, f64)> {
    let [[x0, y0], [x1, y1], [x2, y2]] = t.xy;
    let minx = (x0.min(x1.min(x2)).floor() as i64).max(0);
    let maxx = (x0.max(x1.max(x2)).ceil() as i64).min(w as i64 - 1);
    let miny = (y0.min(y1.min(y2)).floor() as i64).max(0);
    let maxy = (y0.max(y1.max(y2)).ceil() as i64).min(h as i64 - 1);
    if minx > maxx || miny > maxy {
        return None;
    }
    let d = (y1 - y2) * (x0 - x2) + (x1 - x2) * (y2 - y0);
    if d.abs() < 1e-9 {
        return None;
    }
    Some((minx as usize, maxx as usize, miny as usize, maxy as usize, d))
}

/// Every covered sample of a triangle with its interpolated depth.
fn cover(t: &Tri, w: usize, h: usize, mut each: impl FnMut(usize, f64)) {
    let Some((minx, maxx, miny, maxy, d)) = bounds(t, w, h) else { return };
    let [[x0, y0], [x1, y1], [x2, y2]] = t.xy;
    let [z0, z1, z2] = t.z;
    for py in miny..=maxy {
        let gy = py as f64 + 0.5;
        for px in minx..=maxx {
            let gx = px as f64 + 0.5;
            let a = ((y1 - y2) * (gx - x2) + (x2 - x1) * (gy - y2)) / d;
            let bb = ((y2 - y0) * (gx - x2) + (x0 - x2) * (gy - y2)) / d;
            let c = 1.0 - a - bb;
            if a >= 0.0 && bb >= 0.0 && c >= 0.0 {
                each(py * w + px, a * z0 + bb * z1 + c * z2);
            }
        }
    }
}

const OFFS: [([i64; 3], [[f64; 3]; 4]); 3] = [
    ([1, 0, 0], [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, 1.0, 1.0], [1.0, 0.0, 1.0]]),
    ([0, 1, 0], [[0.0, 1.0, 0.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 0.0]]),
    ([0, 0, 1], [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0]]),
];

fn opaque(kind: u8) -> bool {
    kind == 0 || kind == 2
}

/// `render_voxels(voxels, dim, {"fit": "tight"} if tight)`: `(dim, dim, 4)` RGBA.
/// `cells` is the dict in insertion order; `None` for an empty model.
pub fn render(cells: &[Rendered], dim: usize, tight: bool) -> Option<Vec<u8>> {
    if cells.is_empty() {
        return None;
    }
    let h = dim * SS;
    let cam = Camera::new(dim);
    let (mut mn, mut mx) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for c in cells {
        for (k, v) in [c.x, c.y, c.z].into_iter().enumerate() {
            mn[k] = mn[k].min(v as f64);
            mx[k] = mx[k].max(v as f64);
        }
    }
    let center = [0, 1, 2].map(|k| (mn[k] + mx[k] + 1.0) / 2.0);
    let s = if tight {
        fit_tight(&cam, center, mn, mx, h as f64, (4 * SS) as f64)
    } else {
        fit_scale([0, 1, 2].map(|k| mx[k] - mn[k] + 1.0))
    };

    let at: std::collections::HashMap<(i64, i64, i64), u8> = cells.iter().map(|c| ((c.x, c.y, c.z), c.kind)).collect();
    let factors = [F_X, F_TOP, F_Z];
    let mut solid: Vec<Tri> = Vec::new();
    let mut glass: Vec<(f64, Tri, f64)> = Vec::new();
    for c in cells {
        let mine = opaque(c.kind);
        for (k, (n, corners)) in OFFS.iter().enumerate() {
            if let Some(&nb) = at.get(&(c.x + n[0], c.y + n[1], c.z + n[2]))
                && (!mine || opaque(nb))
            {
                continue;
            }
            let mut xy = [[0.0; 2]; 4];
            let mut z = [0.0; 4];
            for (i, o) in corners.iter().enumerate() {
                let p = [c.x as f64 + o[0], c.y as f64 + o[1], c.z as f64 + o[2]];
                (xy[i], z[i]) = cam.project([(p[0] - center[0]) * s, (p[1] - center[1]) * s, (p[2] - center[2]) * s]);
            }
            let base = [f64::from(c.r), f64::from(c.g), f64::from(c.b)];
            let rgb = if c.kind >= 2 { base } else { base.map(|v| (v * factors[k]).min(255.0)) };
            let tris = [
                Tri { xy: [xy[0], xy[1], xy[2]], z: [z[0], z[1], z[2]], rgb },
                Tri { xy: [xy[0], xy[2], xy[3]], z: [z[0], z[2], z[3]], rgb },
            ];
            if c.kind == 1 || c.kind == 3 {
                let lv = f64::from(c.level) / 255.0;
                let alpha = lv * lv * 255.0;
                let depth = (((z[0] + z[1]) + z[2]) + z[3]) / 4.0;
                glass.extend(tris.into_iter().map(|t| (depth, t, alpha)));
            } else {
                solid.extend(tris);
            }
        }
    }
    glass.sort_by(|a, b| b.0.total_cmp(&a.0));

    let mut buf = vec![[0.0f64; 4]; h * h];
    let mut zbuf = vec![f64::INFINITY; h * h];
    for t in &solid {
        cover(t, h, h, |i, zz| {
            if zz < zbuf[i] {
                zbuf[i] = zz;
                buf[i] = [t.rgb[0], t.rgb[1], t.rgb[2], 255.0];
            }
        });
    }
    for (_, t, al) in &glass {
        let ga = al / 255.0;
        cover(t, h, h, |i, zz| {
            if zz < zbuf[i] {
                let p = &mut buf[i];
                p[0] = ga * t.rgb[0] + (1.0 - ga) * p[0];
                p[1] = ga * t.rgb[1] + (1.0 - ga) * p[1];
                p[2] = ga * t.rgb[2] + (1.0 - ga) * p[2];
                p[3] = al + (1.0 - ga) * p[3];
            }
        });
    }

    let mut out = vec![0u8; dim * dim * 4];
    for oy in 0..dim {
        for ox in 0..dim {
            let (mut asum, mut rgb) = (0.0f64, [0.0f64; 3]);
            for sy in 0..SS {
                for sx in 0..SS {
                    let p = buf[(oy * SS + sy) * h + ox * SS + sx];
                    asum += p[3];
                    for ch in 0..3 {
                        rgb[ch] += p[ch] * p[3];
                    }
                }
            }
            let px = &mut out[(oy * dim + ox) * 4..][..4];
            for ch in 0..3 {
                let v = if asum > 0.0 { rgb[ch] / asum.max(1e-6) } else { 0.0 };
                px[ch] = (v + 0.5).clamp(0.0, 255.0) as u8;
            }
            px[3] = (asum / (SS * SS) as f64 + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
    Some(out)
}

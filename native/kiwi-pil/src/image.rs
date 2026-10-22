//! Pillow's "L" and "RGBA" images and the `libImaging/Draw.c` primitives as an
//! `ImageDraw.Draw(rgba_image, "RGBA")` uses them: blend 0, so every primitive
//! writes the ink (alpha included) straight into the pixel.

/// An "L" image (a text mask).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Mask {
    pub fn new(width: usize, height: usize) -> Self {
        Mask { width, height, data: vec![0; width * height] }
    }

    pub fn row_mut(&mut self, y: usize) -> &mut [u8] {
        &mut self.data[y * self.width..(y + 1) * self.width]
    }
}

pub type Ink = [u8; 4];

/// An "RGBA" image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub width: usize,
    pub height: usize,
    pub data: Vec<Ink>,
}

pub(crate) fn div255(a: u32) -> u32 {
    let tmp = a + 128;
    ((tmp >> 8) + tmp) >> 8
}

pub(crate) fn blend(mask: u32, in1: u32, in2: u32) -> u8 {
    div255(in1 * (255 - mask) + in2 * mask) as u8
}

/// `ROUND_UP` / `ROUND_DOWN` on a C float: the positive branch adds in float.
pub(crate) fn round_up_f32(f: f32) -> i32 {
    if f >= 0.0 { ((f + 0.5f32) as f64).floor() as i32 } else { -((f as f64).abs() + 0.5).floor() as i32 }
}

pub(crate) fn round_down_f32(f: f32) -> i32 {
    if f >= 0.0 { ((f - 0.5f32) as f64).ceil() as i32 } else { -((f as f64).abs() - 0.5).ceil() as i32 }
}

pub(crate) fn round_up_f64(f: f64) -> i32 {
    if f >= 0.0 { (f + 0.5).floor() as i32 } else { -(f.abs() + 0.5).floor() as i32 }
}

pub(crate) fn round_down_f64(f: f64) -> i32 {
    if f >= 0.0 { (f - 0.5).ceil() as i32 } else { -(f.abs() - 0.5).ceil() as i32 }
}

#[derive(Clone, Copy, Debug)]
struct Edge {
    x0: i32,
    y0: i32,
    xmin: i32,
    ymin: i32,
    xmax: i32,
    ymax: i32,
    dx: f32,
}

fn edge(x0: i32, y0: i32, x1: i32, y1: i32) -> Edge {
    let (xmin, xmax) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    let (ymin, ymax) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
    let dx = if y0 == y1 { 0.0 } else { (x1 - x0) as f32 / (y1 - y0) as f32 };
    Edge { x0, y0, xmin, ymin, xmax, ymax, dx }
}

impl Rgba {
    /// `Image.new("RGBA", size, color)`.
    pub fn new(width: usize, height: usize, color: Ink) -> Self {
        Rgba { width, height, data: vec![color; width * height] }
    }

    fn point(&mut self, x: i32, y: i32, ink: Ink) {
        if x >= 0 && (x as usize) < self.width && y >= 0 && (y as usize) < self.height {
            self.data[y as usize * self.width + x as usize] = ink;
        }
    }

    /// `hline32`.
    fn hline(&mut self, mut x0: i32, y0: i32, mut x1: i32, ink: Ink) {
        let (w, h) = (self.width as i32, self.height as i32);
        if y0 < 0 || y0 >= h {
            return;
        }
        if x0 < 0 {
            x0 = 0;
        } else if x0 >= w {
            return;
        }
        if x1 < 0 {
            return;
        } else if x1 >= w {
            x1 = w - 1;
        }
        let row = y0 as usize * self.width;
        while x0 <= x1 {
            self.data[row + x0 as usize] = ink;
            x0 += 1;
        }
    }

    /// The Bresenham `line()` (end point excluded).
    fn line(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, ink: Ink) {
        let mut dy = y1 - y0;
        let ys;
        if dy < 0 {
            dy = -dy;
            ys = -1;
        } else if dy == 0 {
            if x1 > x0 {
                self.hline(x0, y0, x1 - 1, ink);
            } else if x0 > x1 {
                self.hline(x1 + 1, y0, x0, ink);
            }
            return;
        } else {
            ys = 1;
        }
        let mut dx = x1 - x0;
        let xs = if dx < 0 {
            dx = -dx;
            -1
        } else {
            1
        };
        if dx == 0 {
            for _ in 0..dy {
                self.point(x0, y0, ink);
                y0 += ys;
            }
        } else if dx > dy {
            let n = dx;
            dy += dy;
            let mut e = dy - dx;
            dx += dx;
            for _ in 0..n {
                self.point(x0, y0, ink);
                if e >= 0 {
                    y0 += ys;
                    e -= dx;
                }
                e += dy;
                x0 += xs;
            }
        } else {
            let n = dy;
            dx += dx;
            let mut e = dx - dy;
            dy += dy;
            for _ in 0..n {
                self.point(x0, y0, ink);
                if e >= 0 {
                    x0 += xs;
                    e -= dy;
                }
                e += dx;
                y0 += ys;
            }
        }
    }

    /// `polygon_generic` without alpha blending.
    fn polygon_generic(&mut self, e: &[Edge], ink: Ink) {
        if e.is_empty() {
            return;
        }
        let mut ymin = self.height as i32 - 1;
        let mut ymax = 0i32;
        let mut table: Vec<&Edge> = Vec::with_capacity(e.len());
        for ed in e {
            if ymin > ed.ymin {
                ymin = ed.ymin;
            }
            if ymax < ed.ymax {
                ymax = ed.ymax;
            }
            if ed.ymin == ed.ymax {
                self.hline(ed.xmin, ed.ymin, ed.xmax, ink);
                continue;
            }
            table.push(ed);
        }
        if ymin < 0 {
            ymin = 0;
        }
        if ymax > self.height as i32 {
            ymax = self.height as i32;
        }
        let at = |ed: &Edge, y: i32| (y - ed.y0) as f32 * ed.dx + ed.x0 as f32;
        let mut xx: Vec<f32> = Vec::with_capacity(table.len() * 2);
        while ymin <= ymax {
            xx.clear();
            for (i, current) in table.iter().enumerate() {
                if ymin >= current.ymin && ymin <= current.ymax {
                    xx.push(at(current, ymin));
                    if ymin == current.ymax && ymin < ymax {
                        let last = xx[xx.len() - 1];
                        xx.push(last);
                    } else if (ymin == current.ymin || ymin == current.ymax) && current.dx != 0.0 {
                        for other in &table[..i] {
                            if (ymin != other.ymin && ymin != other.ymax) || other.dx == 0.0 {
                                continue;
                            }
                            let last = xx.len() - 1;
                            if xx[last].round() == at(other, ymin).round() {
                                let offset = if ymin == current.ymax { -1 } else { 1 };
                                let adj = at(current, ymin + offset);
                                if ymin + offset >= other.ymin && ymin + offset <= other.ymax {
                                    let adj_other = at(other, ymin + offset);
                                    if xx[last] > adj + 1.0 && xx[last] > adj_other + 1.0 {
                                        xx[last] = ((adj as f64).max(adj_other as f64) as f32).round() + 1.0;
                                    } else if xx[last] < adj - 1.0 && xx[last] < adj_other - 1.0 {
                                        xx[last] = ((adj as f64).min(adj_other as f64) as f32).round() - 1.0;
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            xx.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mut i = 1;
            while i < xx.len() {
                self.hline(round_up_f32(xx[i - 1]), ymin, round_down_f32(xx[i]), ink);
                i += 2;
            }
            ymin += 1;
        }
    }

    /// `ImagingDrawWideLine`.
    pub fn wide_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, ink: Ink, width: i32) {
        let dx = x1 - x0;
        let dy = y1 - y0;
        if dx == 0 && dy == 0 {
            self.point(x0, y0, ink);
            return;
        }
        let big = (dx as f64).hypot(dy as f64);
        let small = (width - 1) as f64 / 2.0;
        let ratio_max = round_up_f64(small) as f64 / big;
        let ratio_min = round_down_f64(small) as f64 / big;
        let dxmin = round_down_f64(ratio_min * dy as f64);
        let dxmax = round_down_f64(ratio_max * dy as f64);
        let dymin = round_down_f64(ratio_min * dx as f64);
        let dymax = round_down_f64(ratio_max * dx as f64);
        let v =
            [(x0 - dxmin, y0 + dymax), (x1 - dxmin, y1 + dymax), (x1 + dxmax, y1 - dymin), (x0 + dxmax, y0 - dymin)];
        let e = [
            edge(v[0].0, v[0].1, v[1].0, v[1].1),
            edge(v[1].0, v[1].1, v[2].0, v[2].1),
            edge(v[2].0, v[2].1, v[3].0, v[3].1),
            edge(v[3].0, v[3].1, v[0].0, v[0].1),
        ];
        self.polygon_generic(&e, ink);
    }

    /// `draw.draw_lines(xy, ink, width)`: coordinates truncate like `(int)`.
    pub fn draw_lines(&mut self, xy: &[(f64, f64)], ink: Ink, width: i32) {
        let p: Vec<(i32, i32)> = xy.iter().map(|&(x, y)| (x as i32, y as i32)).collect();
        if width == 1 {
            for w in p.windows(2) {
                self.line(w[0].0, w[0].1, w[1].0, w[1].1, ink);
            }
            if p.len() >= 2 {
                let last = p[p.len() - 1];
                self.point(last.0, last.1, ink);
            }
        } else {
            for w in p.windows(2) {
                self.wide_line(w[0].0, w[0].1, w[1].0, w[1].1, ink, width);
            }
        }
    }

    /// `ImagingDrawRectangle`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_rectangle(&mut self, x0: i32, mut y0: i32, x1: i32, mut y1: i32, ink: Ink, fill: bool, mut width: i32) {
        if y0 > y1 {
            std::mem::swap(&mut y0, &mut y1);
        }
        if fill {
            if y0 < 0 {
                y0 = 0;
            } else if y0 >= self.height as i32 {
                return;
            }
            if y1 < 0 {
                return;
            } else if y1 > self.height as i32 {
                y1 = self.height as i32;
            }
            for y in y0..=y1 {
                self.hline(x0, y, x1, ink);
            }
        } else {
            if width == 0 {
                width = 1;
            }
            for i in 0..width {
                self.hline(x0, y0 + i, x1, ink);
                self.hline(x0, y1 - i, x1, ink);
                self.line(x1 - i, y0 + width, x1 - i, y1 - width + 1, ink);
                self.line(x0 + i, y0 + width, x0 + i, y1 - width + 1, ink);
            }
        }
    }

    /// `ImagingDrawPolygon` with `fill`.
    pub fn draw_polygon_fill(&mut self, xy: &[(i32, i32)], ink: Ink) {
        let count = xy.len();
        if count == 0 {
            return;
        }
        let mut e: Vec<Edge> = Vec::with_capacity(count);
        let mut i = 0;
        while i + 1 < count {
            let (x0, y0) = xy[i];
            let (x1, y1) = xy[i + 1];
            if y0 == y1 && i != 0 && y0 == xy[i - 1].1 {
                let prev_x = xy[i - 1].0;
                if let Some(last) = e.last_mut() {
                    if x1 > x0 && x0 > prev_x {
                        last.xmax = x1;
                        i += 1;
                        continue;
                    } else if x1 < x0 && x0 < prev_x {
                        last.xmin = x1;
                        i += 1;
                        continue;
                    }
                }
            }
            e.push(edge(x0, y0, x1, y1));
            i += 1;
        }
        if xy[i] != xy[0] {
            e.push(edge(xy[i].0, xy[i].1, xy[0].0, xy[0].1));
        }
        self.polygon_generic(&e, ink);
    }

    fn ellipse_spans(&mut self, x0: i32, y0: i32, a: i32, b: i32, st: &mut EllipseState, ink: Ink) {
        while let Some((cx0, cy, cx1)) = st.next() {
            self.hline(x0 + (cx0 + a) / 2, y0 + (cy + b) / 2, x0 + (cx1 + a) / 2, ink);
        }
    }

    /// `ellipseNew` (`ImagingDrawEllipse`).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_ellipse(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, ink: Ink, fill: bool, mut width: i32) {
        let a = x1 - x0;
        let b = y1 - y0;
        if a < 0 || b < 0 {
            return;
        }
        if fill {
            width = a + b;
        }
        let mut st = EllipseState::new(a, b, width);
        self.ellipse_spans(x0, y0, a, b, &mut st, ink);
    }

    fn clip_ellipse(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, mut st: ClipEllipse, ink: Ink) {
        let a = x1 - x0;
        let b = y1 - y0;
        if a < 0 || b < 0 {
            return;
        }
        while let Some((cx0, cy, cx1)) = st.next() {
            self.hline(x0 + (cx0 + a) / 2, y0 + (cy + b) / 2, x0 + (cx1 + a) / 2, ink);
        }
    }

    /// `ImagingDrawArc`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_arc(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, mut start: f32, mut end: f32, ink: Ink, width: i32) {
        normalize_angles(&mut start, &mut end);
        if start + 360.0 == end {
            self.draw_ellipse(x0, y0, x1, y1, ink, false, width);
            return;
        }
        if start == end {
            return;
        }
        let (a, b) = (x1 - x0, y1 - y0);
        if a < 0 || b < 0 {
            return;
        }
        let st = ClipEllipse::arc(a, b, width, start, end);
        self.clip_ellipse(x0, y0, x1, y1, st, ink);
    }

    /// `ImagingDrawPieslice` with `fill`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_pieslice_fill(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, mut start: f32, mut end: f32, ink: Ink) {
        normalize_angles(&mut start, &mut end);
        if start + 360.0 == end {
            self.draw_ellipse(x0, y0, x1, y1, ink, true, 0);
            return;
        }
        if start == end {
            return;
        }
        let (a, b) = (x1 - x0, y1 - y0);
        if a < 0 || b < 0 {
            return;
        }
        let st = ClipEllipse::pie(a, b, x1 + y1 - x0 - y0, start, end);
        self.clip_ellipse(x0, y0, x1, y1, st, ink);
    }

    /// `draw_bitmap` (`ImagingFill2` + `fill_mask_L` on an image with alpha).
    pub fn draw_bitmap(&mut self, dx0: i32, dy0: i32, mask: &Mask, ink: Ink) {
        let mut xsize = mask.width as i64;
        let mut ysize = mask.height as i64;
        let (mut dx0, mut dy0) = (dx0 as i64, dy0 as i64);
        let (mut sx0, mut sy0) = (0i64, 0i64);
        if dx0 < 0 {
            xsize += dx0;
            if xsize <= 0 {
                return;
            }
            sx0 = -dx0;
            dx0 = 0;
        }
        if dx0 + xsize > self.width as i64 {
            xsize = self.width as i64 - dx0;
        }
        if dy0 < 0 {
            ysize += dy0;
            if ysize <= 0 {
                return;
            }
            sy0 = -dy0;
            dy0 = 0;
        }
        if dy0 + ysize > self.height as i64 {
            ysize = self.height as i64 - dy0;
        }
        if xsize <= 0 || ysize <= 0 {
            return;
        }
        for y in 0..ysize {
            let orow = (y + dy0) as usize * self.width;
            let mrow = (y + sy0) as usize * mask.width;
            for x in 0..xsize {
                let m = mask.data[mrow + (x + sx0) as usize];
                let out = &mut self.data[orow + (x + dx0) as usize];
                for i in 0..4 {
                    let mut cm = m as i32;
                    if i != 3 && cm != 0 {
                        cm = (255 - (255 - cm) * (1 - (255 - out[3] as i32) / 255)) & 0xff;
                    }
                    out[i] = blend(cm as u32, out[i] as u32, ink[i] as u32);
                }
            }
        }
    }

    /// `im.convert("RGB")`: drops alpha.
    pub fn to_rgb(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len() * 3);
        for p in &self.data {
            out.extend_from_slice(&p[..3]);
        }
        out
    }
}

/// `normalize_angles`: 0 <= al < 360, al <= ar <= al + 360.
pub(crate) fn normalize_angles(al: &mut f32, ar: &mut f32) {
    if *ar - *al >= 360.0 {
        *al = 0.0;
        *ar = 360.0;
    } else {
        let a = if *al < 0.0 { 360.0 - ((-*al) as f64 % 360.0) } else { *al as f64 };
        *al = (a % 360.0) as f32;
        let d = if *ar < *al { 360.0 - ((*al - *ar) as f64 % 360.0) } else { (*ar - *al) as f64 };
        *ar = (*al as f64 + d % 360.0) as f32;
    }
}

struct Quarter {
    cx: i32,
    cy: i32,
    ex: i32,
    ey: i32,
    a2: i64,
    b2: i64,
    a2b2: i64,
    finished: bool,
}

impl Quarter {
    fn new(a: i32, b: i32) -> Self {
        if a < 0 || b < 0 {
            return Quarter { cx: 0, cy: 0, ex: 0, ey: 0, a2: 0, b2: 0, a2b2: 0, finished: true };
        }
        let a2 = a as i64 * a as i64;
        let b2 = b as i64 * b as i64;
        Quarter { cx: a, cy: b % 2, ex: a % 2, ey: b, a2, b2, a2b2: a2 * b2, finished: false }
    }

    fn delta(&self, x: i64, y: i64) -> i64 {
        (self.a2 * y * y + self.b2 * x * x - self.a2b2).abs()
    }

    fn next(&mut self) -> Option<(i32, i32)> {
        if self.finished {
            return None;
        }
        let ret = (self.cx, self.cy);
        if self.cx == self.ex && self.cy == self.ey {
            self.finished = true;
        } else {
            let mut nx = self.cx;
            let mut ny = self.cy + 2;
            let mut nd = self.delta(nx as i64, ny as i64);
            if nx > 1 {
                let d = self.delta(self.cx as i64 - 2, self.cy as i64 + 2);
                if nd > d {
                    nx = self.cx - 2;
                    ny = self.cy + 2;
                    nd = d;
                }
                let d = self.delta(self.cx as i64 - 2, self.cy as i64);
                if nd > d {
                    nx = self.cx - 2;
                    ny = self.cy;
                }
            }
            self.cx = nx;
            self.cy = ny;
        }
        Some(ret)
    }
}

struct EllipseState {
    o: Quarter,
    i: Quarter,
    py: i32,
    pl: i32,
    pr: i32,
    buf: Vec<(i32, i32, i32)>,
    finished: bool,
    leftmost: i32,
}

impl EllipseState {
    fn new(a: i32, b: i32, w: i32) -> Self {
        let leftmost = a % 2;
        let mut o = Quarter::new(a, b);
        let mut s = EllipseState {
            o: Quarter::new(-1, -1),
            i: Quarter::new(-1, -1),
            py: 0,
            pl: 0,
            pr: 0,
            buf: Vec::with_capacity(4),
            finished: true,
            leftmost,
        };
        if w < 1 {
            s.o = o;
            return s;
        }
        match o.next() {
            None => {
                s.o = o;
            }
            Some((pr, py)) => {
                s.pr = pr;
                s.py = py;
                s.finished = false;
                s.i = Quarter::new(a - 2 * (w - 1), b - 2 * (w - 1));
                s.pl = leftmost;
                s.o = o;
            }
        }
        s
    }

    /// One horizontal segment `(x0, y, x1)` in the doubled grid.
    fn next(&mut self) -> Option<(i32, i32, i32)> {
        if self.buf.is_empty() {
            if self.finished {
                return None;
            }
            let y = self.py;
            let mut l = self.pl;
            let r = self.pr;
            let mut last = None;
            while let Some((cx, cy)) = self.o.next() {
                if cy > y {
                    last = Some((cx, cy));
                    break;
                }
            }
            match last {
                None => self.finished = true,
                Some((cx, cy)) => {
                    self.pr = cx;
                    self.py = cy;
                }
            }
            let mut inner = None;
            while let Some((cx, cy)) = self.i.next() {
                if cy > y {
                    inner = Some(cx);
                    break;
                }
                l = cx;
            }
            self.pl = inner.unwrap_or(self.leftmost);
            // Pushed in this order, popped from the back.
            if (l > 0 || l < r) && y > 0 {
                self.buf.push((if l == 0 { 2 } else { l }, y, r));
            }
            if y > 0 {
                self.buf.push((-r, y, -l));
            }
            if l > 0 || l < r {
                self.buf.push((if l == 0 { 2 } else { l }, -y, r));
            }
            self.buf.push((-r, -y, -l));
        }
        self.buf.pop()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClipType {
    And,
    Or,
    Clip,
}

#[derive(Clone, Copy)]
struct ClipNode {
    kind: ClipType,
    a: f64,
    b: f64,
    c: f64,
    l: Option<usize>,
    r: Option<usize>,
}

impl ClipNode {
    fn clip(a: f64, b: f64, c: f64) -> Self {
        ClipNode { kind: ClipType::Clip, a, b, c, l: None, r: None }
    }

    fn join(kind: ClipType, l: usize, r: usize) -> Self {
        ClipNode { kind, a: 0.0, b: 0.0, c: 0.0, l: Some(l), r: Some(r) }
    }
}

struct ClipEllipse {
    st: EllipseState,
    nodes: Vec<ClipNode>,
    root: Option<usize>,
    pending: std::collections::VecDeque<i32>,
    y: i32,
}

impl ClipEllipse {
    fn do_clip(&self, node: Option<usize>, mut x0: i32, y: i32, mut x1: i32) -> Vec<(i32, i8)> {
        let Some(n) = node else { return vec![(x0, 1), (x1, -1)] };
        let nd = self.nodes[n];
        match nd.kind {
            ClipType::Clip => {
                let eps = 1e-9;
                let (a, b, c) = (nd.a, nd.b, nd.c);
                if a.abs() < eps {
                    if b * y as f64 + c < -eps {
                        x0 = 1;
                        x1 = 0;
                    }
                } else {
                    let ix = -(b * y as f64 + c) / a;
                    if a * x0 as f64 + b * y as f64 + c < eps {
                        x0 = (x0 as f64).max(ix).round() as i32;
                    }
                    if a * x1 as f64 + b * y as f64 + c < eps {
                        x1 = (x1 as f64).min(ix).round() as i32;
                    }
                }
                if x0 <= x1 { vec![(x0, 1), (x1, -1)] } else { Vec::new() }
            }
            ClipType::And | ClipType::Or => {
                let l1 = self.do_clip(nd.l, x0, y, x1);
                let l2 = self.do_clip(nd.r, x0, y, x1);
                let (mut i1, mut i2) = (0usize, 0usize);
                let (mut k1, mut k2) = (0i32, 0i32);
                let mut out: Vec<(i32, i8)> = Vec::new();
                while i1 < l1.len() || i2 < l2.len() {
                    let take_l1 = i2 >= l2.len()
                        || (i1 < l1.len() && (l1[i1].0 < l2[i2].0 || (l1[i1].0 == l2[i2].0 && l1[i1].1 > l2[i2].1)));
                    let t = if take_l1 {
                        let t = l1[i1];
                        k1 += t.1 as i32;
                        i1 += 1;
                        t
                    } else {
                        let t = l2[i2];
                        k2 += t.1 as i32;
                        i2 += 1;
                        t
                    };
                    let tail = out.last().map(|e| e.1);
                    let keep = if nd.kind == ClipType::Or {
                        (t.1 == 1 && tail.is_none_or(|tt| tt == -1)) || (t.1 == -1 && k1 == 0 && k2 == 0)
                    } else {
                        (t.1 == 1 && tail.is_none_or(|tt| tt == -1) && k1 > 0 && k2 > 0)
                            || (t.1 == -1 && tail == Some(1) && (k1 == 0 || k2 == 0))
                    };
                    if keep {
                        out.push(t);
                    }
                }
                out
            }
        }
    }

    fn transpose(&mut self) {
        for n in &mut self.nodes {
            if n.kind == ClipType::Clip {
                std::mem::swap(&mut n.a, &mut n.b);
            }
        }
    }

    /// `arc_init`.
    fn arc(a: i32, b: i32, w: i32, al: f32, ar: f32) -> Self {
        if a < b {
            let mut s = ClipEllipse::arc(b, a, w, 90.0 - ar, 90.0 - al);
            s.st = EllipseState::new(a, b, w);
            s.transpose();
            return s;
        }
        let (mut al, mut ar) = (al, ar);
        let st = EllipseState::new(a, b, w);
        normalize_angles(&mut al, &mut ar);
        let mut s = ClipEllipse { st, nodes: Vec::with_capacity(7), root: None, pending: Default::default(), y: 0 };
        if ar == al + 360.0 {
            return s;
        }
        let (af, bf) = (a as f64, b as f64);
        let (ald, ard) = (al as f64, ar as f64);
        let pi = std::f64::consts::PI;
        s.nodes.push(ClipNode::clip(
            -af * (ald * pi / 180.0).sin(),
            bf * (ald * pi / 180.0).cos(),
            (a * a - b * b) as f64 * (ald * pi / 90.0).sin() / 2.0,
        ));
        s.nodes.push(ClipNode::clip(
            af * (ard * pi / 180.0).sin(),
            -bf * (ard * pi / 180.0).cos(),
            (b * b - a * a) as f64 * (ard * pi / 90.0).sin() / 2.0,
        ));
        let (lc, rc) = (0, 1);
        if ald % 180.0 == 0.0 || ard % 180.0 == 0.0 {
            let kind = if ar - al < 180.0 { ClipType::And } else { ClipType::Or };
            s.nodes.push(ClipNode::join(kind, lc, rc));
            s.root = Some(2);
        } else if (((al / 180.0) as i32) + ((ar / 180.0) as i32)) % 2 == 1 {
            let lb = if ((al / 180.0) as i32) % 2 == 0 { 1.0 } else { -1.0 };
            let rb = if ((ar / 180.0) as i32) % 2 == 0 { 1.0 } else { -1.0 };
            s.nodes.push(ClipNode::clip(0.0, lb, 0.0));
            s.nodes.push(ClipNode::join(ClipType::And, 2, lc));
            s.nodes.push(ClipNode::clip(0.0, rb, 0.0));
            s.nodes.push(ClipNode::join(ClipType::And, 4, rc));
            s.nodes.push(ClipNode::join(ClipType::Or, 3, 5));
            s.root = Some(6);
        } else {
            let kind = if ar - al < 180.0 { ClipType::And } else { ClipType::Or };
            let rb = if (180.0..=540.0).contains(&ar) { -1.0 } else { 1.0 };
            s.nodes.push(ClipNode::join(kind, lc, rc));
            s.nodes.push(ClipNode::clip(0.0, rb, 0.0));
            s.nodes.push(ClipNode::join(kind, 2, 3));
            s.root = Some(4);
        }
        s
    }

    /// `pie_init`.
    fn pie(a: i32, b: i32, w: i32, al: f32, ar: f32) -> Self {
        let st = EllipseState::new(a, b, w);
        let mut s = ClipEllipse { st, nodes: Vec::with_capacity(7), root: None, pending: Default::default(), y: 0 };
        let pi = std::f64::consts::PI;
        let (af, bf, ald, ard) = (a as f64, b as f64, al as f64, ar as f64);
        let xl = af * (ald * pi / 180.0).cos();
        let xr = af * (ard * pi / 180.0).cos();
        let yl = bf * (ald * pi / 180.0).sin();
        let yr = bf * (ard * pi / 180.0).sin();
        s.nodes.push(ClipNode::clip(-yl, xl, 0.0));
        s.nodes.push(ClipNode::clip(yr, -xr, 0.0));
        let kind = if ar - al < 180.0 { ClipType::And } else { ClipType::Or };
        s.nodes.push(ClipNode::join(kind, 0, 1));
        s.root = Some(2);
        if ar - al < 90.0 {
            s.nodes.push(ClipNode::clip((xl + xr) / 2.0, (yl + yr) / 2.0, 0.0));
            s.nodes.push(ClipNode::join(ClipType::And, 2, 3));
            s.root = Some(4);
        }
        s
    }

    /// `clip_ellipse_next`.
    fn next(&mut self) -> Option<(i32, i32, i32)> {
        while self.pending.is_empty() {
            let (x0, y, x1) = self.st.next()?;
            let events = self.do_clip(self.root, x0, y, x1);
            self.pending.extend(events.into_iter().map(|e| e.0));
            self.y = y;
        }
        let x0 = self.pending.pop_front()?;
        let x1 = self.pending.pop_front()?;
        Some((x0, self.y, x1))
    }
}

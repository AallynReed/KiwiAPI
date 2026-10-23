//! pyclipper 1.4.0's Clipper 6.4.2 (vendored C++, `csrc/clipper`): the
//! round-joined closed offset RapidOCR's `unclip` runs.

unsafe extern "C" {
    fn kiwi_clipper_offset_round(
        xy: *const i64,
        n: usize,
        delta: f64,
        out: *mut i64,
        out_cap: usize,
        lens: *mut usize,
        lens_cap: usize,
        npaths: *mut usize,
    ) -> i64;
}

/// `PyclipperOffset().AddPath(path, JT_ROUND, ET_CLOSEDPOLYGON); Execute(delta)`.
/// `path` holds the already-integer coordinates; `None` if Clipper threw.
pub fn offset_round(path: &[(i64, i64)], delta: f64) -> Option<Vec<Vec<(i64, i64)>>> {
    let xy: Vec<i64> = path.iter().flat_map(|&(x, y)| [x, y]).collect();
    let (mut cap, mut lens_cap) = (256usize, 4usize);
    loop {
        let mut out = vec![0i64; cap * 2];
        let mut lens = vec![0usize; lens_cap];
        let mut npaths = 0usize;
        // SAFETY: every buffer is sized as passed; the callee writes within the caps.
        let total = unsafe {
            kiwi_clipper_offset_round(
                xy.as_ptr(),
                path.len(),
                delta,
                out.as_mut_ptr(),
                cap,
                lens.as_mut_ptr(),
                lens_cap,
                &mut npaths,
            )
        };
        if total < 0 {
            return None;
        }
        let total = total as usize;
        if total > cap || npaths > lens_cap {
            cap = cap.max(total);
            lens_cap = lens_cap.max(npaths);
            continue;
        }
        let mut paths = Vec::with_capacity(npaths);
        let mut k = 0;
        for &len in &lens[..npaths] {
            paths.push((0..len).map(|i| (out[2 * (k + i)], out[2 * (k + i) + 1])).collect());
            k += len;
        }
        return Some(paths);
    }
}

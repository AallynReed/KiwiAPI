use super::{PackError, pack, pillow, raster};
use crate::{
    blueprint::{Rendered, Voxel},
    bpedit::codec,
};

#[test]
fn pack_ships_spec_only_when_used() {
    let v = |w, ty| Voxel { x: 0, y: 0, z: 0, r: 200, g: 180, b: 90, w, ty };
    let plain = codec::encode(&[v(0, 21)], 3, None, &[], [0; 3], None).unwrap();
    assert!(pack(&plain, "a", 10).unwrap().get("spec").is_none());
    let metal = codec::encode(&[v(1, 21)], 3, None, &[], [0; 3], None).unwrap();
    assert_eq!(pack(&metal, "a", 10).unwrap()["spec"], serde_json::json!([1]));
    assert_eq!(pack(b"kiwib\x03\0\0\0\0", "a", 10), Err(PackError::Empty));
    assert_eq!(pack(&metal, "a", 0), Err(PackError::TooLarge(1)));
}

#[test]
fn raster_draws_and_culls() {
    let cube = |x, kind| Rendered { x, y: 0, z: 0, r: 255, g: 0, b: 0, kind, level: 255, spec: 0 };
    let img = raster::render(&[cube(0, 0)], 64, false).unwrap();
    assert_eq!(img.len(), 64 * 64 * 4);
    assert!(img.chunks(4).any(|p| p[3] == 255));
    assert!(raster::render(&[], 64, true).is_none());
    let glass = raster::render(&[Rendered { level: 144, ..cube(0, 1) }], 64, true).unwrap();
    assert!(glass.chunks(4).all(|p| p[3] < 255) && glass.chunks(4).any(|p| p[3] > 0));
}

#[test]
fn lanczos_keeps_flat_colour() {
    let img = pillow::Image { w: 7, h: 5, px: [10, 20, 30, 255].repeat(35) };
    let out = pillow::resize_lanczos(&img, 3, 9);
    assert_eq!((out.w, out.h), (3, 9));
    assert!(out.px.chunks(4).all(|p| p == [10, 20, 30, 255]));
}

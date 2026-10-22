//! Pixel-exact Pillow 13 drawing for the server-rendered PNGs: FreeType 2.14.3
//! (vendored, the version Pillow's wheels bundle) behind Pillow's basic text
//! layout, the `libImaging/Draw.c` primitives, `ImagingFill2` text blending and
//! the PNG encoder's filter choice. Only what the Python renderers call.

pub mod draw;
pub mod font;
pub mod ft;
pub mod image;
pub mod png;
#[cfg(test)]
mod tests;

pub use draw::Draw;
pub use font::Font;
pub use ft::{FtError, Library};
pub use image::{Ink, Mask, Rgba};

/// What Pillow would raise: a `ValueError` from a primitive or an `OSError`
/// from FreeType.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageError {
    Value(&'static str),
    Ft(FtError),
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageError::Value(m) => f.write_str(m),
            ImageError::Ft(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ImageError {}

impl From<FtError> for ImageError {
    fn from(e: FtError) -> Self {
        ImageError::Ft(e)
    }
}

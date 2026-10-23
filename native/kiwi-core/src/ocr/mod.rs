//! The character-stat OCR of `app/trove/ocr` without onnxruntime: RapidOCR's
//! pre/post-processing (OpenCV, NumPy, pyclipper, shapely and Pillow
//! semantics reproduced exactly) and the stat parser. The three model runs
//! come in through [`rapid::Infer`].

pub mod clipper;
pub mod cv;
pub mod decode;
pub mod npsort;
pub mod pil;
pub mod rapid;
pub mod text;

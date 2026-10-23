//! The three onnxruntime sessions, loaded once on first use like
//! `engine._get_engine()`, with RapidOCR's session options.
//!
//! `KIWI_ORT_DYLIB` names the onnxruntime shared library (the Python wheel's
//! `libonnxruntime.so.1.26.0`, so both servers run the same build) and
//! `KIWI_OCR_MODEL_DIR` the folder holding RapidOCR's three ONNX files.

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use kiwi_core::ocr::rapid::Infer;
use ort::{
    ep,
    logging::LogLevel,
    session::{Session, builder::GraphOptimizationLevel},
    value::Tensor,
};

pub const DET: &str = "ch_PP-OCRv4_det_mobile.onnx";
pub const CLS: &str = "ch_ppocr_mobile_v2.0_cls_mobile.onnx";
pub const REC: &str = "ch_PP-OCRv4_rec_mobile.onnx";

const DEFAULT_DYLIB: &str = "/usr/local/lib/python3.13/site-packages/onnxruntime/capi/libonnxruntime.so.1.26.0";
const DEFAULT_MODELS: &str = "/usr/local/lib/python3.13/site-packages/rapidocr/models";

pub struct OrtEngine {
    det: Mutex<Session>,
    cls: Mutex<Session>,
    rec: Mutex<Session>,
    chars: Vec<String>,
}

/// `str.splitlines()`.
pub fn splitlines(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(std::mem::take(&mut cur));
            }
            '\n' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn session(path: &Path) -> Result<Session, String> {
    Session::builder()
        .map_err(|e| e.to_string())?
        .with_optimization_level(GraphOptimizationLevel::All)
        .map_err(|e| e.to_string())?
        .with_log_level(LogLevel::Fatal)
        .map_err(|e| e.to_string())?
        .with_execution_providers([ep::CPU::default().with_arena_allocator(false).build()])
        .map_err(|e| e.to_string())?
        .commit_from_file(path)
        .map_err(|e| e.to_string())
}

impl OrtEngine {
    pub fn load(dylib: &Path, models: &Path) -> Result<OrtEngine, String> {
        static INIT: OnceLock<Result<(), String>> = OnceLock::new();
        INIT.get_or_init(|| {
            ort::init_from(dylib).map_err(|e| e.to_string())?.commit();
            Ok(())
        })
        .clone()?;
        let rec = session(&models.join(REC))?;
        let meta = rec.metadata().map_err(|e| e.to_string())?;
        let chars = splitlines(&meta.custom("character").ok_or("rec model has no character list")?);
        drop(meta);
        Ok(OrtEngine {
            det: Mutex::new(session(&models.join(DET))?),
            cls: Mutex::new(session(&models.join(CLS))?),
            rec: Mutex::new(rec),
            chars,
        })
    }
}

/// The process-wide engine; `None` when the library or a model is missing.
pub fn get() -> Option<&'static OrtEngine> {
    static ENGINE: OnceLock<Option<OrtEngine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let env =
                |k: &str, d: &str| PathBuf::from(std::env::var(k).ok().filter(|v| !v.is_empty()).unwrap_or(d.into()));
            match OrtEngine::load(&env("KIWI_ORT_DYLIB", DEFAULT_DYLIB), &env("KIWI_OCR_MODEL_DIR", DEFAULT_MODELS)) {
                Ok(e) => Some(e),
                Err(e) => {
                    tracing::error!("ocr engine unavailable: {e}");
                    None
                }
            }
        })
        .as_ref()
}

fn run(s: &Mutex<Session>, shape: [usize; 4], input: Vec<f32>) -> Result<(Vec<usize>, Vec<f32>), String> {
    let t = Tensor::from_array((shape, input)).map_err(|e| e.to_string())?;
    let mut s = s.lock().map_err(|_| "poisoned session".to_string())?;
    let out = s.run(ort::inputs![t]).map_err(|e| e.to_string())?;
    let (shape, data) = out[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
    Ok((shape.iter().map(|&d| d as usize).collect(), data.to_vec()))
}

impl Infer for OrtEngine {
    fn det(&self, input: Vec<f32>, h: usize, w: usize) -> Result<Vec<f32>, String> {
        run(&self.det, [1, 3, h, w], input).map(|(_, d)| d)
    }

    fn cls(&self, input: Vec<f32>, n: usize) -> Result<Vec<f32>, String> {
        run(&self.cls, [n, 3, 48, 192], input).map(|(_, d)| d)
    }

    fn rec(&self, input: Vec<f32>, n: usize, w: usize) -> Result<(Vec<f32>, usize, usize), String> {
        let (shape, d) = run(&self.rec, [n, 3, 48, w], input)?;
        match shape[..] {
            [_, t, c] => Ok((d, t, c)),
            _ => Err(format!("unexpected rec output shape {shape:?}")),
        }
    }

    fn characters(&self) -> &[String] {
        &self.chars
    }
}

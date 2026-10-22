//! JSON exactly as Python writes it. Two flavours:
//! - `dumps`: Starlette `JSONResponse` = `json.dumps(ensure_ascii=False,
//!   allow_nan=False, separators=(",", ":"))`, Python float repr.
//! - `pydantic`: pydantic-core `dump_json` for `response_model` routes.
//!
//! (Foundation wave fills these in; until then they are serde_json.)

use serde_json::Value;

pub fn dumps(value: &Value) -> String {
    value.to_string()
}

pub fn pydantic(value: &Value) -> String {
    value.to_string()
}

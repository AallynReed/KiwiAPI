//! The `/v1` response models of `app/trove/schemas.py` (`Activity*`,
//! `ClassActivity*`): validate a payload the way `Model(**payload)` does and
//! return what pydantic dumps (declared fields, declared order, floats as
//! floats). `None` is the validation error Python answers with a 500.

use serde_json::{Value, json};

use crate::validation::{Field, Input, Model, Validator};

fn project(payload: &Value, f: impl FnOnce(&mut Model<'_>) -> Value) -> Option<Value> {
    let mut v = Validator::default();
    let out = Field::new(&mut v, vec![], Input::Json(payload)).model(f).required();
    v.is_ok().then_some(out)
}

/// `ActivityResponse`.
pub fn activity(payload: &Value) -> Option<Value> {
    project(payload, |m| {
        json!({
            "window_start": m.field("window_start").int().nullable(),
            "window_end": m.field("window_end").int().nullable(),
            "duration_hours": m.field("duration_hours").float().nullable(),
            "estimate": m.field("estimate").int().nullable(),
            "estimate_24h": m.field("estimate_24h").int().optional(),
            "estimate_7d": m.field("estimate_7d").int().optional(),
            "window_24h_start": m.field("window_24h_start").int().optional(),
            "window_7d_start": m.field("window_7d_start").int().optional(),
            "span_24h_hours": m.field("span_24h_hours").float().optional(),
            "span_7d_hours": m.field("span_7d_hours").float().optional(),
            "by_board": m.field("by_board").list(|f| {
                f.model(|b| json!({
                    "uuid": b.field("uuid").int().required(),
                    "name": b.field("name").str().required(),
                    "category": b.field("category").str().required(),
                    "active_players": b.field("active_players").int().required(),
                }))
                .required()
            })
            .required(),
            "boards_analyzed": m.field("boards_analyzed").int().required(),
            "methodology": m.field("methodology").str().required(),
            "computed_at": m.field("computed_at").int().required(),
        })
    })
}

/// `ActivityHistoryResponse`.
pub fn history(payload: &Value) -> Option<Value> {
    project(payload, |m| {
        json!({
            "days": m.field("days").int().required(),
            "window_start": m.field("window_start").int().required(),
            "window_end": m.field("window_end").int().required(),
            "points": m.field("points").list(|f| {
                f.model(|p| json!({
                    "window_end": p.field("window_end").int().required(),
                    "window_start": p.field("window_start").int().required(),
                    "duration_hours": p.field("duration_hours").float().required(),
                    "estimate": p.field("estimate").int().required(),
                    "estimate_24h": p.field("estimate_24h").int().optional(),
                    "estimate_per_hour": p.field("estimate_per_hour").float().required(),
                }))
                .required()
            })
            .required(),
            "methodology": m.field("methodology").str().required(),
        })
    })
}

fn peak(f: Field<'_>) -> Option<Value> {
    f.model(|p| json!({"t": p.field("t").int().required(), "active": p.field("active").float().required()})).optional()
}

/// `ActivitySeriesResponse` (its points drop `avg`).
pub fn series(payload: &Value) -> Option<Value> {
    project(payload, |m| {
        json!({
            "period": m.field("period").str().required(),
            "bucket_seconds": m.field("bucket_seconds").int().required(),
            "window_start": m.field("window_start").int().required(),
            "window_end": m.field("window_end").int().required(),
            "points": m.field("points").list(|f| {
                f.model(|p| json!({
                    "t": p.field("t").int().required(),
                    "active": p.field("active").float().required(),
                    "peak": p.field("peak").float().required(),
                    "samples": p.field("samples").int().required(),
                }))
                .required()
            })
            .required(),
            "peak": peak(m.field("peak")),
            "low": peak(m.field("low")),
            "average": m.field("average").float().optional(),
            "latest": m.field("latest").float().optional(),
            "methodology": m.field("methodology").str().required(),
        })
    })
}

/// `ClassActivityCurrentResponse`.
pub fn class_current(payload: &Value) -> Option<Value> {
    project(payload, |m| {
        json!({
            "window_start": m.field("window_start").int().nullable(),
            "window_end": m.field("window_end").int().nullable(),
            "duration_hours": m.field("duration_hours").float().nullable(),
            "total_active": m.field("total_active").int().nullable(),
            "total_active_clean": m.field("total_active_clean").int().optional(),
            "total_effort_added": m.field("total_effort_added").int().optional(),
            "total_effort_added_clean": m.field("total_effort_added_clean").int().optional(),
            "power_rank_threshold": m.field("power_rank_threshold").int().default(0),
            "effort_threshold": m.field("effort_threshold").int().default(0),
            "xp_threshold": m.field("xp_threshold").int().default(0),
            "classes": m.field("classes").list(|f| {
                f.model(|c| json!({
                    "class_index": c.field("class_index").int().required(),
                    "name": c.field("name").str().required(),
                    "icon": c.field("icon").str().nullable(),
                    "active_players": c.field("active_players").int().required(),
                    "share": c.field("share").float().required(),
                    "active_players_clean": c.field("active_players_clean").int().optional(),
                    "share_clean": c.field("share_clean").float().optional(),
                    "effort_added": c.field("effort_added").int().optional(),
                    "effort_added_clean": c.field("effort_added_clean").int().optional(),
                }))
                .required()
            })
            .required(),
            "methodology": m.field("methodology").str().required(),
            "computed_at": m.field("computed_at").int().required(),
        })
    })
}

fn float_list(f: Field<'_>) -> Value {
    Value::Array(f.list(|x| json!(x.float().nullable())).required())
}

/// `ClassActivitySeriesResponse`.
pub fn class_series(payload: &Value) -> Option<Value> {
    project(payload, |m| {
        json!({
            "period": m.field("period").str().required(),
            "bucket_seconds": m.field("bucket_seconds").int().required(),
            "window_start": m.field("window_start").int().required(),
            "window_end": m.field("window_end").int().required(),
            "power_rank_threshold": m.field("power_rank_threshold").int().default(0),
            "effort_threshold": m.field("effort_threshold").int().default(0),
            "xp_threshold": m.field("xp_threshold").int().default(0),
            "buckets": m.field("buckets").list(|f| f.int().required()).required(),
            "classes": m.field("classes").list(|f| {
                f.model(|c| {
                    let class_index = c.field("class_index").int().required();
                    let name = c.field("name").str().required();
                    let icon = c.field("icon").str().nullable();
                    let values = float_list(c.field("values"));
                    let values_clean = if c.get("values_clean").is_some() {
                        float_list(c.field("values_clean"))
                    } else {
                        json!([])
                    };
                    json!({
                        "class_index": class_index,
                        "name": name,
                        "icon": icon,
                        "values": values,
                        "values_clean": values_clean,
                    })
                })
                .required()
            })
            .required(),
            "methodology": m.field("methodology").str().required(),
        })
    })
}

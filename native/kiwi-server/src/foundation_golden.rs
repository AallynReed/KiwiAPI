//! Byte-for-byte checks of the foundation modules against fixtures generated
//! from the real Python code (`testdata/foundation/gen_*.py`).

use std::sync::OnceLock;

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use chrono::{DateTime, FixedOffset, Utc};
use serde_json::{json, Map, Number, Value};
use tower::ServiceExt;

use crate::{
    dt,
    error::{self, ApiError, ErrorCode},
    pyfloat,
    pyjson::{self, LoadsError, PyJson, PydanticJson},
    request_id,
    validation::{parse_qsl, py_repr_str, Checked, Fail, Field, FromBody, FromParams, Json, Model, Params, Path, Query},
};

fn load(cell: &'static OnceLock<Value>, text: &str) -> &'static Value {
    cell.get_or_init(|| serde_json::from_str(text).expect("fixture parses"))
}

fn stdlib() -> &'static Value {
    static S: OnceLock<Value> = OnceLock::new();
    load(&S, include_str!("../testdata/foundation/stdlib.json"))
}

fn app() -> &'static Value {
    static S: OnceLock<Value> = OnceLock::new();
    load(&S, include_str!("../testdata/foundation/app.json"))
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key].as_array().unwrap_or_else(|| panic!("fixture section {key}"))
}

fn bits(h: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(h, 16).expect("hex bits"))
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn hex(h: &str) -> Vec<u8> {
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).expect("hex")).collect()
}

/// A Python int as `Value` holds it: exact within i64/u64, else a float, and
/// `None` past the float range (the loader then reports `NonFinite`).
fn int_value(digits: &str) -> Option<Value> {
    if let Ok(i) = digits.parse::<i64>() {
        Some(Value::from(i))
    } else if let Ok(u) = digits.parse::<u64>() {
        Some(Value::from(u))
    } else {
        Number::from_f64(digits.parse().expect("digits")).map(Value::Number)
    }
}

/// The fixtures' tagged values (`{"i"}`, `{"f"}`, `{"d"}`) back to JSON.
fn try_untag(v: &Value) -> Option<Value> {
    Some(match v {
        Value::Array(a) => Value::Array(a.iter().map(try_untag).collect::<Option<_>>()?),
        Value::Object(o) => {
            if let Some(i) = o.get("i") {
                int_value(s(i))?
            } else if let Some(f) = o.get("f") {
                Value::Number(Number::from_f64(bits(s(f)))?)
            } else {
                let mut m = Map::new();
                for pair in o["d"].as_array().expect("pairs") {
                    m.insert(s(&pair[0]).to_owned(), try_untag(&pair[1])?);
                }
                Value::Object(m)
            }
        }
        other => other.clone(),
    })
}

fn untag(v: &Value) -> Value {
    try_untag(v).expect("representable value")
}

/// Asserts no mismatches, printing the first few.
fn report(what: &str, total: usize, bad: Vec<String>) {
    assert!(total > 0, "{what}: empty corpus");
    if !bad.is_empty() {
        let shown: Vec<_> = bad.iter().take(12).collect();
        panic!("{what}: {} of {total} differ, e.g.\n{}", bad.len(), shown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n"));
    }
}

#[test]
fn float_repr() {
    let c = arr(stdlib(), "float_repr");
    let bad = c
        .iter()
        .filter_map(|p| {
            let got = pyfloat::repr(bits(s(&p[0])));
            (got != s(&p[1])).then(|| format!("{} -> {got} want {}", s(&p[0]), s(&p[1])))
        })
        .collect();
    report("float repr", c.len(), bad);
}

#[test]
fn pydantic_float() {
    let c = arr(app(), "pydantic_float");
    let bad = c
        .iter()
        .filter_map(|p| {
            let got = pyfloat::pydantic(bits(s(&p[0])));
            (got != s(&p[1])).then(|| format!("{} -> {got} want {}", s(&p[0]), s(&p[1])))
        })
        .collect();
    report("pydantic float", c.len(), bad);
}

#[test]
fn json_dumps() {
    let c = arr(stdlib(), "dumps");
    let mut bad = Vec::new();
    for case in c {
        let v = untag(&case["v"]);
        if pyjson::dumps(&v) != s(&case["compact"]) {
            bad.push(format!("compact {} vs {}", pyjson::dumps(&v), s(&case["compact"])));
        }
        if pyjson::dumps_spaced(&v) != s(&case["spaced"]) {
            bad.push(format!("spaced {} vs {}", pyjson::dumps_spaced(&v), s(&case["spaced"])));
        }
    }
    report("json.dumps", c.len(), bad);
}

#[test]
fn pydantic_dumps() {
    let c = arr(app(), "pydantic_dumps");
    let bad = c
        .iter()
        .filter_map(|p| {
            let got = pyjson::pydantic(&untag(&p[0]));
            (got != s(&p[1])).then(|| format!("{got} vs {}", s(&p[1])))
        })
        .collect();
    report("pydantic dump_json", c.len(), bad);
}

#[test]
fn json_loads() {
    let c = arr(stdlib(), "loads");
    let mut bad = Vec::new();
    for case in c {
        let input = hex(s(&case["hex"]));
        let got = pyjson::loads(&input);
        let want = if let Some(ok) = case.get("ok") {
            try_untag(ok).ok_or(LoadsError::NonFinite)
        } else if let Some(msg) = case.get("err") {
            let pos = case["pos"].as_u64().expect("pos") as usize;
            match &got {
                Err(LoadsError::Syntax { msg: m, pos: p }) if *m == s(msg) && *p == pos => continue,
                _ => {
                    bad.push(format!("{:?}: got {got:?}, want {} at {pos}", String::from_utf8_lossy(&input), s(msg)));
                    continue;
                }
            }
        } else if case.get("decode").is_some() {
            Err(LoadsError::Decode)
        } else {
            Err(LoadsError::NonFinite)
        };
        if got != want {
            bad.push(format!("{:?}: got {got:?}, want {want:?}", String::from_utf8_lossy(&input)));
        }
    }
    report("json.loads", c.len(), bad);
}

#[test]
fn python_round() {
    let c = arr(stdlib(), "round");
    let bad = c
        .iter()
        .filter_map(|p| {
            let (x, n, want) = (bits(s(&p[0])), p[1].as_u64().expect("n") as u32, bits(s(&p[2])));
            let got = pyfloat::round(x, n);
            (got.to_bits() != want.to_bits()).then(|| format!("round({x:?}, {n}) = {got:?} want {want:?}"))
        })
        .collect();
    report("round(x, n)", c.len(), bad);
    let c = arr(stdlib(), "round_int");
    let bad = c
        .iter()
        .filter_map(|p| {
            let (x, want) = (bits(s(&p[0])), p[1].as_i64()?);
            let got = pyfloat::round_int(x);
            (got != want).then(|| format!("round({x:?}) = {got} want {want}"))
        })
        .collect();
    report("round(x)", c.len(), bad);
}

fn aware(us: i64, off: i32) -> DateTime<FixedOffset> {
    DateTime::from_timestamp_micros(us).expect("in range").with_timezone(&FixedOffset::east_opt(off).expect("offset"))
}

#[test]
fn datetime_formats() {
    let c = arr(stdlib(), "datetimes");
    let mut bad = Vec::new();
    for case in c {
        let d = aware(case["us"].as_i64().expect("us"), case["off"].as_i64().expect("off") as i32);
        let naive = d.naive_local();
        let checks = [
            ("iso", dt::isoformat(&d)),
            ("str", dt::str_dt(&d)),
            ("naive_iso", dt::isoformat_naive(&naive)),
            ("naive_str", dt::str_naive(&naive)),
            ("date", dt::date_iso(&naive.date())),
        ];
        for (key, got) in checks {
            if got != s(&case[key]) {
                bad.push(format!("{key}: {got} want {}", s(&case[key])));
            }
        }
        if dt::parse(s(&case["iso"])) != Some(d) || dt::parse(s(&case["str"])) != Some(d) {
            bad.push(format!("parse {}", s(&case["iso"])));
        }
    }
    report("datetime text", c.len(), bad);

    let c = arr(app(), "pydantic_datetimes");
    let mut bad = Vec::new();
    for case in c {
        let us = case["us"].as_i64().expect("us");
        let d = aware(us, case["off"].as_i64().expect("off") as i32);
        let utc = DateTime::from_timestamp_micros(us).expect("in range");
        let checks = [
            ("aware", dt::pydantic(&d)),
            ("utc_bson_tz", dt::pydantic(&utc)),
            ("naive", dt::isoformat_naive(&d.naive_local())),
            ("date", dt::date_iso(&d.naive_local().date())),
        ];
        for (key, got) in checks {
            if got != s(&case[key]) {
                bad.push(format!("{key}: {got} want {}", s(&case[key])));
            }
        }
        let back = dt::mongo_round_trip(utc).timestamp_micros();
        if back != case["mongo_us"].as_i64().expect("mongo_us") {
            bad.push(format!("mongo {us} -> {back}"));
        }
        if dt::parse_utc(&dt::pydantic(&utc)) != Some(utc) {
            bad.push(format!("parse {}", dt::pydantic(&utc)));
        }
    }
    report("pydantic datetimes", c.len(), bad);
}

#[test]
fn from_timestamp() {
    let c = arr(stdlib(), "fromtimestamp");
    let bad = c
        .iter()
        .filter_map(|p| {
            let got = dt::from_timestamp(bits(s(&p[0]))).map(|d| d.timestamp_micros());
            (got != p[1].as_i64()).then(|| format!("{} -> {got:?} want {}", bits(s(&p[0])), p[1]))
        })
        .collect();
    report("fromtimestamp", c.len(), bad);
}

#[test]
fn now_has_microsecond_precision() {
    assert_eq!(dt::now().timestamp_subsec_nanos() % 1000, 0);
    let t: DateTime<Utc> = "2024-01-02T03:04:05.123456789Z".parse().expect("rfc3339");
    assert_eq!(dt::pydantic(&dt::trunc_micros(t)), "2024-01-02T03:04:05.123456Z");
    assert_eq!(dt::pydantic(&dt::mongo_round_trip(t)), "2024-01-02T03:04:05.123000Z");
}

#[test]
fn query_strings() {
    let c = arr(stdlib(), "parse_qsl");
    let bad = c
        .iter()
        .filter_map(|p| {
            let got: Vec<Value> = parse_qsl(s(&p[0])).into_iter().map(|(k, v)| json!([k, v])).collect();
            (Value::Array(got.clone()) != p[1]).then(|| format!("{:?} -> {got:?} want {}", s(&p[0]), p[1]))
        })
        .collect();
    report("parse_qsl", c.len(), bad);
}

#[test]
fn str_repr_and_phrases() {
    for p in arr(stdlib(), "repr_str") {
        assert_eq!(py_repr_str(s(&p[0])), s(&p[1]));
    }
    for p in arr(stdlib(), "http_phrases") {
        assert_eq!(error::status_phrase(p[0].as_u64().expect("code") as u16), Some(s(&p[1])));
    }
}

#[test]
fn error_codes_match_python() {
    let want: Vec<&str> = arr(app(), "error_codes").iter().map(s).collect();
    let got: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(got, want);
    for (status, code) in app()["status_to_code"].as_object().expect("map") {
        let st = StatusCode::from_u16(status.parse().expect("status")).expect("status");
        assert_eq!(error::code_for_status(st).as_str(), s(code));
    }
    assert_eq!(error::code_for_status(StatusCode::IM_A_TEAPOT), ErrorCode::InternalError);
    assert_eq!(error::NOT_FOUND_HTML, s(&app()["not_found_html"]));
}

#[tokio::test]
async fn envelope_uses_the_scoped_request_id() {
    let e = ApiError::not_found("Token not found");
    assert_eq!(e.body(), r#"{"error":{"code":"not_found","message":"Token not found","details":null,"request_id":"-"}}"#);
    let body = request_id::scope("req_x".into(), async { e.body() }).await;
    assert!(body.ends_with(r#""request_id":"req_x"}}"#));
    let id = request_id::generate();
    assert!(id.len() == 20 && id.starts_with("req_") && id[4..].bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    assert_ne!(id, request_id::generate());
}

#[tokio::test]
async fn generated_request_id_is_forwarded_upstream() {
    let router = Router::new()
        .route(
            "/echo",
            get(|req: Request| async move {
                let seen = req.headers().get("x-request-id").map(|v| v.to_str().unwrap_or("").to_owned());
                PyJson(json!({"seen": seen, "current": request_id::current()}))
            }),
        )
        .layer(axum::middleware::from_fn(request_id::middleware));
    let resp = router.oneshot(Request::get("/echo").body(Body::empty()).expect("req")).await.expect("resp");
    let rid = resp.headers()["x-request-id"].to_str().expect("ascii").to_owned();
    let body: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body"))
        .expect("json");
    assert_eq!(body, json!({"seen": rid, "current": rid}));
}

#[test]
fn datetime_serde_helpers() {
    #[derive(serde::Serialize)]
    struct Row {
        #[serde(serialize_with = "dt::ser_pydantic")]
        a: DateTime<Utc>,
        #[serde(serialize_with = "dt::ser_isoformat_opt")]
        b: Option<DateTime<Utc>>,
        #[serde(serialize_with = "dt::ser_pydantic_opt")]
        c: Option<DateTime<Utc>>,
    }
    let t = DateTime::from_timestamp_micros(1_700_000_000_123_000).expect("ts");
    let v = serde_json::to_value(Row { a: t, b: Some(t), c: None }).expect("value");
    assert_eq!(
        pyjson::pydantic(&v),
        r#"{"a":"2023-11-14T22:13:20.123000Z","b":"2023-11-14T22:13:20.123000+00:00","c":null}"#
    );
}

// --- HTTP: the same routes as gen_app.py, replayed through axum ------------

#[derive(Default)]
struct Inner {
    n: i64,
    tags: Vec<String>,
}

#[derive(Default)]
struct M {
    name: String,
    age: i64,
    score: Option<f64>,
    flag: bool,
    kind: String,
    color: String,
    inner: Option<Inner>,
    items: Vec<i64>,
    mp: Vec<(String, i64)>,
    oid: Option<mongodb::bson::oid::ObjectId>,
    ratio: f64,
    n2: i64,
    note: Option<String>,
    nums: Vec<f64>,
    req_list: Option<Vec<String>>,
}

fn read_m(m: &mut Model<'_>) -> M {
    M {
        name: m
            .field("name")
            .str()
            .min_len(1)
            .max_len(5)
            .check(|s| if s == "x" { Err(Fail::value_error("no x allowed")) } else { Ok(()) })
            .required(),
        age: m.field("age").int().ge(0).le(150).required(),
        score: m.field("score").float().ge(0.05).le(1.0).optional(),
        flag: m.field("flag").bool().default(false),
        kind: m.field("kind").literal(&["a", "b"]).default("a".into()),
        color: m.field("color").enumeration(&["red", "green", "blue"]).default("red".into()),
        inner: m
            .field("inner")
            .model(|i| {
                Some(Inner {
                    n: i.field("n").int().required(),
                    tags: i.field("tags").list(|f| f.str().required()).max_len(2).default(vec![]),
                })
            })
            .optional()
            .flatten(),
        items: m.field("items").list(|f| f.int().required()).max_len(3).default(vec![]),
        mp: m.field("mp").dict(|f| f.int().required()).default(vec![]),
        oid: m.field("oid").object_id().optional(),
        ratio: m.field("ratio").float().gt(0).lt(1.5).default(0.5),
        n2: m.field("n2").int().multiple_of(5).default(0),
        note: m.field("note").str().max_len(10).optional(),
        nums: m.field("nums").list(|f| f.float().required()).default(vec![]),
        req_list: m.field("req_list").list(|f| f.str().required()).min_len(2).optional(),
    }
}

impl M {
    fn json(&self) -> Value {
        json!({
            "name": self.name, "age": self.age, "score": self.score, "flag": self.flag, "kind": self.kind,
            "color": self.color,
            "inner": self.inner.as_ref().map(|i| json!({"n": i.n, "tags": i.tags})),
            "items": self.items,
            "mp": Value::Object(self.mp.iter().map(|(k, v)| (k.clone(), Value::from(*v))).collect()),
            "oid": self.oid.map(|o| o.to_hex()), "ratio": self.ratio, "n2": self.n2, "note": self.note,
            "nums": self.nums, "req_list": self.req_list,
        })
    }
}

impl FromBody for M {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(read_m(m))).required()
    }
}

struct OptM(Option<M>);

impl FromBody for OptM {
    fn from_body(body: Field<'_>) -> Option<Self> {
        Some(OptM(body.model(|m| Some(read_m(m))).optional().flatten()))
    }
}

struct Ints(Vec<i64>);

impl FromBody for Ints {
    fn from_body(body: Field<'_>) -> Option<Self> {
        Some(Ints(body.list(|f| f.int().required()).min_len(1).max_len(3).required()))
    }
}

struct AnyDict(Vec<(String, Value)>);

impl FromBody for AnyDict {
    fn from_body(body: Field<'_>) -> Option<Self> {
        Some(AnyDict(body.dict(|f| f.any().required()).required()))
    }
}

struct Q(Value);

impl FromParams for Q {
    fn from_params(p: &mut Params<'_>) -> Self {
        Q(json!({
            "limit": p.query("limit").int().ge(1).le(200).default(50),
            "fmt": p.query("fmt").str().one_of_pattern(&["json", "bin"]).default("json".into()),
            "name": p.query("name").str().min_len(1).max_len(4).required(),
            "f": p.query("f").float().ge(0.05).le(1.0).optional(),
            "b": p.query("b").bool().default(false),
            "kind": p.query("kind").literal(&["x", "y", "z"]).default("x".into()),
            "color": p.query("color").enumeration(&["red", "green", "blue"]).default("red".into()),
            "ids": p.query_list("ids").list(|f| f.int().required()).default(vec![]),
            "u": p.query("u").int().ge(0).le(0xFFFF_FFFFi64).required(),
            "cursor": p.query("cursor").str().max_len(40).optional(),
            "oid": p.query("oid").object_id().optional().map(|o| o.to_hex()),
            "tags": p.query_list("tags").list(|f| f.str().required()).max_len(2).default(vec![]),
        }))
    }
}

struct P(Value);

impl FromParams for P {
    fn from_params(p: &mut Params<'_>) -> Self {
        let oid = p.path("oid").object_id().required();
        P(json!({"oid": oid.to_hex(), "n": p.path("n").int().ge(1).required()}))
    }
}

struct H(Value);

impl FromParams for H {
    fn from_params(p: &mut Params<'_>) -> Self {
        H(json!({
            "key": p.header("x-internal-key").str().default(String::new()),
            "num": p.header("x-num").int().default(0),
        }))
    }
}

struct B2(i64, i64);

impl FromParams for B2 {
    fn from_params(p: &mut Params<'_>) -> Self {
        let n = p.path("n").int().required();
        B2(n, p.query("x").int().required())
    }
}

struct Kind(String);

impl FromParams for Kind {
    fn from_params(p: &mut Params<'_>) -> Self {
        Kind(p.path("kind").str().required())
    }
}

async fn e_route(method: Method, uri: Uri, headers: HeaderMap, Path(Kind(kind)): Path<Kind>) -> Response {
    let http = |code: u16, detail: Option<&str>| ApiError::http(StatusCode::from_u16(code).expect("status"), detail);
    let err = match kind.as_str() {
        "http404" => return error::http_exception(StatusCode::NOT_FOUND, None, &method, uri.path(), &headers),
        "http404d" => {
            return error::http_exception(StatusCode::NOT_FOUND, Some("texture not found"), &method, uri.path(), &headers);
        }
        "api404" => ApiError::not_found("The Mods Hub is currently disabled."),
        "http400" => http(400, Some("Bad thing \u{2615} \u{e9} \u{2028} \"q\"")),
        "http422" => http(422, Some("Blueprint not renderable: x")),
        "http418" => http(418, None),
        "http503" => http(503, Some("down")),
        "http409" => http(409, Some("You already have a creator token.")),
        "http401" => http(401, Some("nope")),
        "api" => ApiError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::IpNotAllowed,
            "Requests from this IP are not allowed for this token",
        ),
        "details" => ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::RateLimited,
            "Rate limit exceeded. Slow down and try again later.",
        )
        .with_details(json!({
            "limit": 120, "window_seconds": 60, "f": 1e16, "g": 1e-05, "h": 0.1,
            "s": "\u{e9}\u{2028}\u{7f}\u{1f}", "n": null, "l": [1, 2.0, true],
        })),
        "adhoc" => ApiError::new(StatusCode::BAD_REQUEST, "weird_code", "Caf\u{e9}"),
        "exists" => ApiError::from_value_error("Name already exists"),
        "valueerr" => ApiError::from_value_error("bad value"),
        "boom" => panic!("boom"),
        other => return PyJson(json!({"kind": other})).into_response(),
    };
    err.into_response()
}

async fn fallback(req: Request) -> Response {
    error::not_found(req.method(), req.uri().path(), req.headers())
}

fn test_router() -> Router {
    Router::new()
        .route("/q", get(|Query(Q(v)): Query<Q>| async move { PyJson(v) }))
        .route("/p/{oid}/{n}", get(|Path(P(v)): Path<P>| async move { PyJson(v) }))
        .route("/h", get(|Query(H(v)): Query<H>| async move { PyJson(v) }))
        .route("/b", post(|Json(m): Json<M>| async move { PyJson(json!({"m": m.json()})) }))
        .route(
            "/b2/{n}",
            post(|Checked(B2(n, x), m): Checked<B2, M>| async move { PyJson(json!({"n": n, "x": x, "m": m.json()})) }),
        )
        .route("/opt", post(|Json(OptM(m)): Json<OptM>| async move { PyJson(json!({"m": m.map(|m| m.json())})) }))
        .route("/lst", post(|Json(Ints(v)): Json<Ints>| async move { PyJson(json!({"items": v})) }))
        .route(
            "/dict",
            post(|Json(AnyDict(d)): Json<AnyDict>| async move {
                PyJson(json!({"d": Value::Object(d.into_iter().collect())}))
            }),
        )
        .route(
            "/only-get",
            get(|| async { PyJson(json!({"ok": true})) }).fallback(|| async { error::method_not_allowed("GET") }),
        )
        .route("/e/{kind}", get(e_route))
        .fallback(fallback)
        .layer(axum::middleware::from_fn(request_id::middleware))
}

struct Login;

impl FromBody for Login {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            m.field("email").str().required();
            m.field("password").str().required();
            m.field("captcha_token").str().optional();
            Some(Login)
        })
        .required()
    }
}

struct Signup;

impl FromBody for Signup {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            m.field("email").str().required();
            m.field("password").str().min_len(8).max_len(256).required();
            m.field("display_name").str().max_len(80).optional();
            m.field("captcha_token").str().optional();
            Some(Signup)
        })
        .required()
    }
}

struct StatRange;

impl FromParams for StatRange {
    fn from_params(p: &mut Params<'_>) -> Self {
        p.query("tier").int().required();
        p.query("type").int().required();
        p.query("stat_type").int().required();
        p.query("level").int().ge(1).default(1);
        p.query("extra_containers").int().ge(0).default(0);
        p.query("element").int().optional();
        StatRange
    }
}

struct News;

impl FromParams for News {
    fn from_params(p: &mut Params<'_>) -> Self {
        p.query("limit").int().ge(1).le(50).default(16);
        News
    }
}

struct StarChart;

impl FromParams for StarChart {
    fn from_params(p: &mut Params<'_>) -> Self {
        p.query("code").str().max_len(8192).default(String::new());
        StarChart
    }
}

struct EvalSimple;

impl FromBody for EvalSimple {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            for (name, default) in [("tier", 4), ("type", 1), ("power_rank", 0), ("level", 1)] {
                m.field(name).int().default(default);
            }
            Some(EvalSimple)
        })
        .required()
    }
}

struct Evaluate;

impl FromBody for Evaluate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            for (name, default) in [("tier", 4), ("type", 1), ("level", 1)] {
                m.field(name).int().default(default);
            }
            m.field("stats")
                .list(|f| {
                    f.model(|s| {
                        s.field("type").int().required();
                        s.field("value").float().required();
                        s.field("extra_containers").int().default(0);
                        Some(())
                    })
                    .required()
                })
                .required();
            m.field("auto_guess_procs").bool().default(false);
            Some(Evaluate)
        })
        .required()
    }
}

fn unreachable_handler() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

fn real_router() -> Router {
    Router::new()
        .route(
            "/health",
            get(|| async { PydanticJson(json!({"status": "ok"})) }).fallback(|| async { error::method_not_allowed("GET") }),
        )
        .route("/auth/login", post(|_: Json<Login>| async { unreachable_handler() }))
        .route("/auth/signup", post(|_: Json<Signup>| async { unreachable_handler() }))
        .route("/site/gems/stat-range", get(|_: Query<StatRange>| async { unreachable_handler() }))
        .route("/site/feeds/news", get(|_: Query<News>| async { unreachable_handler() }))
        .route("/site/gems/parse-star-chart", get(|_: Query<StarChart>| async { unreachable_handler() }))
        .route("/site/gems/evaluate-simple", post(|_: Json<EvalSimple>| async { unreachable_handler() }))
        .route("/site/gems/evaluate", post(|_: Json<Evaluate>| async { unreachable_handler() }))
        .fallback(fallback)
        .layer(axum::middleware::from_fn(request_id::middleware))
}

async fn replay(router: Router, section: &str) {
    let c = arr(app(), section);
    let mut bad = Vec::new();
    for case in c {
        let name = s(&case["name"]);
        let mut req = Request::builder()
            .method(Method::from_bytes(s(&case["method"]).as_bytes()).expect("method"))
            .uri(s(&case["url"]));
        for h in case["headers"].as_array().expect("headers") {
            req = req.header(
                HeaderName::from_bytes(s(&h[0]).as_bytes()).expect("name"),
                HeaderValue::from_bytes(&hex(s(&h[1]))).expect("value"),
            );
        }
        let req = req.body(Body::from(hex(s(&case["body_hex"])))).expect("request");
        let resp = router.clone().oneshot(req).await.expect("infallible");
        let status = resp.status().as_u16();
        let header = |n: &str| resp.headers().get(n).map(|v| v.as_bytes().to_vec()).unwrap_or_default();
        let (ct, allow, rid) = (header("content-type"), header("allow"), header("x-request-id"));
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf-8 body");

        let mut want_body = s(&case["resp"]).to_owned();
        let want_rid = hex(s(&case["x_request_id_hex"]));
        if name == "rid_empty" || name.starts_with("noid") {
            let ours = String::from_utf8(rid.clone()).expect("ascii id");
            let ok = ours.len() == 20 && ours.starts_with("req_") && ours[4..].bytes().all(|b| b.is_ascii_hexdigit());
            if !ok {
                bad.push(format!("{name}: generated id {ours:?}"));
            }
            want_body = want_body.replace(&String::from_utf8(want_rid.clone()).expect("ascii"), &ours);
        } else if rid != want_rid {
            bad.push(format!("{name}: x-request-id {:?} want {:?}", String::from_utf8_lossy(&rid), String::from_utf8_lossy(&want_rid)));
        }
        if status as u64 != case["status"].as_u64().expect("status") {
            bad.push(format!("{name}: status {status} want {}", case["status"]));
        }
        if ct != s(&case["content_type"]).as_bytes() {
            bad.push(format!("{name}: content-type {:?} want {}", String::from_utf8_lossy(&ct), case["content_type"]));
        }
        if allow != s(&case["allow"]).as_bytes() {
            bad.push(format!("{name}: allow {:?} want {}", String::from_utf8_lossy(&allow), case["allow"]));
        }
        if body != want_body {
            bad.push(format!("{name}:\n  got  {body}\n  want {want_body}"));
        }
    }
    report(section, c.len(), bad);
}

#[tokio::test]
async fn http_test_app() {
    replay(test_router(), "http_test_app").await;
}

#[tokio::test]
async fn http_real_app() {
    replay(real_router(), "http_real_app").await;
}

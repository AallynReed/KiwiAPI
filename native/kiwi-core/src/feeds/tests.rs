//! Spot checks; the golden fixtures from Python run in kiwi-server
//! (`src/feeds/tests.rs`).

use super::soup::{AttrValue, Node, Soup, StrKind};
use super::{py, unescape};

#[test]
fn python_int() {
    assert_eq!(py::int(" 12 ", 10), Some(12));
    assert_eq!(py::int("1_000", 10), Some(1000));
    assert_eq!(py::int("+5", 10), Some(5));
    assert_eq!(py::int("-5", 10), Some(-5));
    assert_eq!(py::int("\u{661}\u{662}", 10), Some(12));
    assert_eq!(py::int("0x_1f", 16), Some(31));
    for bad in ["", " ", "1__0", "_1", "1_", "1.0", "abc", "-", "0x1f"] {
        assert_eq!(py::int(bad, 10), None, "{bad:?}");
    }
    assert_eq!(py::int("0x", 16), None);
}

#[test]
fn unescape_like_python() {
    assert_eq!(unescape::unescape("&amp;&lt;x&gt;"), "&<x>");
    assert_eq!(unescape::unescape("&notit;"), "\u{ac}it;");
    assert_eq!(unescape::unescape("&#128;&#0;&#xD800;"), "\u{20ac}\u{fffd}\u{fffd}");
    assert_eq!(unescape::unescape("a & b &#; &#x;"), "a & b &#; &#x;");
    assert_eq!(unescape::unescape_attr("&amp=x&copyx &#65"), "&amp=x&copyx A");
    assert_eq!(unescape::bs4_entity("nbsp"), Some("\u{a0}"));
    assert_eq!(unescape::bs4_entity("acE"), Some("\u{223e}\u{333}"));
}

#[test]
fn void_and_unclosed_tags() {
    let soup = Soup::parse("<div><img src=a.png><span>x<p>y</div>z").unwrap();
    let div = soup.select_one(0, |t| t.name == "div").unwrap();
    let names: Vec<&str> = soup.descendants(div).iter().map(|&t| soup.tag(t).name.as_str()).collect();
    assert_eq!(names, ["img", "span", "p"]);
    assert_eq!(soup.text(0), "xyz");
    let img = soup.select_one(0, |t| t.name == "img").unwrap();
    assert_eq!(soup.tag(img).get("src"), Some(&AttrValue::Str("a.png".into())));
}

#[test]
fn script_text_is_not_tag_text() {
    let soup = Soup::parse("<p>a<script>var x = '<b>';</script><!-- c -->b</p>").unwrap();
    let p = soup.select_one(0, |t| t.name == "p").unwrap();
    assert_eq!(soup.text(p), "ab");
    let script = soup.select_one(0, |t| t.name == "script").unwrap();
    assert!(matches!(&soup.tag(script).children[..], [Node::Text(StrKind::Script, s)] if s == "var x = '<b>';"));
}

#[test]
fn bad_charref_is_an_error() {
    assert!(Soup::parse("<p>&#12a; more</p>").is_err());
}

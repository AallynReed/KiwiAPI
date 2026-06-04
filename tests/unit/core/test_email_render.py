from app.core.email_render import render_email


def test_render_email_includes_content():
    html = render_email(
        "Welcome", ["First line.", "Second line."],
        button={"label": "Verify", "url": "https://api.aallyn.net/verify?t=abc"},
    )
    assert "Welcome" in html
    assert "First line." in html and "Second line." in html
    assert "https://api.aallyn.net/verify?t=abc" in html
    assert "Verify" in html


def test_render_email_autoescapes_content():
    # Autoescaping must neutralize HTML injected via content (e.g. a token name).
    html = render_email("Hi", ["<script>alert(1)</script>"])
    assert "<script>alert(1)</script>" not in html
    assert "&lt;script&gt;" in html


def test_render_email_optional_button_and_note():
    html = render_email("Notice", ["Body only."])
    assert "Body only." in html  # renders fine with no button / no note

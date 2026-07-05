"""Jinja2-rendered HTML emails: one branded base template, autoescaped content.

Templates are in-module strings loaded via ``DictLoader`` so there are no
filesystem-path concerns inside the container.
"""

from jinja2 import DictLoader, Environment, select_autoescape

from app.core.config import settings

_BASE = """\
<!doctype html>
<html lang="en">
<body style="margin:0;padding:0;background:#0d1117;
  font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;">
  <table role="presentation" width="100%" cellpadding="0" cellspacing="0"
    style="background:#0d1117;padding:28px 0;">
    <tr><td align="center">
      <table role="presentation" width="100%" cellpadding="0" cellspacing="0"
        style="max-width:520px;background:#161b22;border:1px solid #232a33;border-radius:14px;">
        <tr><td style="padding:26px 30px 8px;">
          <div style="font-size:1.25rem;font-weight:700;color:#e6edf3;">
            <span style="color:#58a6ff;">&#9670;</span> {{ app_name }}
          </div>
        </td></tr>
        <tr><td style="padding:8px 30px 4px;">
          <h1 style="margin:0 0 6px;font-size:1.3rem;color:#e6edf3;">{{ heading }}</h1>
        </td></tr>
        <tr><td style="padding:4px 30px 8px;color:#c2ccd6;line-height:1.6;font-size:.97rem;">
          {% for p in paragraphs %}<p style="margin:0 0 14px;">{{ p }}</p>{% endfor %}
          {% if button %}
          <p style="margin:22px 0;">
            <a href="{{ button.url }}" style="display:inline-block;background:#238636;color:#fff;
              text-decoration:none;padding:11px 20px;border-radius:8px;font-weight:600;">{{ button.label }}</a>
          </p>
          <p style="margin:0 0 14px;color:#8b949e;font-size:.85rem;word-break:break-all;">
            Or paste this link into your browser:<br>{{ button.url }}</p>
          {% endif %}
          {% if note %}<p style="margin:14px 0 0;color:#8b949e;font-size:.85rem;">{{ note }}</p>{% endif %}
        </td></tr>
        <tr><td style="padding:18px 30px 26px;border-top:1px solid #232a33;color:#6e7681;font-size:.8rem;">
          {{ app_name }} · <a href="{{ dev_url }}" style="color:#58a6ff;">Developer portal</a>
          · <a href="{{ docs_url }}" style="color:#58a6ff;">Docs</a>
        </td></tr>
      </table>
    </td></tr>
  </table>
</body>
</html>
"""

_env = Environment(
    loader=DictLoader({"base.html": _BASE}),
    autoescape=select_autoescape(["html"]),
)


def render_email(
    heading: str,
    paragraphs: list[str],
    button: dict | None = None,
    note: str | None = None,
) -> str:
    """Render the branded HTML body (all content autoescaped)."""
    return _env.get_template("base.html").render(
        app_name=settings.app_name,
        dev_url=settings.dev_url,
        docs_url=settings.docs_url,
        heading=heading,
        paragraphs=paragraphs,
        button=button,
        note=note,
    )

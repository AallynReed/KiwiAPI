from app.core import email_outbox
from app.core.config import settings
from app.core.mailer import EmailDeliveryError


def test_backoff_is_exponential():
    b1 = email_outbox._backoff(1)
    b2 = email_outbox._backoff(2)
    b3 = email_outbox._backoff(3)
    assert b1 < b2 < b3
    assert b1.total_seconds() == settings.email_retry_base_seconds
    assert b2.total_seconds() == settings.email_retry_base_seconds * 2


async def test_queue_email_noop_when_smtp_disabled(monkeypatch):
    # With SMTP unconfigured, queue_email must short-circuit (log only) and never
    # touch the database — so this runs without a Mongo connection.
    monkeypatch.setattr(settings, "smtp_host", None)
    assert await email_outbox.queue_email("a@b.com", "subj", "text") is None


def test_email_delivery_error_classification():
    assert EmailDeliveryError("x", permanent=True).permanent is True
    assert EmailDeliveryError("x", permanent=False).permanent is False

//! `app/core/mailer.py`: one message over SMTP (the host Postfix in
//! production), with optional implicit TLS / STARTTLS and login, classified
//! into a hard bounce (5xx) or a failure worth retrying.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, MultiPart, SinglePart},
    transport::smtp::{
        authentication::Credentials,
        client::{Tls, TlsParameters},
        extension::ClientId,
    },
};

use crate::config::Settings;

/// `EmailDeliveryError`: `permanent` is a hard bounce (don't retry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryError {
    pub message: String,
    pub permanent: bool,
    pub code: Option<u16>,
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl DeliveryError {
    fn permanent(message: impl Into<String>) -> Self {
        DeliveryError { message: message.into(), permanent: true, code: None }
    }
}

/// `make_msgid(domain=...)`: `<centiseconds.pid.random64@domain>`.
pub fn make_msgid(domain: &str) -> String {
    let centis = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() / 10).unwrap_or(0);
    let mut buf = [0u8; 8];
    let _ = getrandom::fill(&mut buf);
    format!("<{centis}.{}.{}@{domain}>", std::process::id(), u64::from_le_bytes(buf))
}

/// The message `_build_message` makes: From, To, Subject, Date, Message-ID,
/// a text/plain body and, when there is HTML, a multipart/alternative.
pub fn build_message(
    settings: &Settings,
    to: &str,
    subject: &str,
    text: &str,
    html: Option<&str>,
) -> Result<Message, DeliveryError> {
    let from_addr = settings.mail_from.parse().map_err(|e| DeliveryError::permanent(format!("MAIL FROM: {e}")))?;
    let to: Mailbox = to.parse().map_err(|e| DeliveryError::permanent(format!("Recipients refused: {to}: {e}")))?;
    let domain = settings.mail_from.rsplit('@').next().unwrap_or_default();
    let builder = Message::builder()
        .from(Mailbox::new(Some(settings.mail_from_name.clone()), from_addr))
        .to(to)
        .subject(subject)
        .date_now()
        .message_id(Some(make_msgid(domain)));
    let built = match html.filter(|h| !h.is_empty()) {
        Some(h) => builder.multipart(MultiPart::alternative_plain_html(text.to_owned(), h.to_owned())),
        None => builder.singlepart(SinglePart::plain(text.to_owned())),
    };
    built.map_err(|e| DeliveryError::permanent(format!("Invalid message: {e}")))
}

fn hello_name() -> ClientId {
    let host = std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| "localhost".into());
    ClientId::Domain(host)
}

/// A failed send as Python classifies it: a 5xx reply is permanent, any
/// other reply or connection trouble is transient.
fn classify(err: &lettre::transport::smtp::Error) -> DeliveryError {
    if let Some(code) = err.status() {
        let n: u16 = code.to_string().parse().unwrap_or(0);
        let text = std::error::Error::source(err).map(|s| s.to_string()).unwrap_or_default();
        return DeliveryError {
            message: format!("SMTP {n}: {text}"),
            permanent: (500..600).contains(&n),
            code: Some(n),
        };
    }
    DeliveryError { message: format!("Transient SMTP failure: {err}"), permanent: false, code: None }
}

/// `deliver_email`: send one message now. `Ok(Err(..))` is a classified
/// delivery failure; `Err` is a configuration error Python lets escape.
pub async fn deliver_email(
    settings: &Settings,
    to: &str,
    subject: &str,
    text: &str,
    html: Option<&str>,
) -> anyhow::Result<Result<(), DeliveryError>> {
    let Some(host) = settings.smtp_host.as_deref().filter(|h| !h.is_empty()) else {
        return Ok(Err(DeliveryError::permanent("SMTP is not configured")));
    };
    if settings.smtp_starttls && settings.smtp_ssl {
        anyhow::bail!("The start_tls and use_tls options are not compatible.");
    }
    let message = match build_message(settings, to, subject, text, html) {
        Ok(m) => m,
        Err(e) => return Ok(Err(e)),
    };
    let port = u16::try_from(settings.smtp_port).map_err(|_| anyhow::anyhow!("SMTP_PORT out of range"))?;
    let tls = if settings.smtp_ssl || settings.smtp_starttls {
        let params = match TlsParameters::new(host.to_owned()) {
            Ok(p) => p,
            Err(e) => {
                return Ok(Err(DeliveryError {
                    message: format!("Transient SMTP failure: {e}"),
                    permanent: false,
                    code: None,
                }));
            }
        };
        if settings.smtp_ssl { Tls::Wrapper(params) } else { Tls::Required(params) }
    } else {
        Tls::None
    };
    let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
        .port(port)
        .tls(tls)
        .hello_name(hello_name())
        .timeout(Some(Duration::from_secs(15)));
    let user = settings.smtp_username.as_deref().filter(|u| !u.is_empty());
    let pass = settings.smtp_password.as_deref().filter(|p| !p.is_empty());
    if let (Some(u), Some(p)) = (user, pass) {
        builder = builder.credentials(Credentials::new(u.to_owned(), p.to_owned()));
    }
    Ok(builder.build().send(message).await.map(|_| ()).map_err(|e| classify(&e)))
}

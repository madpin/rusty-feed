//! Outbound transactional mail (email verification), behind a small port so a
//! missing/broken SMTP config degrades gracefully instead of breaking signup
//! (`docs/PUBLIC_LAUNCH_PLAN.md` §4.5).

use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

/// A cloneable mailer. `Noop` logs and no-ops (SMTP not configured); `Smtp`
/// sends via a pooled, TLS SMTP transport.
#[derive(Clone)]
pub enum Mailer {
    Noop,
    Smtp {
        transport: AsyncSmtpTransport<Tokio1Executor>,
        from: Mailbox,
    },
}

impl std::fmt::Debug for Mailer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mailer::Noop => write!(f, "Mailer::Noop"),
            Mailer::Smtp { .. } => write!(f, "Mailer::Smtp"),
        }
    }
}

impl Mailer {
    /// Build an SMTP mailer from a transport URL (e.g.
    /// `smtps://user:pass@smtp.example.com:465` or `smtp://host:587` for
    /// STARTTLS) and a `From` mailbox (e.g. `Rusty Feed <no-reply@example.com>`).
    pub fn from_smtp_url(url: &str, from: &str) -> anyhow::Result<Self> {
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)?.build();
        let from: Mailbox = from
            .parse()
            .map_err(|error| anyhow::anyhow!("invalid SMTP_FROM address: {error}"))?;
        Ok(Mailer::Smtp { transport, from })
    }

    /// Whether a real transport is configured.
    pub fn is_enabled(&self) -> bool {
        matches!(self, Mailer::Smtp { .. })
    }

    /// Send a plain-text email. Errors are surfaced so callers can decide how to
    /// degrade (signup, for instance, treats a send failure as non-fatal).
    pub async fn send(&self, to: &str, subject: &str, body_text: &str) -> anyhow::Result<()> {
        match self {
            Mailer::Noop => {
                tracing::warn!(to, subject, "email not sent: SMTP is not configured");
                Ok(())
            }
            Mailer::Smtp { transport, from } => {
                let message = Message::builder()
                    .from(from.clone())
                    .to(to
                        .parse()
                        .map_err(|error| anyhow::anyhow!("invalid recipient address: {error}"))?)
                    .subject(subject)
                    .header(ContentType::TEXT_PLAIN)
                    .body(body_text.to_string())?;
                transport.send(message).await?;
                Ok(())
            }
        }
    }
}

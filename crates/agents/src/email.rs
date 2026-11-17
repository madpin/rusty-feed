//! Small plain-text SMTP helper shared by opted-in Briefing and Roundup mail.

use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

pub(crate) async fn send_plain_text(
    smtp_url: Option<&str>,
    smtp_from: Option<&str>,
    to: &str,
    subject: &str,
    body: &str,
) -> anyhow::Result<()> {
    let (Some(url), Some(from)) = (smtp_url, smtp_from) else {
        return Ok(());
    };
    let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)?.build();
    let from: Mailbox = from
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid SMTP from address: {error}"))?;
    let message = Message::builder()
        .from(from)
        .to(to
            .parse()
            .map_err(|error| anyhow::anyhow!("invalid email recipient: {error}"))?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())?;
    transport.send(message).await?;
    Ok(())
}

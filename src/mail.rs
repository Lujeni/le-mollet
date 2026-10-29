//! One e-mail at a time, over SMTP (lettre): the sign-in links of src/account.rs. `LE_MOLLET_SMTP_URL` names the
//! server (`smtps://user:pass@host:465`, or `smtp://user:pass@host:587?tls=required` for STARTTLS) and `LE_MOLLET_MAIL_FROM`
//! the sender; without the URL the mail is printed on stderr, which is how a test or a LAN server reads its links.

use lettre::{Message, SmtpTransport, Transport, message::header::ContentType};

/// Slow (a TLS handshake and a round trip): call it off the async threads, and do not wait for it.
pub fn send(to: &str, subject: &str, body: &str) -> anyhow::Result<()> {
    let Ok(url) = std::env::var("LE_MOLLET_SMTP_URL") else {
        eprintln!("mail (no LE_MOLLET_SMTP_URL) to {to}: {subject}\n{body}");
        return Ok(());
    };
    let from = std::env::var("LE_MOLLET_MAIL_FROM").unwrap_or_else(|_| "Le Mollet <no-reply@le-mollet.fr>".into());
    let m = Message::builder().from(from.parse()?).to(to.parse()?).subject(subject).header(ContentType::TEXT_PLAIN).body(body.to_owned())?;
    SmtpTransport::from_url(&url)?.build().send(&m)?;
    Ok(())
}

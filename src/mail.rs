//! One e-mail at a time, over SMTP (lettre): the sign-in links of src/account.rs. `LE_MOLLET_SMTP_URL` names the
//! server (`smtps://user:pass@host:465`, or `smtp://user:pass@host:587?tls=required` for STARTTLS) and `LE_MOLLET_MAIL_FROM`
//! the sender (`LE_MOLLET_MAIL_REPLY_TO`, when set, where answers go: a sending-only address like auth@ points them to
//! the contact box); without the URL the mail is printed on stderr, which is how a test or a LAN server reads its links.

use lettre::message::{MultiPart, SinglePart, header::ContentType};
use lettre::{Message, SmtpTransport, Transport};

/// Text and, when given, HTML of the same mail (what mail readers and spam filters expect of a real sender).
/// Slow (a TLS handshake and a round trip): call it off the async threads, and do not wait for it.
pub fn send(to: &str, subject: &str, text: &str, html: Option<&str>) -> anyhow::Result<()> {
    let Ok(url) = std::env::var("LE_MOLLET_SMTP_URL") else {
        eprintln!("mail (no LE_MOLLET_SMTP_URL) to {to}: {subject}\n{text}");
        return Ok(());
    };
    let from = std::env::var("LE_MOLLET_MAIL_FROM").unwrap_or_else(|_| "Le Mollet <no-reply@le-mollet.fr>".into());
    let mut m = Message::builder().from(from.parse()?).to(to.parse()?).subject(subject);
    if let Ok(reply) = std::env::var("LE_MOLLET_MAIL_REPLY_TO") {
        m = m.reply_to(reply.parse()?);
    }
    let m = match html {
        Some(html) => m.multipart(
            MultiPart::alternative()
                .singlepart(SinglePart::builder().header(ContentType::TEXT_PLAIN).body(text.to_owned()))
                .singlepart(SinglePart::builder().header(ContentType::TEXT_HTML).body(html.to_owned())),
        )?,
        None => m.header(ContentType::TEXT_PLAIN).body(text.to_owned())?,
    };
    SmtpTransport::from_url(&url)?.build().send(&m)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn without_a_server_the_mail_is_printed() {
        if std::env::var("LE_MOLLET_SMTP_URL").is_ok() {
            return;
        } // a developer's real relay: nothing to send
        assert!(super::send("someone@example.org", "Test", "Hello", Some("<p>Hello</p>")).is_ok());
        assert!(super::send("not an address", "Test", "Hello", None).is_ok()); // printed, never parsed
    }
}

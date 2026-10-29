//! Le Mollet let hash = PasswordHash::new(a.as_ref().map(|a| a.hash.as_str()).filter(|h| !h.is_empty()).unwrap_or(DUMMY.as_str()))ccounts: a name, a password or an e-mail (or both), kept in the database (src/db.rs). Passwords are
//! stored as Argon2id hashes (PHC strings with their own salt), never as such. In cloud mode an account is
//! required to ride and carries an e-mail; at home it is optional (roles for LAN guests). See docs/rbac.md.
//! An account without a password signs in by a link sent to its e-mail (`link`, src/mail.rs).

use crate::db::Db;
use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// What roles and profiles refer to; the name may be looked up case-insensitively.
    pub id: u64,
    pub name: String,
    /// Empty: no password, the account signs in by e-mail only.
    hash: String,
    pub created: u64,
    /// Lower case, one account each; where the sign-in links go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// 3 to 24 letters, digits, `-` or `_`: a name to type and to show, nothing to escape.
pub fn valid_name(name: &str) -> bool {
    (3..=24).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Accounts are filed by their name in lower case: "Julien" and "julien" are one account.
fn key(name: &str) -> String {
    name.to_ascii_lowercase()
}

pub fn find(db: &Db, name: &str) -> Option<Account> {
    db.get("accounts", &key(name)).ok().flatten()
}

pub fn by_id(db: &Db, id: u64) -> Option<Account> {
    db.all::<Account>("accounts").ok()?.into_iter().map(|(_, a)| a).find(|a| a.id == id)
}

/// `a@b.c`, 254 characters at most, no spaces: enough to send to, the mail server says the rest.
pub fn valid_email(email: &str) -> bool {
    let Some((user, host)) = email.split_once('@') else { return false };
    email.len() <= 254 && !user.is_empty() && host.contains('.') && !host.starts_with('.') && !host.ends_with('.') && !email.chars().any(|c| c.is_whitespace() || c.is_control()) && email.matches('@').count() == 1
}

// ponytail: a scan, like by_id; an index when accounts count in thousands
pub fn by_email(db: &Db, email: &str) -> Option<Account> {
    let email = email.to_ascii_lowercase();
    db.all::<Account>("accounts").ok()?.into_iter().map(|(_, a)| a).find(|a| a.email.as_deref() == Some(&email))
}

/// Create an account: a password, an e-mail, or both (`None` and `""` alike mean none). Slow on purpose (the
/// hash): call it off the async threads.
pub fn signup(db: &Db, name: &str, email: Option<&str>, password: Option<&str>, now: u64) -> Result<Account, &'static str> {
    if !valid_name(name) { return Err("name: 3 to 24 letters, digits, - or _") }
    if find(db, name).is_some() { return Err("name taken") }
    let email = email.map(str::trim).filter(|e| !e.is_empty()).map(str::to_ascii_lowercase);
    if let Some(e) = &email {
        if !valid_email(e) { return Err("email: not an address") }
        if by_email(db, e).is_some() { return Err("email taken") }
    }
    let password = password.filter(|p| !p.is_empty());
    if password.is_none() && email.is_none() { return Err("password: 8 characters at least") }
    let hash = password.map(hash).transpose()?.unwrap_or_default();
    // A random id below 2^53, so the page reads it exactly.
    let id = ((uuid::Uuid::new_v4().as_u128() >> 75) as u64).max(1);
    let a = Account { id, name: name.into(), hash, created: now, email };
    // Atomic: two sign-ups with one name at once, one wins (the check above only gives the quick answer).
    match db.insert("accounts", &key(name), &a) {
        Ok(true) => Ok(a),
        Ok(false) => Err("name taken"),
        Err(_) => Err("could not save the account"),
    }
}

/// 8 to 128 characters, hashed. Slow on purpose: call it off the async threads.
fn hash(password: &str) -> Result<String, &'static str> {
    if password.chars().count() < 8 { return Err("password: 8 characters at least") }
    if password.len() > 128 { return Err("password: 128 characters at most") }
    Ok(Argon2::default().hash_password(password.as_bytes()).map_err(|_| "could not hash the password")?.to_string())
}

/// A new password, the current one checked first.
pub fn set_password(db: &Db, name: &str, password: &str, new: &str) -> Result<Account, &'static str> {
    let mut a = login(db, name, password)?;
    a.hash = hash(new)?;
    db.put("accounts", &key(name), &a).map_err(|_| "could not save the account")?;
    Ok(a)
}

/// A new e-mail, the password checked first (an account without one got here by its mail: it has no other key).
pub fn set_email(db: &Db, name: &str, password: &str, email: &str) -> Result<Account, &'static str> {
    let mut a = login(db, name, password)?;
    let email = email.trim().to_ascii_lowercase();
    if !valid_email(&email) { return Err("email: not an address") }
    if by_email(db, &email).is_some_and(|o| o.id != a.id) { return Err("email taken") }
    a.email = Some(email);
    db.put("accounts", &key(name), &a).map_err(|_| "could not save the account")?;
    Ok(a)
}

/// The account gone, its password checked first. Its sessions and profiles are the server's to end (src/web.rs).
pub fn delete(db: &Db, name: &str, password: &str) -> Result<Account, &'static str> {
    let a = login(db, name, password)?;
    db.delete("accounts", &key(name)).map_err(|_| "could not delete the account")?;
    Ok(a)
}

/// A sign-in link's body, for src/mail.rs; the token is the server's (one use, a quarter of an hour).
pub fn link_mail(name: &str, url: &str) -> (String, String) {
    ("Ta connexion à Le Mollet".into(), format!("Salut {name},\n\nVoici ton lien pour te connecter, valable 15 minutes et une seule fois :\n\n{url}\n\nSi tu n'as rien demandé, ignore ce message.\n\nLe Mollet"))
}

/// Sign-up proof of work, a captcha nobody sees: the page asks for a challenge, finds a counter whose SHA-256
/// with it starts with `POW_BITS` zero bits (a second or so of a browser), and sends both with the form. A bot
/// pays the same second per account, and thousands of accounts become hours of CPU. No third party, no tracking.
/// Challenges carry a signature (a key drawn at start) and a time: nothing to store until one is used (src/web.rs).
pub const POW_BITS: u32 = 16;
const POW_SECS: u64 = 600;
static POW_KEY: std::sync::LazyLock<[u8; 32]> = std::sync::LazyLock::new(|| {
    let (a, b) = (uuid::Uuid::new_v4().into_bytes(), uuid::Uuid::new_v4().into_bytes());
    let mut k = [0u8; 32];
    k[..16].copy_from_slice(&a);
    k[16..].copy_from_slice(&b);
    k
});

fn sha(parts: &[&[u8]]) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for p in parts { h.update(p) }
    h.finalize().into()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn pow_sig(body: &str) -> String {
    hex(&sha(&[&POW_KEY[..], body.as_bytes()]))[..32].to_owned()
}

/// A fresh challenge: `<time>.<random>.<signature>`.
pub fn pow_challenge(now: u64) -> String {
    let body = format!("{now}.{}", uuid::Uuid::new_v4().simple());
    format!("{body}.{}", pow_sig(&body))
}

/// Check an answer, `<challenge>:<counter>`; Ok with the challenge, for the server to refuse it a second time.
pub fn pow_check(answer: &str, now: u64) -> Result<String, &'static str> {
    let (token, _) = answer.rsplit_once(':').ok_or("pow: missing")?;
    let (body, sig) = token.rsplit_once('.').ok_or("pow: malformed")?;
    if pow_sig(body) != sig { return Err("pow: bad signature") }
    let at: u64 = body.split('.').next().and_then(|t| t.parse().ok()).ok_or("pow: malformed")?;
    if now.saturating_sub(at) > POW_SECS || at > now + 60 { return Err("pow: expired") }
    let d = sha(&[answer.as_bytes()]);
    let zeros = d.iter().map(|b| b.leading_zeros()).scan(true, |go, z| { if !*go { return None } *go = z == 8; Some(z) }).sum::<u32>();
    if zeros < POW_BITS { return Err("pow: wrong") }
    Ok(token.to_owned())
}

/// A hash to check an unknown name against, so a wrong name takes as long as a wrong password.
static DUMMY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| Argon2::default().hash_password(b"nobody").map(|h| h.to_string()).unwrap_or_default());

/// Check a name and password. One answer, in the same time, for a wrong name and a wrong password.
pub fn login(db: &Db, name: &str, password: &str) -> Result<Account, &'static str> {
    const WRONG: &str = "wrong name or password";
    let a = find(db, name);
    let hash = PasswordHash::new(a.as_ref().map_or(DUMMY.as_str(), |a| a.hash.as_str())).map_err(|_| WRONG)?;
    Argon2::default().verify_password(password.as_bytes(), &hash).map_err(|_| WRONG)?;
    a.filter(|a| !a.hash.is_empty()).ok_or(WRONG)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_up_then_log_in() {
        let db = Db::memory();
        assert_eq!(signup(&db, "Ju", None, Some("longenough"), 1).unwrap_err(), "name: 3 to 24 letters, digits, - or _");
        assert_eq!(signup(&db, "Julien", None, Some("short"), 1).unwrap_err(), "password: 8 characters at least");
        let a = signup(&db, "Julien", Some("Ju@Example.org"), Some("pédale-douce"), 1).unwrap();
        assert!(a.id > 0 && a.id < 1 << 53 && a.hash.starts_with("$argon2id$") && !a.hash.contains("pédale"));
        assert_eq!(signup(&db, "julien", None, Some("autrechose"), 2).unwrap_err(), "name taken");
        assert_eq!(login(&db, "JULIEN", "pédale-douce").unwrap().id, a.id);
        assert_eq!(login(&db, "Julien", "pedale-douce").unwrap_err(), "wrong name or password");
        assert_eq!(login(&db, "nobody", "pédale-douce").unwrap_err(), "wrong name or password");
        assert_eq!(by_id(&db, a.id).unwrap().name, "Julien");

        // A new password needs the old one; the account goes the same way.
        assert_eq!(set_password(&db, "julien", "wrong-one", "nouveau-mdp").unwrap_err(), "wrong name or password");
        assert_eq!(set_password(&db, "julien", "pédale-douce", "court").unwrap_err(), "password: 8 characters at least");
        assert_eq!(set_password(&db, "julien", "pédale-douce", "nouveau-mdp").unwrap().id, a.id);
        assert!(login(&db, "julien", "pédale-douce").is_err() && login(&db, "julien", "nouveau-mdp").is_ok());
        assert_eq!(delete(&db, "julien", "pédale-douce").unwrap_err(), "wrong name or password");
        assert_eq!(delete(&db, "julien", "nouveau-mdp").unwrap().id, a.id);
        assert!(find(&db, "julien").is_none());
    }

    #[test]
    fn email_and_no_password() {
        let db = Db::memory();
        assert_eq!(signup(&db, "Marie", None, None, 1).unwrap_err(), "password: 8 characters at least");
        assert_eq!(signup(&db, "Marie", Some("marie"), None, 1).unwrap_err(), "email: not an address");
        assert!(!valid_email("a@b") && !valid_email("a b@c.d") && !valid_email("@c.d") && !valid_email("a@.d") && valid_email("a@b.c"));
        let m = signup(&db, "Marie", Some(" Marie@Mail.fr "), None, 1).unwrap();
        assert_eq!(m.email.as_deref(), Some("marie@mail.fr"));
        assert_eq!(signup(&db, "Marie2", Some("marie@mail.fr"), Some("longenough"), 1).unwrap_err(), "email taken");
        // No password: no password works, the same answer as a wrong one.
        assert_eq!(login(&db, "marie", "").unwrap_err(), "wrong name or password");
        assert_eq!(by_email(&db, "MARIE@mail.fr").unwrap().id, m.id);
        assert!(by_email(&db, "nobody@mail.fr").is_none());
        let j = signup(&db, "Julien", None, Some("pédale-douce"), 1).unwrap();
        assert_eq!(set_email(&db, "julien", "pédale-douce", "marie@mail.fr").unwrap_err(), "email taken");
        assert_eq!(set_email(&db, "julien", "pédale-douce", "ju@mail.fr").unwrap().email.as_deref(), Some("ju@mail.fr"));
        assert_eq!(by_email(&db, "ju@mail.fr").unwrap().id, j.id);
        assert!(link_mail("Marie", "https://x/?link=1").1.contains("https://x/?link=1"));
    }

    #[test]
    fn proof_of_work() {
        let now = 1_700_000_000;
        let c = pow_challenge(now);
        let answer = (0u64..).map(|n| format!("{c}:{n}")).find(|a| pow_check(a, now).is_ok()).unwrap();
        assert_eq!(pow_check(&answer, now).unwrap(), c);
        assert_eq!(pow_check(&answer, now + POW_SECS + 1).unwrap_err(), "pow: expired");
        let wrong = (0u64..).map(|n| format!("{c}:{n}")).find(|a| pow_check(a, now).is_err()).unwrap();
        assert_eq!(pow_check(&wrong, now).unwrap_err(), "pow: wrong");
        let mut forged = c.clone();
        forged.replace_range(..1, if c.starts_with('1') { "2" } else { "1" });
        assert_eq!(pow_check(&format!("{forged}:1"), now).unwrap_err(), "pow: bad signature");
        assert_eq!(pow_check("nothing", now).unwrap_err(), "pow: missing");
    }
}

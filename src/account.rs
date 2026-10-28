//! Le Mollet accounts: a name and a password, nothing else, kept in the database (src/db.rs). Passwords are
//! stored as Argon2id hashes (PHC strings with their own salt), never as such. In cloud mode an account is
//! required to ride; at home it is optional (roles for LAN guests). See docs/rbac.md.

use crate::db::Db;
use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// What roles and profiles refer to; the name may be looked up case-insensitively.
    pub id: u64,
    pub name: String,
    hash: String,
    pub created: u64,
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

/// Create an account. Slow on purpose (the hash): call it off the async threads.
pub fn signup(db: &Db, name: &str, password: &str, now: u64) -> Result<Account, &'static str> {
    if !valid_name(name) { return Err("name: 3 to 24 letters, digits, - or _") }
    if password.chars().count() < 8 { return Err("password: 8 characters at least") }
    if password.len() > 128 { return Err("password: 128 characters at most") }
    if find(db, name).is_some() { return Err("name taken") }
    let hash = Argon2::default().hash_password(password.as_bytes()).map_err(|_| "could not hash the password")?.to_string();
    // A random id below 2^53, so the page reads it exactly.
    let id = ((uuid::Uuid::new_v4().as_u128() >> 75) as u64).max(1);
    let a = Account { id, name: name.into(), hash, created: now };
    db.put("accounts", &key(name), &a).map_err(|_| "could not save the account")?;
    Ok(a)
}

/// Check a name and password. One answer for a wrong name and a wrong password.
pub fn login(db: &Db, name: &str, password: &str) -> Result<Account, &'static str> {
    const WRONG: &str = "wrong name or password";
    let a = find(db, name).ok_or(WRONG)?;
    let hash = PasswordHash::new(&a.hash).map_err(|_| WRONG)?;
    Argon2::default().verify_password(password.as_bytes(), &hash).map_err(|_| WRONG)?;
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_up_then_log_in() {
        let db = Db::memory();
        assert_eq!(signup(&db, "Ju", "longenough", 1).unwrap_err(), "name: 3 to 24 letters, digits, - or _");
        assert_eq!(signup(&db, "Julien", "short", 1).unwrap_err(), "password: 8 characters at least");
        let a = signup(&db, "Julien", "pédale-douce", 1).unwrap();
        assert!(a.id > 0 && a.id < 1 << 53 && a.hash.starts_with("$argon2id$") && !a.hash.contains("pédale"));
        assert_eq!(signup(&db, "julien", "autrechose", 2).unwrap_err(), "name taken");
        assert_eq!(login(&db, "JULIEN", "pédale-douce").unwrap().id, a.id);
        assert_eq!(login(&db, "Julien", "pedale-douce").unwrap_err(), "wrong name or password");
        assert_eq!(login(&db, "nobody", "pédale-douce").unwrap_err(), "wrong name or password");
        assert_eq!(by_id(&db, a.id).unwrap().name, "Julien");
    }
}

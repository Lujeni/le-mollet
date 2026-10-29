//! Le Mollet accounts, on torii (https://github.com/cmackenzie1/torii-rs): users, sessions, magic links and
//! passwords live in torii's own tables, in the same SQLite file as the rest of the server. On a public server an
//! account is an e-mail and a name (out of the e-mail until renamed), no password: every sign-in is a link mailed
//! to the address, and the account is provisional until the first one is used. At home (accounts optional, roles
//! for LAN guests) it is a name and a password, torii's password service behind a synthetic `<name>@local` e-mail.
//! The rest of the server files an account under a `u64` (roles, profiles, ride owners): `account_id` folds
//! torii's id into one. See docs/rbac.md.

use std::sync::Arc;
use torii::sqlite::{SqliteRepositoryProvider, SqliteStorage};
use torii::{SecureToken, Session, SessionConfig, SessionProviderType, SessionToken, TokenPurpose, Torii, User, UserId};
use torii_core::repositories::{RepositoryProvider, TokenRepository, UserRepository};

pub type Error = &'static str;

/// Sessions (and the cookie, src/web.rs) last this long.
pub const SESSION_DAYS: i64 = 30;

pub struct Auth {
    torii: Torii<Repos>,
    repos: Arc<Repos>,
    pool: sqlx::SqlitePool,
}

impl Auth {
    /// torii's tables in `path` (the server's SQLite file), created or migrated; `":memory:"` for a test.
    pub async fn open(path: &str) -> anyhow::Result<Self> {
        use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
        use std::str::FromStr;
        let memory = path == ":memory:";
        let opts = SqliteConnectOptions::from_str(if memory { "sqlite::memory:" } else { path })?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(5));
        // One connection in memory (each would be its own database), a few on a file.
        let pool = SqlitePoolOptions::new().max_connections(if memory { 1 } else { 4 }).connect_with(opts).await?;
        let storage = SqliteStorage::new(pool.clone());
        storage.migrate().await.map_err(|e| anyhow::anyhow!("accounts: {e}"))?;
        sqlx::query("CREATE TABLE IF NOT EXISTS tokens (id INTEGER PRIMARY KEY, user_id TEXT NOT NULL, token TEXT NOT NULL UNIQUE, purpose TEXT NOT NULL, used_at INTEGER, expires_at INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE)").execute(&pool).await?;
        let repos = Arc::new(Repos { inner: storage.into_repository_provider(), token: Tokens(pool.clone()) });
        let torii = Torii::new(repos.clone()).with_session_config(SessionConfig { expires_in: chrono::Duration::days(SESSION_DAYS), provider_type: SessionProviderType::Opaque });
        Ok(Self { torii, repos, pool })
    }

    /// The `u64` the rest of the server files an account under: torii's id folded to 53 bits (what the page
    /// reads exactly), never 0.
    pub fn account_id(id: &UserId) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in id.as_str().bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3)
        }
        (h & ((1 << 53) - 1)).max(1)
    }

    /// Signed up by e-mail and never signed in: the mailbox may be a typo, so a new sign-up with the same name or
    /// e-mail takes its place, and `sweep` drops it after a day. A password account is verified from the start.
    pub fn provisional(u: &User) -> bool {
        !u.is_email_verified()
    }

    pub async fn user(&self, id: &UserId) -> Option<User> {
        self.torii.get_user(id).await.ok().flatten()
    }

    pub async fn by_email(&self, email: &str) -> Option<User> {
        self.repos.user().find_by_email(&email.trim().to_ascii_lowercase()).await.ok().flatten()
    }

    /// Names are unique, case aside: "Julien" and "julien" are one account (torii knows no names, hence the query).
    pub async fn by_name(&self, name: &str) -> Option<User> {
        let id: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE lower(name) = lower(?1)").bind(name.trim()).fetch_optional(&self.pool).await.ok().flatten();
        match id {
            Some(id) => self.user(&UserId::new(&id)).await,
            None => None,
        }
    }

    // ponytail: a scan of the ids, for the CLI and a rare lookup; a column when accounts count in thousands
    pub async fn by_account(&self, account: u64) -> Option<User> {
        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM users").fetch_all(&self.pool).await.unwrap_or_default();
        for id in ids {
            let id = UserId::new(&id);
            if Self::account_id(&id) == account {
                return self.user(&id).await;
            }
        }
        None
    }

    /// A public server's sign-up: the e-mail, a name out of it (four digits added if it is taken), provisional
    /// until a link is used; the link's token comes back for the mail (src/mail.rs).
    pub async fn signup_by_email(&self, email: &str) -> Result<(User, SecureToken), Error> {
        let email = email.trim().to_ascii_lowercase();
        if !valid_email(&email) {
            return Err("email: not an address");
        }
        match self.by_email(&email).await {
            Some(u) if Self::provisional(&u) => self.delete(&u.id).await?,
            Some(_) => return Err("email taken"),
            None => {}
        }
        let base = name_from(&email);
        let mut name = None;
        for i in 0..5 {
            let n = if i == 0 { base.clone() } else { format!("{}-{:04}", &base[..base.len().min(19)], uuid::Uuid::new_v4().as_u128() % 10_000) };
            match self.by_name(&n).await {
                Some(u) if Self::provisional(&u) => {
                    self.delete(&u.id).await?;
                    name = Some(n);
                    break;
                }
                Some(_) => continue,
                None => {
                    name = Some(n);
                    break;
                }
            }
        }
        let name = name.ok_or("name taken")?;
        let token = self.torii.magic_link().generate_token(&email).await.map_err(|e| {
            eprintln!("accounts: {e}");
            "could not create the account"
        })?; // creates the user
        let mut user = self.user(&token.user_id).await.ok_or("could not create the account")?;
        user.name = Some(name);
        let user = self.repos.user().update(&user).await.map_err(|_| "could not save the account")?;
        Ok((user, token))
    }

    /// A sign-in link for an account that exists; None when the e-mail has none (the caller answers the same).
    pub async fn link(&self, email: &str) -> Option<(User, SecureToken)> {
        let u = self.by_email(email).await?;
        let t = self.torii.magic_link().generate_token(&u.email).await.ok()?;
        Some((u, t))
    }

    /// The link used, once, within its 15 minutes: a session, and the mailbox proven.
    pub async fn use_link(&self, token: &str, ip: Option<String>) -> Result<(User, Session), Error> {
        let (user, session) = self.torii.magic_link().authenticate(token, None, ip).await.map_err(|_| "link: expired or used")?;
        if !user.is_email_verified() {
            self.torii.set_user_email_verified(&user.id).await.map_err(|_| "could not save the account")?
        }
        Ok((self.user(&user.id).await.unwrap_or(user), session))
    }

    /// At home: a name and a password (8 to 128 characters), signed in at once.
    pub async fn signup_password(&self, name: &str, password: &str, ip: Option<String>) -> Result<(User, Session), Error> {
        let name = name.trim();
        if !valid_name(name) {
            return Err("name: 3 to 24 letters, digits, - or _");
        }
        if password.chars().count() < 8 {
            return Err("password: 8 characters at least");
        }
        if password.len() > 128 {
            return Err("password: 128 characters at most");
        }
        if self.by_name(name).await.is_some() {
            return Err("name taken");
        }
        let user = self.torii.password().register_with_name(&local_email(name), password, Some(name)).await.map_err(|e| {
            eprintln!("accounts: {e}");
            "name taken"
        })?;
        // Nobody's mailbox: verified from the start, so the account is never provisional.
        self.torii.set_user_email_verified(&user.id).await.map_err(|_| "could not save the account")?;
        let user = self.user(&user.id).await.unwrap_or(user);
        let session = self.new_session(&user.id, ip).await?;
        Ok((user, session))
    }

    /// One answer for a wrong name and a wrong password.
    pub async fn login_password(&self, name: &str, password: &str, ip: Option<String>) -> Result<(User, Session), Error> {
        self.torii.password().authenticate(&local_email(name.trim()), password, None, ip).await.map_err(|_| "wrong name or password")
    }

    /// The password checked, nothing else (the session it makes is dropped at once).
    pub async fn check_password(&self, user: &User, password: &str) -> bool {
        match self.torii.password().authenticate(&user.email, password, None, None).await {
            Ok((_, s)) => {
                let _ = self.torii.delete_session(&s.token).await;
                true
            }
            Err(_) => false,
        }
    }

    pub async fn change_password(&self, id: &UserId, old: &str, new: &str) -> Result<(), Error> {
        if new.chars().count() < 8 {
            return Err("password: 8 characters at least");
        }
        if new.len() > 128 {
            return Err("password: 128 characters at most");
        }
        self.torii.password().change_password(id, old, new).await.map_err(|_| "wrong name or password")
    }

    pub async fn new_session(&self, id: &UserId, ip: Option<String>) -> Result<Session, Error> {
        self.torii.create_session(id, None, ip).await.map_err(|_| "could not sign in")
    }

    /// The session behind a cookie, live, and its user.
    pub async fn session(&self, token: &str) -> Option<(User, Session)> {
        let s = self.torii.get_session(&SessionToken::new(token)).await.ok()?;
        let u = self.user(&s.user_id).await?;
        Some((u, s))
    }

    pub async fn sign_out(&self, token: &str) {
        let _ = self.torii.delete_session(&SessionToken::new(token)).await;
    }

    pub async fn sign_out_all(&self, id: &UserId) {
        let _ = self.torii.delete_sessions_for_user(id).await;
    }

    /// Live sessions of an account: the browsers it is on.
    pub async fn devices(&self, id: &UserId) -> usize {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions WHERE user_id = ?1 AND expires_at > unixepoch()").bind(id.as_str()).fetch_one(&self.pool).await.unwrap_or(0) as usize
    }

    /// Live sessions in all (a gauge, src/metrics.rs).
    pub async fn signed_in(&self) -> usize {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions WHERE expires_at > unixepoch()").fetch_one(&self.pool).await.unwrap_or(0) as usize
    }

    /// A new name, unique still; the rest of the server reads names from the session's user, so nothing else moves.
    pub async fn rename(&self, user: &User, new: &str) -> Result<User, Error> {
        let new = new.trim();
        if !valid_name(new) {
            return Err("name: 3 to 24 letters, digits, - or _");
        }
        if self.by_name(new).await.is_some_and(|o| o.id != user.id) {
            return Err("name taken");
        }
        let u = User { name: Some(new.to_owned()), ..user.clone() };
        self.repos.user().update(&u).await.map_err(|_| "could not save the account")
    }

    /// A new e-mail. The proof stays: the session asking came from the old mailbox.
    // ponytail: no link to the new address first; a verified flag reset would make the account provisional, see sweep
    pub async fn set_email(&self, user: &User, email: &str) -> Result<User, Error> {
        let email = email.trim().to_ascii_lowercase();
        if !valid_email(&email) {
            return Err("email: not an address");
        }
        if self.by_email(&email).await.is_some_and(|o| o.id != user.id) {
            return Err("email taken");
        }
        let u = User { email, ..user.clone() };
        self.repos.user().update(&u).await.map_err(|_| "email taken")
    }

    /// The account gone, its sessions and links with it (foreign keys). Its profiles are the server's (src/web.rs).
    pub async fn delete(&self, id: &UserId) -> Result<(), Error> {
        self.torii.delete_user(id).await.map_err(|_| "could not delete the account")
    }

    /// Provisional accounts older than a day go (a mailbox that never answered); how many went.
    pub async fn sweep(&self) -> usize {
        sqlx::query("DELETE FROM users WHERE email_verified_at IS NULL AND created_at < unixepoch() - 86400").execute(&self.pool).await.map_or(0, |r| r.rows_affected() as usize)
    }
}

/// torii's SQLite storage leaves its token repository a stub ("not yet implemented", 0.5.2), so magic links
/// need this one: the `tokens` table (created in `Auth::open`, in the same file), and torii's own repositories
/// for everything else.
struct Repos {
    inner: SqliteRepositoryProvider,
    token: Tokens,
}

#[async_trait::async_trait]
impl RepositoryProvider for Repos {
    type User = <SqliteRepositoryProvider as RepositoryProvider>::User;
    type Session = <SqliteRepositoryProvider as RepositoryProvider>::Session;
    type Password = <SqliteRepositoryProvider as RepositoryProvider>::Password;
    type OAuth = <SqliteRepositoryProvider as RepositoryProvider>::OAuth;
    type Passkey = <SqliteRepositoryProvider as RepositoryProvider>::Passkey;
    type Token = Tokens;
    fn user(&self) -> &Self::User {
        self.inner.user()
    }
    fn session(&self) -> &Self::Session {
        self.inner.session()
    }
    fn password(&self) -> &Self::Password {
        self.inner.password()
    }
    fn oauth(&self) -> &Self::OAuth {
        self.inner.oauth()
    }
    fn passkey(&self) -> &Self::Passkey {
        self.inner.passkey()
    }
    fn token(&self) -> &Self::Token {
        &self.token
    }
    async fn migrate(&self) -> Result<(), torii_core::Error> {
        self.inner.migrate().await
    }
    async fn health_check(&self) -> Result<(), torii_core::Error> {
        self.inner.health_check().await
    }
}

/// Secure tokens (magic links, password resets) in SQLite: random, dated, spent once.
struct Tokens(sqlx::SqlitePool);

fn db_err(e: sqlx::Error) -> torii_core::Error {
    torii_core::Error::Storage(torii_core::error::StorageError::Database(e.to_string()))
}

fn at(ts: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(ts, 0).unwrap_or_default()
}

#[async_trait::async_trait]
impl TokenRepository for Tokens {
    async fn create_token(&self, user_id: &UserId, purpose: TokenPurpose, expires_in: chrono::Duration) -> Result<SecureToken, torii_core::Error> {
        let now = chrono::Utc::now();
        let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let expires = now + expires_in;
        sqlx::query("INSERT INTO tokens (user_id, token, purpose, expires_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)")
            .bind(user_id.as_str())
            .bind(&token)
            .bind(purpose.as_str())
            .bind(expires.timestamp())
            .bind(now.timestamp())
            .execute(&self.0)
            .await
            .map_err(db_err)?;
        Ok(SecureToken::new(user_id.clone(), token, purpose, None, expires, now, now))
    }

    /// Spent as it is checked: one UPDATE, so two clicks at once cannot both win.
    async fn verify_token(&self, token: &str, purpose: TokenPurpose) -> Result<Option<SecureToken>, torii_core::Error> {
        let now = chrono::Utc::now();
        let row: Option<(String, i64, i64)> =
            sqlx::query_as("UPDATE tokens SET used_at = ?1, updated_at = ?1 WHERE token = ?2 AND purpose = ?3 AND used_at IS NULL AND expires_at > ?1 RETURNING user_id, expires_at, created_at")
                .bind(now.timestamp())
                .bind(token)
                .bind(purpose.as_str())
                .fetch_optional(&self.0)
                .await
                .map_err(db_err)?;
        Ok(row.map(|(uid, exp, created)| SecureToken::new(UserId::new(&uid), token.to_owned(), purpose, Some(now), at(exp), at(created), now)))
    }

    async fn check_token(&self, token: &str, purpose: TokenPurpose) -> Result<bool, torii_core::Error> {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM tokens WHERE token = ?1 AND purpose = ?2 AND used_at IS NULL AND expires_at > ?3")
            .bind(token)
            .bind(purpose.as_str())
            .bind(chrono::Utc::now().timestamp())
            .fetch_one(&self.0)
            .await
            .map_err(db_err)?;
        Ok(n > 0)
    }

    async fn cleanup_expired_tokens(&self) -> Result<(), torii_core::Error> {
        sqlx::query("DELETE FROM tokens WHERE expires_at < ?1").bind(chrono::Utc::now().timestamp()).execute(&self.0).await.map_err(db_err)?;
        Ok(())
    }
}

/// 3 to 24 letters, digits, `-` or `_`: a name to type and to show, nothing to escape.
pub fn valid_name(name: &str) -> bool {
    (3..=24).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// `a@b.c`, 254 characters at most, no spaces: enough to send to, the mail server says the rest.
pub fn valid_email(email: &str) -> bool {
    let Some((user, host)) = email.split_once('@') else { return false };
    email.len() <= 254
        && !user.is_empty()
        && host.contains('.')
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && email.matches('@').count() == 1
}

/// A name out of an e-mail, for a public server's sign-up (which asks for no name): the part before the @, kept to
/// what names allow; `Auth::signup_by_email` adds digits if it is taken. Changed later with `Auth::rename`.
pub fn name_from(email: &str) -> String {
    let local = email.split('@').next().unwrap_or("");
    let mut n: String = local.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '-' }).take(24).collect();
    while n.contains("--") {
        n = n.replace("--", "-")
    }
    let n = n.trim_matches('-').to_owned();
    if n.len() < 3 { format!("rider-{}", &uuid::Uuid::new_v4().simple().to_string()[..4]) } else { n }
}

/// The e-mail behind a home account: a name, nobody's mailbox.
fn local_email(name: &str) -> String {
    format!("{}@home.invalid", name.to_ascii_lowercase()) // a reserved domain, a shape torii accepts
}

/// Shown to nobody: a home account's synthetic address.
pub fn public_email(u: &User) -> Option<&str> {
    (!u.email.ends_with("@home.invalid")).then_some(u.email.as_str())
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
    for p in parts {
        h.update(p)
    }
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
    if pow_sig(body) != sig {
        return Err("pow: bad signature");
    }
    let at: u64 = body.split('.').next().and_then(|t| t.parse().ok()).ok_or("pow: malformed")?;
    if now.saturating_sub(at) > POW_SECS || at > now + 60 {
        return Err("pow: expired");
    }
    let d = sha(&[answer.as_bytes()]);
    let zeros = d
        .iter()
        .map(|b| b.leading_zeros())
        .scan(true, |go, z| {
            if !*go {
                return None;
            }
            *go = z == 8;
            Some(z)
        })
        .sum::<u32>();
    if zeros < POW_BITS {
        return Err("pow: wrong");
    }
    Ok(token.to_owned())
}

/// A sign-in link's mail, for src/mail.rs: subject, text, HTML. The token is torii's (one use, a quarter of an hour).
/// Written to look like what it is to a spam filter: who sent it, why, where it goes, in text and in HTML.
pub fn link_mail(name: &str, url: &str) -> (String, String, String) {
    let site = url.split("/?").next().unwrap_or(url);
    let subject = "Ton lien pour rouler sur Le Mollet".to_owned();
    let text = format!(
        "Salut {name},\n\nQuelqu'un (toi, sans doute) a demandé à se connecter à ton compte Le Mollet sur {site}.\nVoici ton lien de connexion, valable 15 minutes et une seule fois :\n\n{url}\n\nSi ce n'est pas toi, ignore ce message : rien ne se passe sans ce lien.\n\nLe Mollet, le home trainer libre et gratuit\n{site}\n"
    );
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let (n, u, s) = (esc(name), esc(url), esc(site));
    let html = format!(
        concat!(
            "<!doctype html><html lang=\"fr\"><body style=\"font-family:Inter,Arial,sans-serif;color:#16161d;background:#fff8e7;padding:24px\">",
            "<div style=\"max-width:520px;margin:0 auto;background:#fff;border:3px solid #16161d;border-radius:16px;padding:24px\">",
            "<p style=\"font-size:22px;font-weight:700;margin:0 0 16px\">Salut {n},</p>",
            "<p>Quelqu'un (toi, sans doute) a demandé à se connecter à ton compte Le Mollet sur <a href=\"{s}\" style=\"color:#16161d\">{s}</a>.</p>",
            "<p style=\"margin:24px 0\"><a href=\"{u}\" style=\"display:inline-block;background:#ffcf1a;color:#16161d;font-weight:700;text-decoration:none;padding:12px 20px;border:3px solid #16161d;border-radius:12px\">Me connecter</a></p>",
            "<p style=\"font-size:13px;color:#55556a\">Le lien vaut 15 minutes et une seule fois. Si le bouton ne marche pas, copie cette adresse :<br><span style=\"word-break:break-all\">{u}</span></p>",
            "<p style=\"font-size:13px;color:#55556a\">Si ce n'est pas toi, ignore ce message : rien ne se passe sans ce lien.</p>",
            "<p style=\"font-size:13px;color:#55556a;margin:24px 0 0\">Le Mollet, le home trainer libre et gratuit · <a href=\"{s}\" style=\"color:#55556a\">{s}</a></p>",
            "</div></body></html>"
        ),
        n = n,
        u = u,
        s = s
    );
    (subject, text, html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_ids_fold_torii_ids_for_the_page() {
        let a = UserId::new("usr_01HZX3M2Z9ABCDEFGHJKMNPQRS");
        let b = UserId::new("usr_01HZX3M2Z9ABCDEFGHJKMNPQRT");
        assert_eq!(Auth::account_id(&a), Auth::account_id(&a)); // the same id, every run, every server
        assert_ne!(Auth::account_id(&a), Auth::account_id(&b));
        assert!(Auth::account_id(&a) < 1 << 53 && Auth::account_id(&a) > 0); // what JavaScript reads exactly
        assert!(valid_name("Ju-lien_2") && !valid_name("Ju") && !valid_name("Julien Thebault") && !valid_name(&"a".repeat(25)));
    }

    #[tokio::test]
    async fn signs_up_by_email_then_by_link() {
        let auth = Auth::open(":memory:").await.unwrap();
        assert_eq!(auth.signup_by_email("marie").await.unwrap_err(), "email: not an address");
        let (m, t) = auth.signup_by_email(" Marie.Dupont@Mail.fr ").await.unwrap();
        assert!(m.email == "marie.dupont@mail.fr" && m.name.as_deref() == Some("marie-dupont") && Auth::provisional(&m));
        assert!(auth.by_email("MARIE.dupont@mail.fr").await.unwrap().id == m.id && auth.by_name("Marie-Dupont").await.unwrap().id == m.id);
        // Provisional: the same e-mail or name signs up again and takes the place (a typo is not a lost name).
        let (m2, _) = auth.signup_by_email("marie.dupont@mail.fr").await.unwrap();
        assert!(m2.id != m.id && auth.use_link(&t.token, None).await.is_err());
        // The link makes it real: a session, the mailbox proven, and nobody takes its place any more.
        let (_, t2) = auth.link("marie.dupont@mail.fr").await.unwrap();
        let (m2, s) = auth.use_link(&t2.token, Some("127.0.0.1".into())).await.unwrap();
        assert!(!Auth::provisional(&m2) && auth.session(s.token.as_str()).await.unwrap().0.id == m2.id);
        assert_eq!(auth.use_link(&t2.token, None).await.unwrap_err(), "link: expired or used");
        assert_eq!(auth.signup_by_email("marie.dupont@mail.fr").await.unwrap_err(), "email taken");
        let (other, _) = auth.signup_by_email("marie.dupont@other.org").await.unwrap();
        assert!(other.name.as_deref().unwrap().starts_with("marie-dupont-") && other.id != m2.id);
        assert!(auth.link("nobody@mail.fr").await.is_none());
        // Devices, sign-out, rename, e-mail, delete.
        assert_eq!(auth.devices(&m2.id).await, 1);
        auth.sign_out(s.token.as_str()).await;
        assert!(auth.session(s.token.as_str()).await.is_none() && auth.devices(&m2.id).await == 0);
        assert_eq!(auth.rename(&m2, "ju").await.unwrap_err(), "name: 3 to 24 letters, digits, - or _");
        assert_eq!(auth.rename(&m2, other.name.as_deref().unwrap()).await.unwrap_err(), "name taken");
        let m2 = auth.rename(&m2, "Marie").await.unwrap(); // torii's update writes every field: keep the latest user
        assert_eq!(m2.name.as_deref(), Some("Marie"));
        assert_eq!(auth.set_email(&m2, "marie.dupont@other.org").await.unwrap_err(), "email taken");
        let m2 = auth.set_email(&m2, "M@mail.fr").await.unwrap();
        assert_eq!(m2.email, "m@mail.fr");
        assert_eq!(Auth::account_id(&m2.id), Auth::account_id(&auth.by_name("marie").await.unwrap().id));
        assert_eq!(auth.by_account(Auth::account_id(&m2.id)).await.unwrap().id, m2.id);
        auth.delete(&m2.id).await.unwrap();
        assert!(auth.by_name("marie").await.is_none() && auth.by_email("m@mail.fr").await.is_none());
    }

    #[tokio::test]
    async fn home_accounts_have_passwords() {
        let auth = Auth::open(":memory:").await.unwrap();
        assert_eq!(auth.signup_password("Ju", "longenough", None).await.unwrap_err(), "name: 3 to 24 letters, digits, - or _");
        assert_eq!(auth.signup_password("Julien", "short", None).await.unwrap_err(), "password: 8 characters at least");
        let (j, s) = auth.signup_password("Julien", "pédale-douce", None).await.unwrap();
        assert!(!Auth::provisional(&j) && public_email(&j).is_none() && auth.session(s.token.as_str()).await.is_some());
        assert_eq!(auth.signup_password("julien", "autrechose", None).await.unwrap_err(), "name taken");
        assert_eq!(auth.login_password("JULIEN", "pédale-douce", None).await.unwrap().0.id, j.id);
        assert_eq!(auth.login_password("Julien", "pedale-douce", None).await.unwrap_err(), "wrong name or password");
        assert_eq!(auth.login_password("nobody", "pédale-douce", None).await.unwrap_err(), "wrong name or password");
        assert!(auth.check_password(&j, "pédale-douce").await && !auth.check_password(&j, "nope").await);
        assert_eq!(auth.change_password(&j.id, "wrong-one", "nouveau-mdp").await.unwrap_err(), "wrong name or password");
        auth.change_password(&j.id, "pédale-douce", "nouveau-mdp").await.unwrap();
        assert!(auth.login_password("julien", "nouveau-mdp", None).await.is_ok());
        auth.sign_out_all(&j.id).await;
        assert_eq!(auth.devices(&j.id).await, 0);
        assert_eq!(auth.sweep().await, 0); // never provisional
    }

    #[test]
    fn names_from_emails() {
        assert_eq!(name_from("julien.thebault@mail.fr"), "julien-thebault");
        assert_eq!(name_from("Ju_2024+ride@x.y"), "Ju_2024-ride");
        assert_eq!(name_from("a.very.long.local.part.for.sure.really@x.y"), "a-very-long-local-part-f");
        assert!(name_from("ab@x.y").starts_with("rider-") && valid_name(&name_from("ab@x.y")) && valid_name(&name_from("...@x.y")));
        assert!(!valid_email("a@b") && !valid_email("a b@c.d") && !valid_email("@c.d") && !valid_email("a@.d") && valid_email("a@b.c"));
        let (_, text, html) = link_mail("Marie", "https://x/?link=1");
        assert!(text.contains("https://x/?link=1") && html.contains("href=\"https://x/?link=1\"") && html.contains("https://x</a>"));
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

//! Role-based access control. Riding needs no account: anyone not signed in is a `Rider`. Signing in (with
//! a Le Mollet account, see account.rs) gives an identity; roles in the database (`le-mollet role`) raise some identities to `Organizer` or `Admin`.
//! Endpoints check permissions (`Perm`), never roles. See docs/rbac.md.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Rider,
    Organizer,
    Admin,
}

/// What a request may do. New features add a variant here and a line in `Role::can`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Perm {
    /// Ride, send Ride Ons, set one's own power.
    Ride,
    /// Open new free-roam worlds.
    CreateWorld,
    /// Schedule events and group rides (to come).
    ManageEvents,
    /// Remove a rider from a road, reset a leaderboard.
    Moderate,
    /// Give and take roles.
    ManageRoles,
}

pub const ALL: [Perm; 5] = [Perm::Ride, Perm::CreateWorld, Perm::ManageEvents, Perm::Moderate, Perm::ManageRoles];

impl Role {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "rider" => Some(Self::Rider),
            "organizer" => Some(Self::Organizer),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    /// The whole policy in one place: each role and what it may do.
    pub fn can(self, p: Perm) -> bool {
        match p {
            Perm::Ride | Perm::CreateWorld => true,
            Perm::ManageEvents => self >= Role::Organizer,
            Perm::Moderate | Perm::ManageRoles => self == Role::Admin,
        }
    }

    pub fn perms(self) -> Vec<Perm> {
        ALL.into_iter().filter(|p| self.can(*p)).collect()
    }
}

/// Who is asking. Anonymous riders have no account here: their profile lives in their browser.
#[derive(Debug, Clone, Serialize)]
pub struct Identity {
    pub role: Role,
    pub name: Option<String>,
    /// Le Mollet account id, when signed in (`account::Auth::account_id`).
    pub account: Option<u64>,
    /// torii's user id behind it (src/account.rs).
    pub user: Option<String>,
}

impl Identity {
    pub const ANONYMOUS: Identity = Identity { role: Role::Rider, name: None, account: None, user: None };

    pub fn can(&self, p: Perm) -> bool {
        self.role.can(p)
    }
}

/// Roles above Rider, by account id. Everyone else who signs in is a Rider.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Roles {
    #[serde(default)]
    pub grants: HashMap<u64, Grant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    pub role: Role,
    /// A label for the list; the identity is the account id.
    #[serde(default)]
    pub note: String,
}

impl Roles {
    /// None yet: everyone rides, nobody administers.
    pub fn load(db: &crate::db::Db) -> Result<Self> {
        let rows: Vec<(String, Grant)> = db.all("roles")?;
        Ok(Self { grants: rows.into_iter().filter_map(|(id, g)| Some((id.parse().ok()?, g))).collect() })
    }

    pub fn save(&self, db: &crate::db::Db) -> Result<()> {
        db.replace("roles", self.grants.iter().map(|(id, g)| (id.to_string(), g)))
    }

    pub fn role_of(&self, account: u64) -> Role {
        self.grants.get(&account).map_or(Role::Rider, |g| g.role)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy() {
        assert!(Role::Rider.can(Perm::Ride) && Role::Rider.can(Perm::CreateWorld));
        assert!(!Role::Rider.can(Perm::ManageEvents) && !Role::Rider.can(Perm::Moderate));
        assert!(Role::Organizer.can(Perm::ManageEvents) && !Role::Organizer.can(Perm::Moderate));
        assert_eq!(Role::Admin.perms(), ALL.to_vec());
    }

    #[test]
    fn roles_by_account() {
        let r: Roles = serde_json::from_str(r#"{"grants":{"42":{"role":"admin","note":"Julien"}}}"#).unwrap();
        assert_eq!((r.role_of(42), r.role_of(7)), (Role::Admin, Role::Rider));
        let db = crate::db::Db::memory();
        assert_eq!(Roles::load(&db).unwrap().grants.len(), 0);
        let mut r = Roles::default();
        r.grants.insert(42, Grant { role: Role::Admin, note: "me".into() });
        r.save(&db).unwrap();
        assert_eq!(Roles::load(&db).unwrap().role_of(42), Role::Admin);
    }
}

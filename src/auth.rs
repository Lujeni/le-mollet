//! Role-based access control. Riding needs no account: anyone not signed in is a `Rider`. Signing in (with
//! Strava, see strava.rs) gives an identity; `roles.json` raises some identities to `Organizer` or `Admin`.
//! Endpoints check permissions (`Perm`), never roles. See docs/rbac.md.

use anyhow::{Context, Result};
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
    /// Strava athlete id, when signed in.
    pub athlete: Option<u64>,
}

impl Identity {
    pub const ANONYMOUS: Identity = Identity { role: Role::Rider, name: None, athlete: None };

    pub fn can(&self, p: Perm) -> bool {
        self.role.can(p)
    }
}

/// Roles above Rider, by Strava athlete id. Everyone else who signs in is a Rider.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Roles {
    #[serde(default)]
    pub grants: HashMap<u64, Grant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    pub role: Role,
    /// A label for the list; the identity is the athlete id.
    #[serde(default)]
    pub note: String,
}

impl Roles {
    /// A missing file is an empty one: everyone rides, nobody administers.
    pub fn load(path: &str) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).with_context(|| format!("parse {path}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("read {path}")),
        }
    }

    pub fn save(&self, path: &str) -> Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self)?).with_context(|| format!("write {path}"))
    }

    pub fn role_of(&self, athlete: u64) -> Role {
        self.grants.get(&athlete).map_or(Role::Rider, |g| g.role)
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
    fn roles_by_athlete() {
        let r: Roles = serde_json::from_str(r#"{"grants":{"42":{"role":"admin","note":"Julien"}}}"#).unwrap();
        assert_eq!((r.role_of(42), r.role_of(7)), (Role::Admin, Role::Rider));
        assert_eq!(Roles::load("/nonexistent/roles.json").unwrap().grants.len(), 0);
    }
}

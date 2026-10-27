//! Sign in with Strava (OAuth 2 authorization code flow, done by the `oauth2` crate). The server's own Strava
//! app comes from the environment: `LE_MOLLET_STRAVA_CLIENT_ID`, `LE_MOLLET_STRAVA_CLIENT_SECRET`, and
//! `LE_MOLLET_PUBLIC_URL` (where riders reach the server, default http://127.0.0.1:8080). See docs/rbac.md.

use anyhow::{Context, Result};
use oauth2::basic::{BasicErrorResponse, BasicRevocationErrorResponse, BasicTokenIntrospectionResponse, BasicTokenType};
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, Client, ClientId, ClientSecret, CsrfToken, EndpointNotSet, EndpointSet, ExtraTokenFields, RedirectUrl,
    Scope, StandardRevocableToken, StandardTokenResponse, TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};

/// What Strava's token response carries besides the tokens: who signed in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extra {
    pub athlete: Athlete,
}
impl ExtraTokenFields for Extra {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Athlete {
    pub id: u64,
    #[serde(default)]
    pub firstname: String,
    #[serde(default)]
    pub lastname: String,
    #[serde(default)]
    pub profile_medium: Option<String>,
}

impl Athlete {
    pub fn name(&self) -> String {
        format!("{} {}", self.firstname.trim(), self.lastname.trim()).trim().to_owned()
    }
}

/// Tokens to act on the athlete's behalf later (upload rides). Access tokens last 6 h; the refresh token
/// changes on each refresh, so the latest one must be kept.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub access: String,
    pub refresh: Option<String>,
    /// Unix seconds.
    pub expires_at: u64,
    pub scope: String,
}

type StravaClient = Client<
    BasicErrorResponse,
    StandardTokenResponse<Extra, BasicTokenType>,
    BasicTokenIntrospectionResponse,
    StandardRevocableToken,
    BasicRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointSet,
>;

/// Read and write the athlete's activities: enough to upload rides later. Strava separates scopes with
/// commas, so they travel as one.
const SCOPES: &str = "read,activity:write";

pub struct Strava {
    client: StravaClient,
    http: ureq::Agent,
}

impl Strava {
    /// The server's Strava app, if one is configured.
    pub fn from_env() -> Result<Option<Self>> {
        let (Ok(id), Ok(secret)) = (std::env::var("LE_MOLLET_STRAVA_CLIENT_ID"), std::env::var("LE_MOLLET_STRAVA_CLIENT_SECRET")) else { return Ok(None) };
        // Strava itself, or a stand-in for tests.
        let strava = std::env::var("LE_MOLLET_STRAVA_URL").unwrap_or_else(|_| "https://www.strava.com".into());
        let base = std::env::var("LE_MOLLET_PUBLIC_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into());
        let client = Client::new(ClientId::new(id))
            .set_client_secret(ClientSecret::new(secret))
            .set_auth_uri(AuthUrl::new(format!("{strava}/oauth/authorize"))?)
            .set_token_uri(TokenUrl::new(format!("{strava}/oauth/token"))?)
            .set_auth_type(AuthType::RequestBody) // Strava wants the client credentials in the form, not in a header
            .set_redirect_uri(RedirectUrl::new(format!("{}/auth/strava/callback", base.trim_end_matches('/'))).context("LE_MOLLET_PUBLIC_URL")?);
        // Following redirects during the token exchange would open the server to SSRF.
        Ok(Some(Self { client, http: ureq::AgentBuilder::new().redirects(0).build() }))
    }

    /// Where to send the browser, and the CSRF state to check when it comes back.
    pub fn authorize_url(&self) -> (String, String) {
        let (url, state) = self.client.authorize_url(CsrfToken::new_random).add_scope(Scope::new(SCOPES.into())).add_extra_param("approval_prompt", "auto").url();
        (url.to_string(), state.secret().clone())
    }

    /// Trade the code from the callback for the athlete and their tokens. Blocking: run it off the async threads.
    pub fn exchange(&self, code: &str) -> Result<(Athlete, Tokens)> {
        let t = self.client.exchange_code(AuthorizationCode::new(code.into())).request(&self.http).map_err(|e| anyhow::anyhow!("Strava token exchange: {e}"))?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
        let tokens = Tokens {
            access: t.access_token().secret().clone(),
            refresh: t.refresh_token().map(|r| r.secret().clone()),
            expires_at: now + t.expires_in().map_or(6 * 3600, |d| d.as_secs()),
            scope: t.scopes().map(|s| s.iter().map(|x| x.as_str()).collect::<Vec<_>>().join(",")).unwrap_or_default(),
        };
        Ok((t.extra_fields().athlete.clone(), tokens))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_carries_the_app_scopes_and_state() {
        // SAFETY: the only test touching these variables; nothing reads them concurrently.
        unsafe {
            std::env::set_var("LE_MOLLET_STRAVA_CLIENT_ID", "12345");
            std::env::set_var("LE_MOLLET_STRAVA_CLIENT_SECRET", "s3cret");
            std::env::set_var("LE_MOLLET_PUBLIC_URL", "https://ride.example.org/");
        }
        let s = Strava::from_env().unwrap().unwrap();
        let (url, state) = s.authorize_url();
        assert!(url.starts_with("https://www.strava.com/oauth/authorize?"));
        for part in ["client_id=12345", "response_type=code", "scope=read%2Cactivity%3Awrite", &format!("state={state}"), "redirect_uri=https%3A%2F%2Fride.example.org%2Fauth%2Fstrava%2Fcallback"] {
            assert!(url.contains(part), "{part} missing in {url}");
        }
        assert!(!url.contains("s3cret"));
        let a: Extra = serde_json::from_str(r#"{"athlete":{"id":42,"firstname":"Julien","lastname":"T","profile_medium":null}}"#).unwrap();
        assert_eq!((a.athlete.id, a.athlete.name()), (42, "Julien T".into()));
    }
}

//! What riders tell us from the page's side tab (a bug, an idea, an opinion): one row each in the database, with what
//! the page saw of the moment (screen, course, ride, trainer, browser, last errors) and what the server knows (account,
//! address, version). Read back by an admin on `GET /admin/feedback`, or straight from the database.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const KINDS: [&str; 3] = ["bug", "idea", "feedback"];
/// Characters of the text; the page's context in bytes of JSON. Both fit a request (src/web.rs reads 16 KB).
pub const MAX_TEXT: usize = 2000;
pub const MAX_PAGE: usize = 6000;
pub const COLL: &str = "feedback";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Feedback {
    /// `<unix ms>-<account>`: sorts by time, unique enough for one account.
    pub id: String,
    /// When, RFC 3339 UTC.
    pub at: String,
    pub kind: String,
    pub text: String,
    pub account: u64,
    pub name: String,
    pub email: String,
    pub ip: String,
    pub user_agent: String,
    /// The server's version and mode.
    pub version: String,
    pub mode: String,
    /// What the page saw, as it sent it.
    pub page: Value,
}

/// The page's post, checked: one of the kinds, a text that says something and fits, a context that fits.
pub fn parse(body: &str) -> Result<(String, String, Value), &'static str> {
    let v: Value = serde_json::from_str(body).map_err(|_| "bad JSON")?;
    let kind = v["kind"].as_str().unwrap_or("feedback");
    if !KINDS.contains(&kind) {
        return Err("kind: bug, idea or feedback");
    }
    let text = v["text"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        return Err("say something");
    }
    if text.chars().count() > MAX_TEXT {
        return Err("text: 2000 characters at most");
    }
    let page = v.get("page").cloned().unwrap_or(Value::Null);
    if page.to_string().len() > MAX_PAGE {
        return Err("page: too much context");
    }
    Ok((kind.to_owned(), text.to_owned(), page))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_post_is_checked() {
        let (kind, text, page) = parse(r##"{"kind":"bug","text":"  the trainer drops  ","page":{"screen":"#ride"}}"##).unwrap();
        assert_eq!((kind.as_str(), text.as_str(), page["screen"].as_str()), ("bug", "the trainer drops", Some("#ride")));
        assert_eq!(parse(r#"{"text":"no kind given"}"#).unwrap().0, "feedback");
        assert_eq!(parse("not json"), Err("bad JSON"));
        assert_eq!(parse(r#"{"kind":"rant","text":"x"}"#), Err("kind: bug, idea or feedback"));
        assert_eq!(parse(r#"{"kind":"idea","text":"   "}"#), Err("say something"));
        assert_eq!(parse(&format!(r#"{{"kind":"idea","text":"{}"}}"#, "a".repeat(MAX_TEXT + 1))), Err("text: 2000 characters at most"));
        assert_eq!(parse(&format!(r#"{{"kind":"idea","text":"ok","page":{{"errors":"{}"}}}}"#, "e".repeat(MAX_PAGE))), Err("page: too much context"));
    }

    #[test]
    fn a_row_round_trips() {
        let db = crate::db::Db::memory();
        let fb = Feedback {
            id: "1700000000000-7".into(),
            at: "2026-10-30T10:00:00Z".into(),
            kind: "idea".into(),
            text: "a dark mode for the HUD".into(),
            account: 7,
            name: "alice".into(),
            email: "alice@mail.fr".into(),
            ip: "10.0.0.1".into(),
            user_agent: "Chrome".into(),
            version: "0.3.0".into(),
            mode: "cloud".into(),
            page: serde_json::json!({ "screen": "#ride" }),
        };
        db.put(COLL, &fb.id, &fb).unwrap();
        assert_eq!(db.all::<Feedback>(COLL).unwrap(), vec![(fb.id.clone(), fb)]);
    }
}

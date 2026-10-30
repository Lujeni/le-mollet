//! The server as a browser sees it: the real binary on a free port with a throwaway database, driven over HTTP.
//! Cloud mode (an account, a mailed link, the settings, friends, quotas) and local mode (a name and a password).
//! Without `LE_MOLLET_SMTP_URL` the server prints each mail on stderr, which is where the links are read.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// No redirects followed: a 302 is an answer this test reads.
static AGENT: std::sync::LazyLock<ureq::Agent> = std::sync::LazyLock::new(|| ureq::builder().redirects(0).build());

struct Server {
    child: Child,
    base: String,
    stderr: Arc<Mutex<String>>,
    /// Mails seen by the last `link`: the next call waits for a new one.
    seen: Mutex<usize>,
    dir: std::path::PathBuf,
}

impl Server {
    fn start(mode: &str) -> Self {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let dir = std::env::temp_dir().join(format!("le-mollet-http-{}-{port}", std::process::id()));
        std::fs::create_dir_all(dir.join("courses")).unwrap();
        let base = format!("http://127.0.0.1:{port}");
        let mut child = Command::new(env!("CARGO_BIN_EXE_le-mollet"))
            .args(["serve", "--mode", mode, "--addr", &format!("127.0.0.1:{port}"), "--courses", dir.join("courses").to_str().unwrap(), "--db", dir.join("t.db").to_str().unwrap()])
            .env("LE_MOLLET_PUBLIC_URL", &base)
            .env("LE_MOLLET_METRICS_TOKEN", "t0k")
            .env_remove("LE_MOLLET_SMTP_URL")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = Arc::new(Mutex::new(String::new()));
        let (out, log) = (child.stderr.take().unwrap(), stderr.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let mut l = log.lock().unwrap();
                l.push_str(&line);
                l.push('\n');
            }
        });
        let s = Self { child, base, stderr, seen: Mutex::new(0), dir };
        let t = Instant::now();
        while s.get("/whoami").0 == 0 {
            assert!(t.elapsed() < Duration::from_secs(30), "server did not come up:\n{}", s.stderr.lock().unwrap());
            std::thread::sleep(Duration::from_millis(100));
        }
        s
    }

    /// Status, body, headers; 0 when the server is not there. A body is a form, or JSON when it starts like one.
    fn call(&self, req: ureq::Request, body: Option<&str>) -> (u16, String, Vec<(String, String)>) {
        let r = match body {
            Some(b) if b.starts_with('{') => req.set("Content-Type", "application/json").send_string(b),
            Some(b) => req.set("Content-Type", "application/x-www-form-urlencoded").send_string(b),
            None => req.call(),
        };
        let r = match r {
            Ok(r) | Err(ureq::Error::Status(_, r)) => r,
            Err(_) => return (0, String::new(), vec![]),
        };
        let headers = r.headers_names().into_iter().map(|h| (h.clone(), r.header(&h).unwrap_or_default().to_owned())).collect();
        (r.status(), r.into_string().unwrap_or_default(), headers)
    }

    fn get(&self, path: &str) -> (u16, String, Vec<(String, String)>) {
        self.call(AGENT.get(&format!("{}{path}", self.base)).set("Host", &self.base[7..]), None)
    }

    /// The next sign-in link the server mails (printed, no SMTP here): its token.
    fn link(&self) -> String {
        let t = Instant::now();
        let mut seen = self.seen.lock().unwrap();
        loop {
            let log = self.stderr.lock().unwrap().clone();
            if log.matches("?link=").count() > *seen
                && let Some(i) = log.rfind("?link=")
            {
                *seen = log.matches("?link=").count();
                return log[i + 6..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
            }
            assert!(t.elapsed() < Duration::from_secs(5), "no link mailed:\n{log}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn mails(&self) -> usize {
        self.stderr.lock().unwrap().matches("?link=").count()
    }

    fn audit(&self, event: &str) -> usize {
        self.stderr.lock().unwrap().matches(&format!("audit event={event} ")).count()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// One browser: its cookie jar is the one session cookie.
#[derive(Clone)]
struct Browser<'a> {
    server: &'a Server,
    cookie: Option<String>,
    /// The address the proxy would report (`X-Forwarded-For`, trusted from loopback in cloud mode): one client each.
    ip: &'static str,
}

impl<'a> Browser<'a> {
    fn new(server: &'a Server, ip: &'static str) -> Self {
        Self { server, cookie: None, ip }
    }

    fn req(&self, method: &str, path: &str) -> ureq::Request {
        let r = AGENT.request(method, &format!("{}{path}", self.server.base)).set("Host", &self.server.base[7..]).set("Sec-Fetch-Site", "same-origin").set("X-Forwarded-For", self.ip);
        match &self.cookie {
            Some(c) => r.set("Cookie", &format!("lm_session={c}")),
            None => r,
        }
    }

    fn get(&self, path: &str) -> (u16, String) {
        let (s, b, _) = self.server.call(self.req("GET", path).set("Sec-Fetch-Site", "same-origin"), None);
        (s, b)
    }

    /// A form post; a `Set-Cookie` in the answer becomes this browser's session.
    fn post(&mut self, path: &str, form: &str) -> (u16, String) {
        let (s, b, h) = self.server.call(self.req("POST", path), Some(form));
        if let Some((_, c)) = h.iter().find(|(k, _)| k.eq_ignore_ascii_case("set-cookie")) {
            let v = c.split(';').next().unwrap_or("").trim_start_matches("lm_session=").to_owned();
            self.cookie = if v.is_empty() { None } else { Some(v) };
        }
        (s, b)
    }

    fn json(&self, path: &str) -> serde_json::Value {
        serde_json::from_str(&self.get(path).1).unwrap_or(serde_json::Value::Null)
    }

    /// The sign-up proof of work: a counter whose SHA-256 with the challenge starts with the asked zero bits.
    fn pow(&self) -> String {
        use sha2::Digest;
        let c = self.json("/auth/challenge");
        let (token, bits) = (c["token"].as_str().unwrap().to_owned(), c["bits"].as_u64().unwrap() as u32);
        (0u64..)
            .map(|n| format!("{token}:{n}"))
            .find(|a| {
                let d = sha2::Sha256::digest(a.as_bytes());
                d.iter()
                    .map(|b| b.leading_zeros())
                    .scan(true, |go, z| {
                        if !*go {
                            return None;
                        }
                        *go = z == 8;
                        Some(z)
                    })
                    .sum::<u32>()
                    >= bits
            })
            .unwrap()
    }

    fn signup(&mut self, form: &str) -> (u16, String) {
        let pow = self.pow();
        self.post("/auth/signup", &format!("{form}&pow={pow}"))
    }
}

#[test]
fn cloud_accounts_links_settings_friends_and_quotas() {
    let s = Server::start("cloud");
    let mut alice = Browser::new(&s, "10.0.0.1");

    // Nobody yet: the sign-in is the only door.
    let me = alice.json("/whoami");
    assert_eq!((me["mode"].as_str(), me["account"].is_null()), (Some("cloud"), true));
    assert_eq!(alice.get("/courses").0, 401);
    assert_eq!(alice.get("/auth/signup").0, 405); // POST only
    assert_eq!(alice.get("/feedback").0, 405);
    // What a sign-up refuses, from a client of its own: each try counts against that client's quota.
    let mut bot = Browser::new(&s, "10.0.0.9");
    assert_eq!(bot.post("/auth/signup", "email=a@b.c").0, 400); // no proof of work
    assert_eq!(bot.signup("name=Alice&password=longenough"), (400, "email: required".into()));
    assert_eq!(bot.signup("email=not-an-address"), (400, "email: not an address".into()));
    assert_eq!(bot.signup("email=fourth@try.io").0, 429); // three an hour per client

    // A sign-up is an e-mail, and a link comes.
    assert_eq!(alice.signup("email=Alice.Dupont@mail.fr"), (202, "link sent".into()));
    let token = s.link();
    assert_eq!(alice.post("/auth/link/use", "t=deadbeef"), (400, "link: expired or used".into()));
    let (status, body) = alice.post("/auth/link/use", &format!("t={token}"));
    assert_eq!(status, 200, "{body}");
    assert!(alice.cookie.is_some());
    assert_eq!(alice.post("/auth/link/use", &format!("t={token}")).0, 400); // once
    let me = alice.json("/whoami");
    assert_eq!((me["name"].as_str(), me["email"].as_str(), me["devices"].as_u64(), me["role"].as_str()), (Some("alice-dupont"), Some("alice.dupont@mail.fr"), Some(1), Some("rider")));
    let account = me["account"].as_u64().unwrap();
    assert!(account > 0 && account < 1 << 53);
    assert!(alice.get("/courses").0 == 200);

    // The feedback tab: a rider's word kept with what the page saw; a stranger has none, a rider reads none back.
    assert_eq!(bot.post("/feedback", r#"{"kind":"bug","text":"x"}"#).0, 401);
    assert_eq!(alice.post("/feedback", r#"{"kind":"rant","text":"x"}"#), (400, "kind: bug, idea or feedback".into()));
    assert_eq!(alice.post("/feedback", r#"{"kind":"idea","text":"  "}"#), (400, "say something".into()));
    let (status, body) = alice.post("/feedback", r##"{"kind":"bug","text":"the trainer drops at km 3","page":{"screen":"#ride","trainer":{"name":"KICKR CORE"}}}"##);
    assert_eq!(status, 201, "{body}");
    assert_eq!(alice.get("/admin/feedback").0, 403);
    assert_eq!(s.audit("feedback"), 1);

    // No passwords on a public server.
    assert_eq!(alice.post("/auth/login", "name=alice-dupont&password=longenough").0, 400);
    assert_eq!(alice.post("/auth/password", "password=x&new=longenough2").0, 400);

    // A sign-up with a known address: the same answer, and nothing sent.
    let mails = s.mails();
    assert_eq!(alice.signup("email=alice.dupont@mail.fr"), (202, "link sent".into()));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!((s.mails(), s.audit("signup.existing")), (mails, 1));
    // A link for an unknown address: the same answer, nothing sent; for a known one, a mail.
    assert_eq!(alice.post("/auth/link", "email=nobody@mail.fr").0, 202);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(s.mails(), mails);
    assert_eq!(alice.post("/auth/link", "email=ALICE.dupont@mail.fr").0, 202);
    s.link();
    assert_eq!(s.mails(), mails + 1);

    // The settings trust the session.
    assert_eq!(alice.post("/auth/name", "name=Al").0, 400);
    assert_eq!(alice.post("/auth/name", "name=Alice").0, 204);
    assert_eq!(alice.post("/auth/email", "email=alice@mail.fr").0, 204);
    let me = alice.json("/whoami");
    assert_eq!((me["name"].as_str(), me["email"].as_str()), (Some("Alice"), Some("alice@mail.fr")));

    // A profile, and a friend. Bob has his own account, and may not touch Alice's profile.
    let p1 = "a1111111-1111-4111-8111-111111111111";
    let p3 = "b3333333-3333-4333-8333-333333333333";
    assert_eq!(alice.post(&format!("/rider?pid={p1}&weight=70&ftp=250"), "").0, 200);
    let mut bob = Browser::new(&s, "10.0.0.2");
    assert_eq!(bob.signup("email=bob@mail.fr").0, 202);
    let t = s.link();
    assert_eq!(bob.post("/auth/link/use", &format!("t={t}")).0, 200);
    assert_eq!(bob.post(&format!("/rider?pid={p3}&weight=80&ftp=300"), "").0, 200);
    assert_eq!(bob.get(&format!("/friend?pid={p1}")).0, 403);
    let handle = alice.json(&format!("/friend?pid={p1}&me=Alice&country=FR"))["handle"].as_str().unwrap().to_owned();
    assert!(handle.starts_with("Alice#"));
    assert_eq!(bob.get("/friend/ask?pid=x").0, 405);
    let h = handle.replace('#', "%23");
    assert_eq!(bob.post(&format!("/friend/ask?pid={p3}&h={h}"), "").0, 400); // no handle of his own yet
    let bh = bob.json(&format!("/friend?pid={p3}"))["handle"].as_str().unwrap().replace('#', "%23"); // his first visit makes one
    assert_eq!(bob.post(&format!("/friend/ask?pid={p3}&h={h}"), "").0, 200);
    assert_eq!(alice.post(&format!("/friend/accept?pid={p1}&h={bh}"), "").0, 200);
    let friends = alice.json(&format!("/friend?pid={p1}"));
    assert_eq!(friends["friends"][0]["name"].as_str(), Some("bob"));
    assert!(!friends.to_string().contains(p3)); // never a profile id

    // Metrics: a token in cloud mode, and the counters this test fed.
    assert_eq!(s.get("/metrics").0, 404); // without the token the endpoint is not there
    let (status, body, _) = s.call(AGENT.get(&format!("{}/metrics", s.base)).set("Host", &s.base[7..]).set("Authorization", "Bearer t0k"), None);
    assert_eq!(status, 200);
    assert!(
        body.contains("le_mollet_signins_total{result=\"signup\"} 2") && body.contains("le_mollet_signins_total{result=\"link\"} 2") && body.contains("le_mollet_mails_total{result=\"sent\"}"),
        "{body}"
    );

    // Out, everywhere; back by a link; gone for good.
    let other = alice.clone();
    assert_eq!(alice.post("/auth/logout?all=1", "").0, 302);
    assert!(other.json("/whoami")["account"].is_null());
    assert_eq!(alice.post("/auth/link", "email=alice@mail.fr").0, 202);
    let t = s.link();
    let mut alice = Browser::new(&s, "10.0.0.1");
    assert_eq!(alice.post("/auth/link/use", &format!("t={t}")).0, 200);
    assert_eq!(alice.post("/auth/delete", "").0, 302);
    assert!(alice.json("/whoami")["account"].is_null());
    let mails = s.mails();
    assert_eq!(bob.post("/auth/link", "email=alice@mail.fr").0, 202);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(s.mails(), mails); // no account, no mail
    assert!(bob.json(&format!("/friend?pid={p3}"))["friends"].as_array().unwrap().is_empty()); // the friendship went with her
    assert_eq!((s.audit("signup"), s.audit("signin"), s.audit("account.deleted"), s.audit("name.changed")), (2, 3, 1, 1));
}

#[test]
fn home_accounts_have_passwords_and_the_pc_is_admin() {
    let s = Server::start("local");
    let mut me = Browser::new(&s, "10.0.0.1"); // ignored at home: no proxy, every browser is the PC's
    let who = me.json("/whoami");
    assert_eq!((who["mode"].as_str(), who["role"].as_str(), who["account"].is_null()), (Some("local"), Some("admin"), true)); // loopback: the owner
    assert_eq!(me.get("/courses").0, 200); // no account needed at home

    // The owner's word counts without an account, and the owner reads everything back, newest first.
    assert_eq!(me.post("/feedback", r##"{"kind":"idea","text":"a dark HUD","page":{"screen":"#home"}}"##).0, 201);
    assert_eq!(me.post("/feedback", r#"{"text":"nice"}"#).0, 201);
    let all = me.json("/admin/feedback");
    let rows = all.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0]["kind"].as_str(), rows[0]["text"].as_str(), rows[1]["kind"].as_str(), rows[1]["page"]["screen"].as_str()), (Some("feedback"), Some("nice"), Some("idea"), Some("#home")));
    assert_eq!((rows[1]["mode"].as_str(), rows[1]["account"].as_u64(), rows[1]["version"].is_string(), rows[1]["at"].as_str().unwrap().ends_with('Z')), (Some("local"), Some(0), true, true));

    assert_eq!(me.signup("name=Julien&password=short"), (400, "password: 8 characters at least".into()));
    let (status, body) = me.signup("name=Julien&password=p%C3%A9dale-douce");
    assert_eq!(status, 200, "{body}");
    assert!(me.cookie.is_some() && me.json("/whoami")["name"] == "Julien" && me.json("/whoami")["email"].is_null());
    assert_eq!(me.signup("name=julien&password=autrechose"), (400, "name taken".into()));

    let mut again = Browser::new(&s, "10.0.0.2");
    assert_eq!(again.post("/auth/login", "name=JULIEN&password=wrong-one"), (400, "wrong name or password".into()));
    assert_eq!(again.post("/auth/login", "name=nobody&password=wrong-one"), (400, "wrong name or password".into()));
    assert_eq!(again.post("/auth/login", "name=JULIEN&password=p%C3%A9dale-douce").0, 200);
    assert_eq!(again.json("/whoami")["devices"].as_u64(), Some(2));

    // A new password signs the other browser out and keeps this one on a fresh session.
    assert_eq!(again.post("/auth/password", "password=wrong&new=nouveau-mdp").0, 400);
    assert_eq!(again.post("/auth/password", "password=p%C3%A9dale-douce&new=nouveau-mdp").0, 204);
    assert!(me.json("/whoami")["account"].is_null() && again.json("/whoami")["name"] == "Julien");
    assert_eq!(me.post("/auth/login", "name=julien&password=nouveau-mdp").0, 200);

    // Each setting asks for the password at home.
    assert_eq!(me.post("/auth/name", "name=Jules&password=nope").0, 400);
    assert_eq!(me.post("/auth/name", "name=Jules&password=nouveau-mdp").0, 204);
    assert_eq!(me.post("/auth/delete", "password=nope"), (400, "wrong name or password".into()));
    assert_eq!(me.post("/auth/delete", "password=nouveau-mdp").0, 302);
    assert!(again.json("/whoami")["account"].is_null());
    assert_eq!(s.audit("signin.refused"), 2);
}

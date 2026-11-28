//! An in-process mock Forgejo for the e2e tests: one repository (acme/app) with one open pull request (#7, `feature`
//! into `main`), 60 commit statuses on its head (two pages of 50: the >50 pagination fix of 0.10.2) and
//! conversation comments the app can add to. Plain HTTP/1.1 on 127.0.0.1, one thread per connection; nothing
//! leaves the machine. Every request is logged for the tests to assert on.
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

pub const TOKEN: &str = "e2e-token";
pub const USER: &str = "alice";
/// Fixed, so the URLs the app shows (and the screenshots) don't change between runs.
pub const PORT: u16 = 47_291;
pub const STATUSES: usize = 60;
pub const PENDING: usize = 2;

#[derive(Default)]
pub struct State {
    /// "GET /api/v1/repos/acme/app/pulls?state=open&page=1&limit=50"
    pub requests: Vec<String>,
    pub comments: Vec<Value>,
    pub pr_head: String,
    pub pr_base: String,
    pub diff: String,
}

pub struct Forge {
    pub url: String,
    pub state: Arc<Mutex<State>>,
}

impl Forge {
    pub fn start() -> Forge {
        let l = TcpListener::bind(("127.0.0.1", PORT)).unwrap_or_else(|e| panic!("mock forge: port {PORT} is taken ({e})"));
        let state = Arc::new(Mutex::new(State::default()));
        let s = state.clone();
        std::thread::spawn(move || for c in l.incoming().flatten() {
            let s = s.clone();
            std::thread::spawn(move || { let _ = serve(c, &s); });
        });
        Forge { url: format!("http://127.0.0.1:{PORT}"), state }
    }

    /// A new fixture: the PR's head and base, the diff it shows; requests and comments cleared.
    pub fn reset(&self, head: &str, base: &str, diff: String) {
        let mut s = self.state.lock().unwrap();
        *s = State { pr_head: head.into(), pr_base: base.into(), diff, comments: vec![comment(1, "bob", "Looks good, one nit inline.")], ..Default::default() };
    }

    pub fn requests(&self) -> Vec<String> { self.state.lock().unwrap().requests.clone() }
}

fn comment(id: i64, who: &str, body: &str) -> Value {
    json!({ "id": id, "user": user(who), "body": body, "created_at": "2026-01-16T10:00:00Z" })
}

fn user(login: &str) -> Value { json!({ "login": login, "avatar_url": null }) }

fn pr(s: &State, base: &str) -> Value {
    json!({
        "id": 1007, "number": 7, "title": "Add colors", "body": "Adds a colors module.\n\n- red\n- green\n- blue",
        "state": "open", "draft": false, "user": user("bob"),
        "head": { "ref": "feature", "sha": s.pr_head, "label": "feature" },
        "base": { "ref": "main", "sha": s.pr_base, "label": "main" },
        "additions": 4, "deletions": 0, "changed_files": 2, "comments": s.comments.len(), "mergeable": true, "merged": false,
        "merged_at": null, "requested_reviewers": [user(USER)], "html_url": format!("{base}/acme/app/pulls/7"),
        "updated_at": "2026-01-16T10:00:00Z", "created_at": "2026-01-15T09:00:00Z",
    })
}

fn statuses(page: usize, limit: usize) -> Value {
    let all: Vec<Value> = (0..STATUSES).map(|i| json!({
        "id": i + 1, "status": if i >= STATUSES - PENDING { "pending" } else { "success" },
        "context": format!("ci/job-{:02}", i + 1), "description": "e2e", "target_url": null,
        "created_at": "2026-01-16T09:00:00Z", "updated_at": "2026-01-16T09:30:00Z",
    })).collect();
    let page: Vec<Value> = all.into_iter().skip((page.max(1) - 1) * limit).take(limit).collect();
    // Forgejo's total_count is the page's, not the commit's: what made 0.10.1 stop after page 1.
    json!({ "state": "pending", "total_count": page.len(), "statuses": page })
}

fn serve(c: TcpStream, state: &Mutex<State>) -> std::io::Result<()> {
    let mut r = BufReader::new(c.try_clone()?);
    let mut w = c;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 { return Ok(()) }
        let mut parts = line.split_whitespace();
        let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
        let (mut len, mut auth) = (0usize, String::new());
        loop {
            let mut h = String::new();
            r.read_line(&mut h)?;
            let h = h.trim_end();
            if h.is_empty() { break }
            let (k, v) = h.split_once(':').unwrap_or((h, ""));
            match k.to_ascii_lowercase().as_str() { "content-length" => len = v.trim().parse().unwrap_or(0), "authorization" => auth = v.trim().into(), _ => {} }
        }
        let mut body = vec![0; len];
        r.read_exact(&mut body)?;
        let (status, ctype, out) = route(&method, &target, &auth, &body, state);
        write!(w, "HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\r\n", out.len())?;
        w.write_all(&out)?;
        w.flush()?;
    }
}

fn route(method: &str, target: &str, auth: &str, body: &[u8], state: &Mutex<State>) -> (u16, &'static str, Vec<u8>) {
    let mut s = state.lock().unwrap();
    s.requests.push(format!("{method} {target}"));
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let q = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
    let js = |status, v: Value| (status, "application/json", serde_json::to_vec(&v).unwrap());
    let Some(api) = path.strip_prefix("/api/v1") else { return js(404, json!({ "message": "not found" })) };
    if auth.rsplit(' ').next() != Some(TOKEN) { return js(401, json!({ "message": "token is required" })) }
    let base = format!("http://127.0.0.1:{PORT}");
    let repo = "/repos/acme/app";
    match (method, api) {
        ("GET", "/version") => js(200, json!({ "version": "11.0.0" })),
        ("GET", "/user") => js(200, user(USER)),
        ("GET", p) if p == repo => js(200, json!({ "name": "app", "full_name": "acme/app", "default_branch": "main", "owner": user("acme") })),
        ("GET", p) if p == format!("{repo}/pulls") => {
            let open = matches!(q("state").as_str(), "open" | "all" | "");
            js(200, if open && q("page").parse().unwrap_or(1) <= 1 { json!([pr(&s, &base)]) } else { json!([]) })
        }
        ("GET", p) if p == format!("{repo}/pulls/7") => js(200, pr(&s, &base)),
        ("GET", p) if p == format!("{repo}/pulls/7.diff") => (200, "text/plain; charset=utf-8", s.diff.clone().into_bytes()),
        ("GET", p) if p.starts_with(&format!("{repo}/pulls/7/")) => js(200, json!([])), // reviews, commits, comments
        ("GET", p) if p == format!("{repo}/issues/7/comments") => js(200, Value::Array(s.comments.clone())),
        ("POST", p) if p == format!("{repo}/issues/7/comments") => {
            let b: Value = serde_json::from_slice(body).unwrap_or_default();
            let c = comment(s.comments.len() as i64 + 1, USER, b["body"].as_str().unwrap_or(""));
            s.comments.push(c.clone());
            js(201, c)
        }
        ("GET", p) if p.starts_with(&format!("{repo}/commits/")) && p.ends_with("/status") =>
            js(200, statuses(q("page").parse().unwrap_or(1), q("limit").parse().unwrap_or(30))),
        ("GET", p) if p == format!("{repo}/actions/runs") => js(200, json!({ "workflow_runs": [], "total_count": 0 })),
        ("GET", p) if p == format!("{repo}/commits") => js(200, json!([])),
        _ => js(404, json!({ "message": "not found" })),
    }
}

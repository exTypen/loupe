//! Forgejo/Gitea API v1, or GitHub's REST API when `github` is set. GitHub's API is close enough to share the
//! models: the methods below convert where the two differ, so the views never know which server they talk to.

use crate::ci::{CiAction, CiFeatures, GhJob, GhRun};
use crate::models::*;
use crate::{Cancel, Error, RemoteRepo, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Client {
    pub base_url: String,
    token: String,
    pub github: bool,
    cancel: Option<Cancel>,
}

/// A response with any status: callers that need the status (logs, PR create) read it themselves.
pub struct Raw {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_range: Option<String>,
}

pub const PULL_PAGE: usize = 50;

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .user_agent("Loupe")
            // GitHub answers a log request with a 302 to a signed blob URL, which must not receive the token.
            .redirect_auth_headers(ureq::config::RedirectAuthHeaders::SameHost)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
            .build()
            .into()
    })
}

/// GitHub sends max-age=60; URLSession was told to revalidate instead of serving polls from its cache for a
/// minute. Same here: GETs carry If-None-Match, and GitHub doesn't count a 304 against the rate limit.
/// Keyed by token + URL: two accounts can see different things at the same URL.
type EtagCache = Mutex<HashMap<String, (String, Arc<Vec<u8>>)>>;
fn etags() -> &'static EtagCache {
    static C: OnceLock<EtagCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { out.push(b as char) } else { out.push_str(&format!("%{b:02X}")) }
    }
    out
}

type Query<'a> = Vec<(&'a str, String)>;

impl Client {
    pub fn new(base_url: &str, token: &str, github: bool) -> Self {
        Self { base_url: base_url.into(), token: token.into(), github, cancel: None }
    }

    /// The same client, whose requests stop with [`Error::Cancelled`] once `c` is cancelled.
    pub fn with_cancel(&self, c: &Cancel) -> Self {
        Self { cancel: Some(c.clone()), ..self.clone() }
    }

    pub fn cancel(&self) -> Option<&Cancel> {
        self.cancel.as_ref()
    }

    fn api_root(&self) -> String {
        let base = self.base_url.trim_matches('/');
        if !self.github { return format!("{base}/api/v1"); }
        if base == "https://github.com" { "https://api.github.com".into() } else { format!("{base}/api/v3") } // Enterprise Server
    }

    pub fn server_name(&self) -> &'static str {
        if self.github { "GitHub" } else { "Forgejo" }
    }

    /// Page-size parameter: Forgejo `limit`, GitHub `per_page`.
    fn size_key(&self) -> &'static str {
        if self.github { "per_page" } else { "limit" }
    }
    fn paged(&self, page: usize, size: usize) -> Query<'static> {
        vec![("page", page.to_string()), (self.size_key(), size.to_string())]
    }

    fn check(&self) -> Result<()> {
        self.cancel.as_ref().map_or(Ok(()), |c| c.check())
    }

    /// One request; any status comes back. `accept` "application/json" means GitHub's own JSON type on GitHub.
    pub fn raw(&self, method: &str, path: &str, query: &[(&str, String)], body: Option<Vec<u8>>, accept: &str, range: Option<&str>) -> Result<Raw> {
        self.check()?;
        let mut url = self.api_root() + path;
        if !query.is_empty() {
            url.push('?');
            url += &query.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect::<Vec<_>>().join("&");
        }
        let accept = if self.github && accept == "application/json" { "application/vnd.github+json" } else { accept };
        let mut hs = vec![("Authorization".to_string(), format!("token {}", self.token)), ("Accept".to_string(), accept.to_string())];
        if let Some(r) = range {
            hs.push(("Range".into(), r.into()));
            hs.push(("Accept-Encoding".into(), "identity".into())); // byte offsets are of the log, not of a gzip stream
        }
        let cache_key = (method == "GET" && range.is_none()).then(|| format!("{}\n{}\n{url}", self.token, accept));
        let cached = cache_key.as_ref().and_then(|k| etags().lock().unwrap().get(k).cloned());
        if let Some((etag, _)) = &cached { hs.push(("If-None-Match".into(), etag.clone())); }
        if body.is_some() { hs.push(("Content-Type".into(), "application/json".into())); }

        fn with<B>(mut rb: ureq::RequestBuilder<B>, hs: &[(String, String)]) -> ureq::RequestBuilder<B> {
            for (k, v) in hs { rb = rb.header(k, v); }
            rb
        }
        let a = agent();
        let sent = match (method, body) {
            ("GET", _) => with(a.get(&url), &hs).call(),
            ("DELETE", _) => with(a.delete(&url), &hs).call(),
            (m, b) => {
                let rb = match m {
                    "PUT" => a.put(&url),
                    "PATCH" => a.patch(&url),
                    _ => a.post(&url),
                };
                let rb = with(rb, &hs);
                match b { Some(b) => rb.send(&b[..]), None => rb.send_empty() }
            }
        };
        self.check()?;
        let mut resp = sent.map_err(|e| Error::msg(e.to_string()))?;
        let status = resp.status().as_u16();
        let header = |n: &str| resp.headers().get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
        let content_range = header("content-range");
        let etag = header("etag");
        if status == 304
            && let Some((_, b)) = cached { return Ok(Raw { status: 200, body: b.to_vec(), content_range }); }
        let body = resp.body_mut().with_config().limit(u64::MAX).read_to_vec().map_err(|e| {
            if self.cancel.as_ref().is_some_and(|c| c.is_cancelled()) { Error::Cancelled } else { Error::msg(e.to_string()) }
        })?;
        self.check()?;
        if let (Some(k), Some(tag), true) = (cache_key, etag, (200..300).contains(&status)) {
            let mut c = etags().lock().unwrap();
            if c.len() > 300 { c.clear(); } // ponytail: wholesale reset, an LRU if hot entries ever matter
            c.insert(k, (tag, Arc::new(body.clone())));
        }
        Ok(Raw { status, body, content_range })
    }

    fn fail(&self, status: u16, body: &[u8]) -> Error {
        let msg = serde_json::from_slice::<serde_json::Value>(body).ok().and_then(|v| v.get("message")?.as_str().map(str::to_string));
        Error::Http { server: self.server_name().into(), status, message: msg.unwrap_or_else(|| prefix(body, 300)) }
    }

    pub fn send(&self, method: &str, path: &str, query: &[(&str, String)], body: Option<Vec<u8>>, accept: &str) -> Result<Vec<u8>> {
        let r = self.raw(method, path, query, body, accept, None)?;
        if !(200..300).contains(&r.status) { return Err(self.fail(r.status, &r.body)); }
        Ok(r.body)
    }

    fn send_json(&self, method: &str, path: &str, body: &impl Serialize) -> Result<Vec<u8>> {
        self.send(method, path, &[], Some(serde_json::to_vec(body).unwrap()), "application/json")
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let b = self.send("GET", path, query, None, "application/json")?;
        serde_json::from_slice(&b).map_err(|e| Error::msg(format!("{} sent something unexpected: {e}", self.server_name())))
    }

    /// Follows page=N until a short page comes back.
    pub fn get_all<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<Vec<T>> {
        let mut all = Vec::new();
        for page in 1.. {
            let mut q = query.to_vec();
            q.extend(self.paged(page, 50));
            let batch: Vec<T> = self.get(path, &q)?;
            let n = batch.len();
            all.extend(batch);
            if n < 50 { break; }
        }
        Ok(all)
    }

    fn repo(r: &RemoteRepo) -> String {
        format!("/repos/{}/{}", r.owner, r.name)
    }

    pub fn me(&self) -> Result<User> {
        self.get("/user", &[])
    }

    /// One page, server default order (= the web UI's, newest first). Closed lists run to thousands; never fetch
    /// them all.
    pub fn pulls(&self, r: &RemoteRepo, state: &str, page: usize) -> Result<Vec<PullRequest>> {
        let mut q = self.paged(page, PULL_PAGE);
        q.push(("state", state.into()));
        let list: Vec<PullRequest> = self.get(&format!("{}/pulls", Self::repo(r)), &q)?;
        Ok(list.into_iter().map(PullRequest::normalized).collect())
    }

    pub fn pull(&self, r: &RemoteRepo, n: i64) -> Result<PullRequest> {
        Ok(self.get::<PullRequest>(&format!("{}/pulls/{n}", Self::repo(r)), &[])?.normalized())
    }

    /// The PR's unified diff. [`Error::DiffTooLarge`] when GitHub refuses (over 300 files or 20k lines): the caller
    /// diffs locally instead.
    pub fn diff(&self, r: &RemoteRepo, n: i64) -> Result<Vec<u8>> {
        if !self.github { return self.send("GET", &format!("{}/pulls/{n}.diff", Self::repo(r)), &[], None, "text/plain"); }
        match self.send("GET", &format!("{}/pulls/{n}", Self::repo(r)), &[], None, "application/vnd.github.diff") {
            Err(Error::Http { status: 406 | 422, .. }) => Err(Error::DiffTooLarge),
            x => x,
        }
    }

    /// GitHub states → Forgejo's: CHANGES_REQUESTED → REQUEST_CHANGES, COMMENTED → COMMENT, DISMISSED → dismissed.
    /// The viewer's own unsubmitted (PENDING) review is left out.
    pub fn reviews(&self, r: &RemoteRepo, n: i64) -> Result<Vec<Review>> {
        let list: Vec<Review> = self.get_all(&format!("{}/pulls/{n}/reviews", Self::repo(r)), &[])?;
        if !self.github { return Ok(list); }
        Ok(list
            .into_iter()
            .filter(|v| v.state != "PENDING")
            .map(|mut v| {
                match v.state.as_str() {
                    "CHANGES_REQUESTED" => v.state = "REQUEST_CHANGES".into(),
                    "COMMENTED" => v.state = "COMMENT".into(),
                    "DISMISSED" => v.dismissed = Some(true),
                    _ => {}
                }
                v
            })
            .collect())
    }

    /// Inline comments of all reviews. Forgejo lists them per review; GitHub lists the whole PR at once and gives
    /// line + side instead of Forgejo's position (new-file line) / original_position (old-file line).
    pub fn review_comments(&self, r: &RemoteRepo, n: i64, reviews: &[Review]) -> Result<Vec<ReviewComment>> {
        if self.github {
            #[derive(Deserialize)]
            struct C {
                id: i64,
                user: Option<User>,
                path: String,
                body: String,
                line: Option<i64>,
                side: Option<String>,
                created_at: Option<String>,
                commit_id: Option<String>,
                pull_request_review_id: Option<i64>,
                diff_hunk: Option<String>,
            }
            let list: Vec<C> = self.get_all(&format!("{}/pulls/{n}/comments", Self::repo(r)), &[])?;
            return Ok(list
                .into_iter()
                .map(|c| {
                    let left = c.side.as_deref() == Some("LEFT");
                    let line = c.line.unwrap_or(0);
                    ReviewComment {
                        id: c.id,
                        user: c.user,
                        path: c.path,
                        body: c.body,
                        position: if left { 0 } else { line },
                        original_position: if left { line } else { 0 },
                        created_at: c.created_at,
                        commit_id: c.commit_id,
                        pull_request_review_id: c.pull_request_review_id,
                        diff_hunk: c.diff_hunk,
                    }
                })
                .collect());
        }
        // In parallel, eight at a time: a long-lived PR has dozens of reviews.
        let mut all = Vec::new();
        for chunk in reviews.chunks(8) {
            let results: Vec<Result<Vec<ReviewComment>>> = std::thread::scope(|s| {
                let hs: Vec<_> = chunk
                    .iter()
                    .map(|v| s.spawn(move || self.get(&format!("{}/pulls/{n}/reviews/{}/comments", Self::repo(r), v.id), &[])))
                    .collect();
                hs.into_iter().map(|h| h.join().unwrap()).collect()
            });
            for x in results { all.extend(x?); }
        }
        all.sort_by_key(|c| c.id);
        Ok(all)
    }

    /// Oldest first. Forgejo skips signature checks and file lists when asked; GitHub ignores the flags.
    pub fn pull_commits(&self, r: &RemoteRepo, n: i64) -> Result<Vec<PrCommit>> {
        self.get_all(&format!("{}/pulls/{n}/commits", Self::repo(r)), &[("verification", "false".into()), ("files", "false".into())])
    }

    /// Forgejo returns them all and ignores paging; GitHub pages at 30.
    pub fn issue_comments(&self, r: &RemoteRepo, n: i64) -> Result<Vec<IssueComment>> {
        let p = format!("{}/issues/{n}/comments", Self::repo(r));
        if self.github { self.get_all(&p, &[]) } else { self.get(&p, &[]) }
    }

    pub fn add_issue_comment(&self, r: &RemoteRepo, n: i64, body: &str) -> Result<()> {
        self.send_json("POST", &format!("{}/issues/{n}/comments", Self::repo(r)), &json!({ "body": body })).map(|_| ())
    }

    pub fn submit_review(&self, r: &RemoteRepo, n: i64, event: ReviewEvent, body: &str, commit: &str, comments: &[DraftComment]) -> Result<()> {
        let path = format!("{}/pulls/{n}/reviews", Self::repo(r));
        let b = review_body(self.github, event, body, commit, comments);
        self.send_json("POST", &path, &b).map(|_| ())
    }

    // MARK: CI (commit statuses + Actions)

    pub fn combined_status(&self, r: &RemoteRepo, sha: &str) -> Result<Vec<CommitStatus>> {
        if self.github { return self.github_checks(r, sha); }
        #[derive(Deserialize)]
        struct Combined {
            total_count: Option<usize>,
            statuses: Option<Vec<CommitStatus>>,
        }
        let mut all = Vec::new();
        for page in 1.. {
            let c: Combined = self.get(&format!("{}/commits/{sha}/status", Self::repo(r)), &self.paged(page, 50))?;
            let n = c.statuses.as_ref().map_or(0, |s| s.len());
            all.extend(c.statuses.unwrap_or_default());
            if n < 50 || all.len() >= c.total_count.unwrap_or(0) { break; }
        }
        Ok(all)
    }

    /// GitHub reports Actions (and most apps) as check runs, older integrations as commit statuses; both become
    /// CommitStatus rows. An Actions check run's id is its job id, which the checks model uses to show the job.
    fn github_checks(&self, r: &RemoteRepo, sha: &str) -> Result<Vec<CommitStatus>> {
        #[derive(Deserialize)]
        struct S {
            id: i64,
            state: String,
            context: String,
            description: Option<String>,
            target_url: Option<String>,
            created_at: Option<String>,
            updated_at: Option<String>,
        }
        #[derive(Deserialize)]
        struct Combined {
            statuses: Vec<S>,
        }
        #[derive(Deserialize)]
        struct Output {
            title: Option<String>,
            summary: Option<String>,
            annotations_count: Option<i64>,
        }
        #[derive(Deserialize)]
        struct App {
            slug: Option<String>,
        }
        #[derive(Deserialize)]
        struct Run {
            id: i64,
            name: String,
            status: String,
            conclusion: Option<String>,
            html_url: Option<String>,
            details_url: Option<String>,
            started_at: Option<String>,
            completed_at: Option<String>,
            output: Option<Output>,
            app: Option<App>,
        }
        #[derive(Deserialize)]
        struct Runs {
            check_runs: Vec<Run>,
        }
        let base = Self::repo(r);
        let q = [("per_page", "100".to_string())];
        let (combined, runs) = std::thread::scope(|s| {
            let a = s.spawn(|| self.get::<Combined>(&format!("{base}/commits/{sha}/status"), &q));
            let b = self.get::<Runs>(&format!("{base}/commits/{sha}/check-runs"), &q);
            (a.join().unwrap(), b)
        });
        let (combined, runs) = (combined?, runs?);
        let mut out: Vec<CommitStatus> = combined
            .statuses
            .into_iter()
            .map(|s| CommitStatus {
                id: s.id,
                status: s.state,
                context: s.context,
                description: s.description,
                target_url: s.target_url,
                created_at: s.created_at,
                updated_at: s.updated_at,
                check_run: None,
            })
            .collect();
        out.extend(runs.check_runs.into_iter().map(|c| {
            // conclusion: success | failure | neutral | cancelled | skipped | timed_out | action_required | stale
            let s = if c.status == "completed" { c.conclusion.clone().unwrap_or_default() } else { "pending".into() };
            let status = match s.as_str() {
                "success" | "neutral" => "success",
                "skipped" | "stale" => "skipped",
                "pending" => "pending",
                _ => "failure",
            };
            let (title, summary, annotations) = c.output.map_or((None, None, None), |o| (o.title, o.summary, o.annotations_count));
            CommitStatus {
                id: c.id,
                status: status.into(),
                context: c.name,
                description: title,
                target_url: c.details_url.or(c.html_url),
                created_at: c.started_at,
                updated_at: c.completed_at,
                check_run: Some(CheckRunRef { id: c.id, app: c.app.and_then(|a| a.slug), summary, annotations }),
            }
        }));
        Ok(out)
    }

    /// Forgejo's shapes; GitHub and Gitea go through [`Client::ci_runs`].
    pub fn action_runs(&self, r: &RemoteRepo, head_sha: &str) -> Result<Vec<ActionRun>> {
        if self.github { return Ok(vec![]); }
        #[derive(Deserialize)]
        struct Page {
            workflow_runs: Option<Vec<ActionRun>>,
        }
        let p: Page = self.get(&format!("{}/actions/runs", Self::repo(r)), &[("head_sha", head_sha.into()), ("limit", "100".into())])?;
        Ok(p.workflow_runs.unwrap_or_default())
    }

    pub fn action_jobs(&self, r: &RemoteRepo, run_id: i64) -> Result<Vec<ActionJob>> {
        self.get(&format!("{}/actions/runs/{run_id}/jobs", Self::repo(r)), &[])
    }

    pub fn action_job(&self, r: &RemoteRepo, id: i64) -> Result<ActionJob> {
        self.get(&format!("{}/actions/jobs/{id}", Self::repo(r)), &[])
    }

    /// Log bytes from `range` ("bytes=N-" to tail, "bytes=-N" for the last N bytes). The server honours Range, so
    /// polling a running job only transfers what's new. Returns (data, first byte, total size); empty data when
    /// nothing is new (416: offset == size).
    pub fn action_log(&self, r: &RemoteRepo, job_id: i64, range: Option<&str>) -> Result<(Vec<u8>, usize, Option<usize>)> {
        let accept = if self.github { "application/json" } else { "text/plain" };
        let resp = self.raw("GET", &format!("{}/actions/jobs/{job_id}/logs", Self::repo(r)), &[], None, accept, range)?;
        if resp.status == 416 { return Ok((vec![], 0, None)); }
        if !(200..300).contains(&resp.status) {
            return Err(Error::Http { server: "Log".into(), status: resp.status, message: prefix(&resp.body, 200) });
        }
        // "Content-Range: bytes 1824000-1887148/1887149"
        if let Some(sp) = resp.content_range.as_deref().and_then(|cr| cr.split(' ').next_back()) {
            let mut parts = sp.split('/');
            let start = parts.next().and_then(|p| p.split('-').next()).and_then(|s| s.parse().ok()).unwrap_or(0);
            let total = parts.next().and_then(|t| t.parse().ok());
            return Ok((resp.body, start, total));
        }
        let n = resp.body.len();
        Ok((resp.body, 0, Some(n)))
    }

    /// The forge user behind a commit's author email (None when the email maps to no account).
    pub fn commit_author(&self, r: &RemoteRepo, sha: &str) -> Result<Option<User>> {
        #[derive(Deserialize)]
        struct C {
            author: Option<User>,
        }
        let q = [
            ("sha", sha.to_string()),
            (self.size_key(), "1".into()),
            ("stat", "false".into()),
            ("verification", "false".into()),
            ("files", "false".into()),
        ];
        let list: Vec<C> = self.get(&format!("{}/commits", Self::repo(r)), &q)?;
        Ok(list.into_iter().next().and_then(|c| c.author))
    }

    pub fn merge(&self, r: &RemoteRepo, n: i64, style: MergeStyle) -> Result<()> {
        let p = format!("{}/pulls/{n}/merge", Self::repo(r));
        if self.github { self.send_json("PUT", &p, &json!({ "merge_method": style.as_str() })) } else { self.send_json("POST", &p, &json!({ "Do": style.as_str() })) }
            .map(|_| ())
    }

    // MARK: CI across forges (CIForges.swift)

    pub fn server_version(&self) -> Option<String> {
        #[derive(Deserialize)]
        struct V {
            version: String,
        }
        self.get::<V>("/version", &[]).ok().map(|v| v.version)
    }

    fn page_size(&self) -> Query<'static> {
        vec![(self.size_key(), "100".into())]
    }

    pub fn ci_runs(&self, r: &RemoteRepo, head_sha: &str, f: &CiFeatures) -> Result<Vec<ActionRun>> {
        if !f.github_shaped() { return self.action_runs(r, head_sha); }
        #[derive(Deserialize)]
        struct Page {
            workflow_runs: Option<Vec<GhRun>>,
        }
        let mut q = self.page_size();
        q.push(("head_sha", head_sha.into()));
        let p: Page = self.get(&format!("{}/actions/runs", Self::repo(r)), &q)?;
        Ok(p.workflow_runs.unwrap_or_default().into_iter().map(GhRun::forgejo).collect())
    }

    pub fn ci_jobs(&self, r: &RemoteRepo, run_id: i64, f: &CiFeatures) -> Result<Vec<ActionJob>> {
        if !f.github_shaped() { return self.action_jobs(r, run_id); }
        #[derive(Deserialize)]
        struct Page {
            jobs: Option<Vec<GhJob>>,
        }
        let p: Page = self.get(&format!("{}/actions/runs/{run_id}/jobs", Self::repo(r)), &self.page_size())?;
        Ok(p.jobs.unwrap_or_default().into_iter().map(GhJob::forgejo).collect())
    }

    pub fn ci_job(&self, r: &RemoteRepo, id: i64, f: &CiFeatures) -> Result<ActionJob> {
        if !f.github_shaped() { return self.action_job(r, id); }
        Ok(self.get::<GhJob>(&format!("{}/actions/jobs/{id}", Self::repo(r)), &[])?.forgejo())
    }

    pub fn ci_perform(&self, a: CiAction, r: &RemoteRepo, job: &ActionJob, f: &CiFeatures) -> Result<()> {
        self.send("POST", &f.path(a, r, job.run_id, job.id), &[], None, "application/json").map(|_| ())
    }

    pub fn check_run_annotations(&self, r: &RemoteRepo, id: i64) -> Result<Vec<CheckAnnotation>> {
        self.get(&format!("{}/check-runs/{id}/annotations", Self::repo(r)), &[("per_page", "100".into())])
    }

    // MARK: Pull request create (PullCreate.swift)

    pub fn create_pull(&self, r: &RemoteRepo, p: &NewPull) -> Result<PullRequest> {
        let resp = self.raw("POST", &format!("{}/pulls", Self::repo(r)), &[], Some(serde_json::to_vec(p).unwrap()), "application/json", None)?;
        if !(200..300).contains(&resp.status) { return Err(create_error(resp.status, &resp.body, self.server_name())); }
        serde_json::from_slice(&resp.body).map_err(|e| Error::msg(e.to_string()))
    }

    /// The open pull request whose head is `branch` in this repository, if any.
    pub fn open_pull(&self, r: &RemoteRepo, branch: &str) -> Result<Option<PullRequest>> {
        let p = format!("{}/pulls", Self::repo(r));
        if self.github {
            let list: Vec<PullRequest> = self.get(&p, &[("state", "open".into()), ("head", format!("{}:{branch}", r.owner))])?;
            return Ok(list.into_iter().next());
        }
        // Forgejo's list has no head filter; the open list is the short one. A fork's PR can share the branch name
        // (patch-1), so only a head in this same repository counts.
        let list: Vec<HeadOnly> = self.get_all(&p, &[("state", "open".into())])?;
        match same_repo_pull(&list, branch) {
            Some(n) => self.pull(r, n).map(Some),
            None => Ok(None),
        }
    }

    pub fn default_branch(&self, r: &RemoteRepo) -> Result<String> {
        #[derive(Deserialize)]
        struct R {
            default_branch: String,
        }
        Ok(self.get::<R>(&Self::repo(r), &[])?.default_branch)
    }
}

pub(crate) fn prefix(b: &[u8], n: usize) -> String {
    String::from_utf8_lossy(&b[..b.len().min(n)]).into_owned()
}

/// The review body each server takes. GitHub: comments by line + side, APPROVE. Forgejo: new_position /
/// old_position (new wins when both are set), APPROVED.
pub fn review_body(github: bool, event: ReviewEvent, body: &str, commit: &str, comments: &[DraftComment]) -> serde_json::Value {
    if github {
        let cs: Vec<_> = comments
            .iter()
            .filter_map(|d| {
                let (line, side) = d.new_line.map(|l| (l, "RIGHT")).or(d.old_line.map(|l| (l, "LEFT")))?;
                Some(json!({ "path": d.path, "body": d.body, "line": line, "side": side }))
            })
            .collect();
        let ev = if event == ReviewEvent::Approve { "APPROVE" } else { event.as_str() };
        return json!({ "body": body, "event": ev, "commit_id": commit, "comments": cs });
    }
    let cs: Vec<_> = comments
        .iter()
        .map(|d| json!({ "path": d.path, "body": d.body, "new_position": d.new_line, "old_position": if d.new_line.is_none() { d.old_line } else { None } }))
        .collect();
    json!({ "body": body, "event": event.as_str(), "commit_id": commit, "comments": cs })
}

/// The body of POST /repos/{o}/{r}/pulls. GitHub takes `draft`; Forgejo and Gitea mark a draft by its title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewPull {
    pub head: String,
    pub base: String,
    pub title: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<bool>,
}

pub fn new_pull(head: &str, base: &str, title: &str, body: &str, draft: bool, github: bool) -> NewPull {
    let t = title.trim();
    if github {
        return NewPull { head: head.into(), base: base.into(), title: t.into(), body: body.into(), draft: draft.then_some(true) };
    }
    let lower = t.to_lowercase();
    let marked = ["wip:", "[wip]"].iter().any(|p| lower.starts_with(p));
    let title = if draft && !marked { format!("WIP: {t}") } else { t.to_string() };
    NewPull { head: head.into(), base: base.into(), title, body: body.into(), draft: None }
}

/// Forgejo answers 409, GitHub 422 with the reason in `errors[].message` (its `message` is just "Validation Failed").
pub fn create_error(status: u16, data: &[u8], server: &str) -> Error {
    let json: Option<serde_json::Value> = serde_json::from_slice(data).ok();
    let reasons: Vec<String> = json
        .as_ref()
        .and_then(|j| j.get("errors")?.as_array().cloned())
        .map(|a| a.iter().filter_map(|e| e.get("message")?.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let text = if reasons.is_empty() {
        json.as_ref().and_then(|j| j.get("message")?.as_str().map(str::to_string)).unwrap_or_else(|| prefix(data, 300))
    } else {
        reasons.join(" ")
    };
    if (status == 409 || status == 422) && text.to_lowercase().contains("already exists") { return Error::PullExists; }
    Error::Http { server: server.into(), status, message: text }
}

#[derive(Debug, Clone, Deserialize)]
pub struct HeadOnly {
    pub number: i64,
    pub head: HeadRef,
    pub base: HeadRef,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HeadRef {
    #[serde(rename = "ref")]
    pub ref_: String,
    pub repo_id: Option<i64>,
}

pub fn same_repo_pull(list: &[HeadOnly], branch: &str) -> Option<i64> {
    list.iter().find(|p| p.head.ref_ == branch && p.head.repo_id == p.base.repo_id).map(|p| p.number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_pull_body_marks_drafts_per_server() {
        let gh = new_pull("feat/x", "main", " Add x ", "b", true, true);
        assert_eq!(gh, NewPull { head: "feat/x".into(), base: "main".into(), title: "Add x".into(), body: "b".into(), draft: Some(true) });
        let gh_json = serde_json::to_string(&new_pull("h", "m", "t", "", false, true)).unwrap();
        assert!(!gh_json.contains("draft")); // not a draft: the field is left out

        let fj = new_pull("feat/x", "main", "Add x", "b", true, false);
        assert!(fj.title == "WIP: Add x" && fj.draft.is_none());
        assert_eq!(new_pull("h", "m", "[WIP] Add x", "", true, false).title, "[WIP] Add x");
        assert_eq!(new_pull("h", "m", "Add x", "", false, false).title, "Add x");
    }

    #[test]
    fn create_errors_tell_an_existing_pull_apart() {
        let gh = br#"{"message":"Validation Failed","errors":[{"resource":"PullRequest","code":"custom","message":"A pull request already exists for acme:feat."}]}"#;
        assert_eq!(create_error(422, gh, "GitHub"), Error::PullExists);
        let fj = br#"{"message":"pull request already exists for these targets [id: 3, issue_id: 7, head_repo_id: 1, base_repo_id: 1, head_branch: feat, base_branch: main]"}"#;
        assert_eq!(create_error(409, fj, "Forgejo"), Error::PullExists);
        let none = br#"{"message":"Validation Failed","errors":[{"resource":"PullRequest","code":"custom","message":"No commits between main and feat"}]}"#;
        let e = create_error(422, none, "GitHub");
        assert_ne!(e, Error::PullExists);
        assert_eq!(e.to_string(), "GitHub 422: No commits between main and feat");
    }

    #[test]
    fn open_pull_on_forgejo_ignores_forks_with_the_same_branch_name() {
        let json = r#"[{"number":4,"head":{"ref":"patch-1","repo_id":9},"base":{"ref":"main","repo_id":8}},{"number":3,"head":{"ref":"patch-1","repo_id":8},"base":{"ref":"main","repo_id":8}}]"#;
        let list: Vec<HeadOnly> = serde_json::from_str(json).unwrap();
        assert_eq!(same_repo_pull(&list, "patch-1"), Some(3));
        assert_eq!(same_repo_pull(&list[..1], "patch-1"), None);
        assert_eq!(same_repo_pull(&list, "other"), None);
    }

    #[test]
    fn api_roots_and_review_bodies() {
        assert_eq!(Client::new("https://github.com/", "t", true).api_root(), "https://api.github.com");
        assert_eq!(Client::new("https://ghe.example.com", "t", true).api_root(), "https://ghe.example.com/api/v3");
        assert_eq!(Client::new("https://git.example.com", "t", false).api_root(), "https://git.example.com/api/v1");
        let d = [DraftComment::new("a.rs", Some(3), None, "new"), DraftComment::new("b.rs", None, Some(7), "old")];
        let gh = review_body(true, ReviewEvent::Approve, "lgtm", "abc", &d);
        assert_eq!(gh["event"], "APPROVE");
        assert_eq!(gh["comments"][1]["side"], "LEFT");
        assert_eq!(gh["comments"][1]["line"], 7);
        let fj = review_body(false, ReviewEvent::Approve, "lgtm", "abc", &d);
        assert_eq!(fj["event"], "APPROVED");
        assert_eq!(fj["comments"][0]["new_position"], 3);
        assert!(fj["comments"][0]["old_position"].is_null());
        assert_eq!(fj["comments"][1]["old_position"], 7);
    }

    #[test]
    fn github_maps_merged_at() {
        let p: PullRequest = serde_json::from_str(r#"{"id":1,"number":2,"title":"t","state":"closed","user":{"login":"a"},"head":{"ref":"h","sha":"1"},"base":{"ref":"m","sha":"0"},"html_url":"","updated_at":"","merged_at":"2026-01-01T00:00:00Z"}"#).unwrap();
        assert_eq!(p.normalized().merged, Some(true));
    }

    #[test]
    fn a_cancelled_client_never_reaches_the_network() {
        let c = Cancel::new();
        c.cancel();
        let client = Client::new("http://127.0.0.1:1", "x", false).with_cancel(&c);
        let e = client.me().unwrap_err();
        assert_eq!(e, Error::Cancelled);
        assert_eq!(e.user_message(), None);
    }
}

#[cfg(test)]
mod live {
    //! Read-only against api.github.com with the `gh` CLI's token: cargo test -p loupe-forge -- --ignored github
    use super::*;
    use crate::account::gh_token;

    fn client() -> Client {
        Client::new("https://github.com", &gh_token("github.com").expect("gh auth login"), true)
    }

    #[test]
    #[ignore]
    fn github_pulls_reviews_and_comments() {
        let c = client();
        let vscode = RemoteRepo::parse("https://github.com/microsoft/vscode").unwrap();
        assert!(!c.me().unwrap().login.is_empty());
        assert_eq!(c.pulls(&vscode, "closed", 1).unwrap().len(), PULL_PAGE);
        let pr = c.pull(&vscode, 280236).unwrap();
        assert_eq!(pr.number, 280236);
        let reviews = c.reviews(&vscode, 280236).unwrap();
        assert!(reviews.iter().any(|r| r.state == "REQUEST_CHANGES"));
        assert!(reviews.iter().any(|r| r.dismissed == Some(true)));
        assert!(!reviews.iter().any(|r| ["CHANGES_REQUESTED", "COMMENTED", "PENDING"].contains(&r.state.as_str())));
        let comments = c.review_comments(&vscode, 280236, &reviews).unwrap();
        assert!(comments.len() > 30); // more than one page
        assert!(comments.iter().any(|c| c.position > 0 || c.original_position > 0));
        c.issue_comments(&vscode, 280236).unwrap();
        c.combined_status(&vscode, &pr.head.sha).unwrap();
        let diff = c.diff(&vscode, 280236).unwrap();
        assert!(diff.starts_with(b"diff --git"));
        // The same GET again revalidates (304) and still returns the body.
        assert_eq!(c.diff(&vscode, 280236).unwrap(), diff);
    }

    #[test]
    #[ignore]
    fn github_too_large_diff_is_reported() {
        let react = RemoteRepo::parse("https://github.com/facebook/react").unwrap();
        assert_eq!(client().diff(&react, 36701).unwrap_err(), Error::DiffTooLarge);
    }

    /// Creates (then closes) a pull request on a local Forgejo, never a real server.
    /// LOUPE_FORGEJO_LIVE_TOKEN=<token> [LOUPE_FORGEJO_LIVE_URL=http://localhost:3300] [LOUPE_FORGEJO_LIVE_REPO=alex/dotfiles]
    #[test]
    #[ignore]
    fn forgejo_creates_finds_and_refuses_a_duplicate() {
        let token = std::env::var("LOUPE_FORGEJO_LIVE_TOKEN").expect("LOUPE_FORGEJO_LIVE_TOKEN");
        let url = std::env::var("LOUPE_FORGEJO_LIVE_URL").unwrap_or("http://localhost:3300".into());
        let repo = std::env::var("LOUPE_FORGEJO_LIVE_REPO").unwrap_or("alex/dotfiles".into());
        let c = Client::new(&url, &token, false);
        let r = RemoteRepo::parse(&format!("http://localhost/{repo}")).unwrap();
        let branch = format!("loupe-test/pr-{}", &uuid::Uuid::new_v4().to_string()[..6]);
        let base = c.default_branch(&r).unwrap();
        let rp = Client::repo(&r);
        c.send_json("POST", &format!("{rp}/branches"), &json!({ "new_branch_name": branch, "old_branch_name": base })).unwrap();
        c.send_json(
            "POST",
            &format!("{rp}/contents/{}.txt", branch.replace('/', "-")),
            &json!({ "branch": branch, "content": "aGkK", "message": "Add a test file" }),
        )
        .unwrap();
        let check = || -> Result<()> {
            assert!(c.open_pull(&r, &branch)?.is_none());
            let pr = c.create_pull(&r, &new_pull(&branch, &base, "Add a test file", "From Loupe's tests.", true, false))?;
            assert!(pr.title == "WIP: Add a test file" && pr.head.ref_ == branch && pr.base.ref_ == base && pr.state == "open");
            assert_eq!(c.open_pull(&r, &branch)?.map(|p| p.number), Some(pr.number));
            assert_eq!(c.create_pull(&r, &new_pull(&branch, &base, "Again", "", false, false)).unwrap_err(), Error::PullExists);
            c.send_json("PATCH", &format!("{rp}/pulls/{}", pr.number), &json!({ "state": "closed" }))?;
            Ok(())
        };
        let res = check();
        let _ = c.send("DELETE", &format!("{rp}/branches/{}", encode(&branch)), &[], None, "application/json");
        res.unwrap();
    }
}

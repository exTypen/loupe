//! CI across forges: commit statuses, Actions runs/jobs/steps, job logs with Range tailing.
//!
//! Forgejo's Actions API has its own shapes (ActionRun / ActionJob); GitHub's and Gitea's (1.25+) are the same as
//! each other, so they're decoded once here and mapped onto Forgejo's.
//!
//! No socket exists for token clients, so the app polls: [`Checks::poll_interval`] (5 s while anything moves,
//! 30 s once settled) and [`LOG_TAIL_INTERVAL`] (2 s) for a running job's log.

use crate::models::*;
use crate::{Cancel, Client, Error, RemoteRepo, Result};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const LOG_TAIL_INTERVAL: Duration = Duration::from_secs(2);
/// Between attempts while the server has no log for a job yet.
pub const LOG_WAIT_INTERVAL: Duration = Duration::from_secs(5);

// MARK: - Features

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Forge {
    GitHub,
    Forgejo,
    Gitea,
}

/// What a server's Actions API can do, decided once per server from GET /version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiFeatures {
    pub forge: Forge,
    pub version: Option<String>,
    /// runs, jobs and logs
    pub actions: bool,
    pub rerun_run: bool,
    pub rerun_failed: bool,
    pub rerun_job: bool,
    pub cancel: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CiAction {
    RerunJob,
    RerunFailed,
    RerunAll,
    Cancel,
}

impl CiAction {
    pub fn title(self) -> &'static str {
        match self {
            CiAction::RerunJob => "Re-run Job",
            CiAction::RerunFailed => "Re-run Failed Jobs",
            CiAction::RerunAll => "Re-run All Jobs",
            CiAction::Cancel => "Cancel Run",
        }
    }
    pub fn help(self) -> &'static str {
        if self == CiAction::RerunJob { "Run this job again (on Forgejo, with the jobs that depend on it)" } else { self.title() }
    }
}

impl CiFeatures {
    fn new(forge: Forge, version: Option<String>) -> Self {
        Self { forge, version, actions: true, rerun_run: false, rerun_failed: false, rerun_job: false, cancel: false }
    }

    /// GitHub and Gitea answer with GitHub's run/job shapes; Forgejo with its own.
    pub fn github_shaped(&self) -> bool {
        self.forge != Forge::Forgejo
    }

    pub fn name(&self) -> &'static str {
        match self.forge {
            Forge::GitHub => "GitHub",
            Forge::Forgejo => "Forgejo",
            Forge::Gitea => "Gitea",
        }
    }

    /// Forgejo reports "17.0.0-100-2254cb0+gitea-1.22.0"; Gitea "1.26.0" (or "1.27.0+dev-…").
    /// Endpoints as each release's swagger lists them:
    /// Forgejo: cancel 16+, rerun run/job 17+, no rerun-failed. Gitea: Actions API 1.25+, rerun (all/failed/job)
    /// 1.26+, cancel after 1.26. An unreadable version keeps today's behaviour: try Actions, offer nothing that writes.
    pub fn detect(github: bool, version: Option<&str>) -> Self {
        if github {
            return Self { rerun_run: true, rerun_failed: true, rerun_job: true, cancel: true, ..Self::new(Forge::GitHub, None) };
        }
        let Some((version, (major, minor))) = version.and_then(|v| numbers(v).map(|n| (v, n))) else {
            return Self::new(Forge::Forgejo, version.map(str::to_string));
        };
        // Gitea has always been 1.x; Forgejo started at 7 and newer releases drop the "+gitea-" suffix, so a plain
        // "17.0.0" is Forgejo too (taking it for Gitea decoded Forgejo's runs as GitHub's and failed every refresh).
        if major >= 2 || version.contains("+gitea-") || version.to_lowercase().contains("forgejo") {
            return Self { rerun_run: major >= 17, rerun_job: major >= 17, cancel: major >= 16, ..Self::new(Forge::Forgejo, Some(version.into())) };
        }
        let at = |ma: i64, mi: i64| major > ma || major == ma && minor >= mi;
        Self {
            actions: at(1, 25),
            rerun_run: at(1, 26),
            rerun_failed: at(1, 26),
            rerun_job: at(1, 26),
            cancel: at(1, 27),
            ..Self::new(Forge::Gitea, Some(version.into()))
        }
    }

    /// The buttons a job's detail header offers. Any rerun, a job's too, needs the whole run to have finished
    /// (GitHub, Forgejo and Gitea all refuse one while the run is moving); cancel needs it moving.
    pub fn actions(&self, run_status: Option<&str>, job_status: &str) -> Vec<CiAction> {
        if !self.actions { return vec![]; }
        let job = CheckState::from_job(job_status);
        let run = run_status.map_or(job, CheckState::from_job);
        let mut out = vec![];
        if self.rerun_job && job.is_done() && run.is_done() { out.push(CiAction::RerunJob); }
        // GitHub reruns cancelled jobs too
        if self.rerun_failed && (run == CheckState::Failing || run_status.unwrap_or(job_status) == "cancelled") { out.push(CiAction::RerunFailed); }
        if self.rerun_run && run.is_done() { out.push(CiAction::RerunAll); }
        if self.cancel && !run.is_done() { out.push(CiAction::Cancel); }
        out
    }

    pub fn path(&self, a: CiAction, r: &RemoteRepo, run_id: i64, job_id: i64) -> String {
        let base = format!("/repos/{}/{}/actions", r.owner, r.name);
        match a {
            CiAction::RerunJob if self.forge == Forge::Gitea => format!("{base}/runs/{run_id}/jobs/{job_id}/rerun"),
            CiAction::RerunJob => format!("{base}/jobs/{job_id}/rerun"),
            CiAction::RerunFailed => format!("{base}/runs/{run_id}/rerun-failed-jobs"),
            CiAction::RerunAll => format!("{base}/runs/{run_id}/rerun"),
            CiAction::Cancel => format!("{base}/runs/{run_id}/cancel"),
        }
    }

    /// A refused write explained in terms of what to change, rather than the bare status line.
    pub fn explain(&self, e: &Error) -> String {
        let m = e.to_string();
        // GitHub answers 404 rather than 403 when a token can't see Actions at all.
        if m.starts_with("GitHub 403") || m.starts_with("GitHub 404") {
            return format!(
                "GitHub refused this ({}): the token needs write access to Actions — “Actions: Read and write” for a fine-grained token, the repo and workflow scopes for a classic one.",
                &m[7..10]
            );
        }
        if m.contains(" 403:") { return format!("{} refused this (403): your account needs write access to this repository.", self.name()); }
        m
    }
}

fn numbers(s: &str) -> Option<(i64, i64)> {
    let parts: Vec<i64> = s.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).take(2).filter_map(|p| p.parse().ok()).collect();
    Some((*parts.first()?, parts.get(1).copied().unwrap_or(0)))
}

// MARK: - GitHub-shaped runs and jobs (GitHub, Gitea 1.25+)

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GhRun {
    pub id: i64,
    pub run_number: Option<i64>,
    pub run_attempt: Option<i64>,
    pub name: Option<String>,
    pub display_title: Option<String>,
    pub path: Option<String>,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub event: Option<String>,
    pub html_url: Option<String>,
}

impl GhRun {
    pub fn forgejo(self) -> ActionRun {
        ActionRun {
            id: self.id,
            index_in_repo: self.run_number.unwrap_or(self.id),
            title: self.display_title.or(self.name),
            workflow_id: self.path,
            status: Some(gh_status(self.status.as_deref(), self.conclusion.as_deref())),
            event: self.event,
            html_url: self.html_url,
        }
    }
}

/// GitHub's status + conclusion → Forgejo's single status (success | failure | cancelled | skipped | waiting | running).
pub fn gh_status(status: Option<&str>, conclusion: Option<&str>) -> String {
    match status.unwrap_or("") {
        "completed" => match conclusion.unwrap_or("") {
            "success" | "neutral" => "success",
            "skipped" | "stale" => "skipped",
            "cancelled" => "cancelled",
            _ => "failure", // failure, timed_out, action_required, startup_failure
        },
        "in_progress" => "running",
        s @ ("success" | "failure" | "cancelled" | "skipped" | "running" | "waiting" | "blocked") => s, // already Forgejo-style
        _ => "waiting", // queued, waiting, requested, pending
    }
    .to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhStep {
    pub number: i64,
    pub name: String,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhJob {
    pub id: i64,
    pub run_id: i64,
    pub run_attempt: Option<i64>,
    pub name: String,
    pub html_url: Option<String>,
    pub labels: Option<Vec<String>>,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub steps: Option<Vec<GhStep>>,
}

impl GhJob {
    pub fn forgejo(self) -> ActionJob {
        ActionJob {
            id: self.id,
            run_id: self.run_id,
            attempt: self.run_attempt,
            name: self.name,
            html_url: self.html_url,
            needs: None,
            runs_on: self.labels,
            task_id: None,
            status: gh_status(self.status.as_deref(), self.conclusion.as_deref()),
            steps: self.steps.map(|ss| {
                ss.into_iter()
                    .map(|s| ActionStep {
                        number: s.number,
                        name: s.name,
                        status: gh_status(s.status.as_deref(), s.conclusion.as_deref()),
                        started: s.started_at,
                        stopped: s.completed_at,
                    })
                    .collect()
            }),
        }
    }
}

// MARK: - Check rows

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CheckState {
    Failing,
    Running,
    Pending,
    Success,
    Skipped,
}

impl CheckState {
    pub const ALL: [CheckState; 5] = [CheckState::Failing, CheckState::Running, CheckState::Pending, CheckState::Success, CheckState::Skipped];

    pub fn from_status(status: &str, description: Option<&str>) -> Self {
        match status {
            "failure" | "error" | "warning" => CheckState::Failing,
            "success" => CheckState::Success,
            "skipped" => CheckState::Skipped,
            _ => {
                let d = description.unwrap_or("").to_lowercase();
                if d.contains("running") || d.contains("started") { CheckState::Running } else { CheckState::Pending }
            }
        }
    }

    pub fn from_job(status: &str) -> Self {
        match status {
            "failure" => CheckState::Failing,
            "success" => CheckState::Success,
            "skipped" | "cancelled" => CheckState::Skipped,
            "running" => CheckState::Running,
            _ => CheckState::Pending,
        }
    }

    pub fn title(self) -> &'static str {
        ["Failing", "Running", "Queued", "Successful", "Skipped"][self as usize]
    }

    pub fn is_done(self) -> bool {
        matches!(self, CheckState::Failing | CheckState::Success | CheckState::Skipped)
    }
}

/// One row in the checks list: a commit status, linked to its Actions job when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub status: CommitStatus,
    pub job: Option<ActionJob>,
}

impl Check {
    pub fn id(&self) -> &str {
        &self.status.context
    }
    pub fn state(&self) -> CheckState {
        let s = CheckState::from_status(&self.status.status, self.status.description.as_deref());
        // A pending status says nothing of queued vs running (GitHub's in-progress check runs); the job does.
        if s == CheckState::Pending && self.job.as_ref().is_some_and(|j| CheckState::from_job(&j.status) == CheckState::Running) {
            return CheckState::Running;
        }
        s
    }
    /// The page to open: absolute target URLs as they are, relative ones (Forgejo) against the server.
    pub fn web_url(&self, base_url: &str) -> Option<String> {
        let t = self.status.target_url.as_ref()?;
        Some(if t.starts_with("http") { t.clone() } else { format!("{base_url}{t}") })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    pub failing: usize,
    pub running: usize,
    pub queued: usize,
    pub passed: usize,
    pub total: usize,
}

fn feature_cache() -> &'static Mutex<HashMap<String, CiFeatures>> {
    static C: OnceLock<Mutex<HashMap<String, CiFeatures>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The checks of one commit. Cheap to clone: clone it into a background task, [`Checks::refresh`] there, and put
/// the result back. A pull request refreshes its checks with every load (once a minute while open); the checks
/// overlay polls at [`Checks::poll_interval`].
#[derive(Debug, Clone, Default)]
pub struct Checks {
    pub sha: String,
    pub checks: Vec<Check>,
    pub error: Option<String>,
    /// Why jobs and logs aren't shown, when they can't be.
    pub notice: Option<String>,
    pub last_updated: Option<Instant>,
    pub features: Option<CiFeatures>,
    pub runs: Vec<ActionRun>,
    /// run id → jobs (latest attempt)
    pub jobs: HashMap<i64, Vec<ActionJob>>,
}

impl Checks {
    pub fn new(sha: &str) -> Self {
        Self { sha: sha.into(), ..Default::default() }
    }

    /// A new head: start over (the old commit's jobs don't apply).
    pub fn set_sha(&mut self, sha: &str) {
        if self.sha != sha { *self = Self { features: self.features.clone(), ..Self::new(sha) }; }
    }

    pub fn summary(&self) -> Summary {
        let n = |s| self.checks.iter().filter(|c| c.state() == s).count();
        Summary { failing: n(CheckState::Failing), running: n(CheckState::Running), queued: n(CheckState::Pending), passed: n(CheckState::Success), total: self.checks.len() }
    }

    pub fn all_done(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|c| c.state().is_done())
    }

    /// Fast while anything is moving, slow once everything has settled.
    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(if self.all_done() { 30 } else { 5 })
    }

    pub fn run(&self, id: i64) -> Option<&ActionRun> {
        self.runs.iter().find(|r| r.id == id)
    }

    /// The header line: "2 failing · 1 running", or "All checks passed".
    pub fn headline(&self) -> String {
        let s = self.summary();
        let parts: Vec<String> = [(s.failing, "failing"), (s.running, "running"), (s.queued, "queued")]
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, w)| format!("{n} {w}"))
            .collect();
        if parts.is_empty() { "All checks passed".into() } else { parts.join(" · ") }
    }

    pub fn features_for(&mut self, client: &Client) -> CiFeatures {
        if let Some(f) = &self.features { return f.clone(); }
        if let Some(f) = feature_cache().lock().unwrap().get(&client.base_url) {
            self.features = Some(f.clone());
            return f.clone();
        }
        let version = if client.github { None } else { client.server_version() };
        let f = CiFeatures::detect(client.github, version.as_deref());
        // A failed lookup is retried next refresh.
        if client.github || version.is_some() {
            feature_cache().lock().unwrap().insert(client.base_url.clone(), f.clone());
            self.features = Some(f.clone());
        }
        f
    }

    /// Blocking. A cancelled refresh changes nothing that's shown.
    pub fn refresh(&mut self, client: &Client, repo: &RemoteRepo) {
        if let Err(e) = self.try_refresh(client, repo) {
            self.error = e.user_message().or(self.error.take());
        }
    }

    fn try_refresh(&mut self, client: &Client, repo: &RemoteRepo) -> Result<()> {
        let f = self.features_for(client);
        let sha = self.sha.clone();
        let (statuses, runs) = std::thread::scope(|s| {
            let runs = s.spawn(|| if f.actions { client.ci_runs(repo, &sha, &f) } else { Ok(vec![]) });
            let statuses = client.combined_status(repo, &sha);
            (statuses, runs.join().unwrap())
        });
        let statuses = statuses?;
        // Actions may be disabled on the server (404): commit statuses still stand on their own.
        match runs {
            Ok(r) => {
                self.runs = r;
                self.notice = notice(&f, None);
            }
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => {
                self.runs = vec![];
                self.notice = notice(&f, Some(&e));
            }
        }
        let present: HashSet<i64> = self.runs.iter().map(|r| r.id).collect();
        self.jobs.retain(|id, _| present.contains(id)); // runs that dropped off the commit (superseded, deleted)

        // Only re-fetch jobs for runs that can still change; finished runs are immutable (a rerun resets their status).
        let stale: Vec<i64> = self
            .runs
            .iter()
            .filter(|r| !self.jobs.contains_key(&r.id) || !matches!(r.status.as_deref(), Some("success" | "failure" | "cancelled" | "skipped")))
            .map(|r| r.id)
            .collect();
        for chunk in stale.chunks(8) {
            let got: Vec<(i64, Result<Vec<ActionJob>>)> = std::thread::scope(|s| {
                let f = &f;
                let hs: Vec<_> = chunk.iter().map(|&id| (id, s.spawn(move || client.ci_jobs(repo, id, f)))).collect();
                hs.into_iter().map(|(id, h)| (id, h.join().unwrap())).collect()
            });
            for (id, js) in got { self.jobs.insert(id, js?); }
        }

        let mut checks: Vec<Check> = statuses.into_iter().map(|s| Check { job: job_for(&s, &self.runs, &self.jobs), status: s }).collect();
        checks.sort_by_cached_key(|c| (c.state(), c.status.context.to_lowercase()));
        self.checks = checks;
        self.last_updated = Some(Instant::now());
        self.error = None;
        Ok(())
    }

    /// Runs a rerun or cancel; None on success, otherwise what went wrong in words the user can act on.
    pub fn perform(&mut self, a: CiAction, job: &ActionJob, client: &Client, repo: &RemoteRepo) -> Option<String> {
        let f = self.features.clone()?;
        match client.ci_perform(a, repo, job, &f) {
            Ok(()) => {
                self.jobs.remove(&job.run_id);
                self.refresh(client, repo);
                None
            }
            Err(Error::Cancelled) => None,
            Err(e) => Some(f.explain(&e)),
        }
    }
}

pub fn notice(f: &CiFeatures, error: Option<&Error>) -> Option<String> {
    if !f.actions {
        return Some(format!("{} {} has no Actions API (it arrived in 1.25), so checks show their status only.", f.name(), f.version.as_deref().unwrap_or("")));
    }
    if !error?.to_string().contains(" 403") { return None; }
    Some(if f.forge == Forge::GitHub {
        "This token can't read GitHub Actions (403), so checks show their status only. Jobs and logs need “Actions: Read”.".into()
    } else {
        format!("{} won't list Actions runs for this account (403), so checks show their status only.", f.name())
    })
}

/// The job behind a status row.
/// - GitHub: an Actions check run's id is its job's id.
/// - Forgejo/Gitea: target_url "/acme/web/actions/runs/292970/jobs/4" → run index 292970 (or id, on servers that
///   link by id), job #4 of that run; failing that, the job whose name the context "workflow / job (event)" carries.
pub fn job_for(s: &CommitStatus, runs: &[ActionRun], jobs: &HashMap<i64, Vec<ActionJob>>) -> Option<ActionJob> {
    if let Some(cr) = &s.check_run { return jobs.values().flatten().find(|j| j.id == cr.id).cloned(); }
    let url = s.target_url.as_deref()?;
    let at = url.find("/actions/runs/")?;
    let parts: Vec<&str> = url[at + "/actions/runs/".len()..].split('/').filter(|p| !p.is_empty()).collect();
    let n: i64 = parts.first()?.parse().ok()?;
    let run = runs.iter().find(|r| r.index_in_repo == n).or_else(|| runs.iter().find(|r| r.id == n))?;
    let list = jobs.get(&run.id)?;
    let job_no: Option<i64> = if parts.len() >= 3 && parts[1] == "jobs" { parts[2].parse().ok() } else { None };
    // Job html_url carries ".../runs/<index>/jobs/<n>/attempt/<a>"; keep the newest attempt per n.
    let mut by_no: HashMap<i64, &ActionJob> = HashMap::new();
    for j in list {
        let Some(u) = j.html_url.as_deref() else { continue };
        let Some(jr) = u.find("/jobs/") else { continue };
        let n = u[jr + 6..].split('/').next().and_then(|x| x.parse().ok()).unwrap_or(-1);
        if by_no.get(&n).map_or(0, |b| b.attempt.unwrap_or(0)) <= j.attempt.unwrap_or(0) { by_no.insert(n, j); }
    }
    if let Some(j) = job_no.and_then(|no| by_no.get(&no)) { return Some((*j).clone()); }
    if job_no.is_none() && by_no.len() == 1 { return by_no.values().next().map(|j| (*j).clone()); }
    list.iter()
        .filter(|j| s.context == j.name || s.context.contains(&format!(" / {}", j.name)))
        .max_by_key(|j| (j.name.chars().count(), j.attempt.unwrap_or(0)))
        .cloned()
}

/// "1m 5s" between two ISO 8601 times (the second defaults to `now`, unix seconds); None past three days or
/// before the start.
pub fn duration(a: Option<&str>, b: Option<&str>, now: i64) -> Option<String> {
    let s = parse_iso8601(a?)?;
    let e = b.and_then(parse_iso8601).unwrap_or(now);
    let secs = e - s;
    if !(0..86_400 * 3).contains(&secs) { return None; }
    Some(if secs >= 60 { format!("{}m {}s", secs / 60, secs % 60) } else { format!("{secs}s") })
}

/// "2026-11-01T10:00:02Z", "…T10:00:02.5Z", "…T10:00:02+03:00" → unix seconds.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' { return None; }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, sec) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        while b.get(i).is_some_and(|c| c.is_ascii_digit()) { i += 1; }
    }
    let offset = match b.get(i)? {
        b'Z' => 0,
        c @ (b'+' | b'-') => {
            let oh = num(i + 1..i + 3)?;
            let om = num(i + 4..i + 6)?;
            (oh * 3600 + om * 60) * if *c == b'-' { -1 } else { 1 }
        }
        _ => return None,
    };
    // days from civil (Howard Hinnant)
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

// MARK: - Log formatting

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogStyle {
    Plain,
    /// A `##[group]` header: secondary, semibold, clickable.
    Header,
    /// `##[error]`: red, semibold.
    Error,
    /// `##[warning]`: orange.
    Warning,
    /// `##[notice]`: accent.
    Notice,
    /// `##[debug]`: tertiary.
    Debug,
    /// `[command]`: secondary.
    Command,
    /// "⭐ …" (Forgejo runner step headers): accent, semibold.
    Star,
    /// A line with ✅: green.
    Success,
    /// A line that reads like a failure: red.
    Problem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// Without the runner timestamp and workflow command. Headers read "▸ title"; a view shows ▾ for an open one.
    pub text: String,
    pub style: LogStyle,
    /// The `##[group]` this body line belongs to: folded unless the group is in `JobLog::expanded`.
    pub group: Option<u32>,
    /// Set on a header line: the group it opens and closes.
    pub header: Option<u32>,
}

/// A line to jump to: index into `JobLog::lines`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LogProblem {
    pub line: String,
    pub index: usize,
    pub warning: bool,
}

/// The ##[group] open at the end of a chunk, and the next group id to hand out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Fold {
    pub open: Option<u32>,
    pub next: u32,
}

pub fn is_problem(l: &str) -> bool {
    l.starts_with("##[error]")
        || l.contains('❌')
        || l.starts_with("error")
        || l.contains("error:")
        || l.contains("Error:")
        || l.contains("FAILED")
        || l.contains("panicked at")
        || l.starts_with("thread '") && l.contains("panicked")
}

/// Strips runner timestamps and turns workflow commands into readable, styled lines. A `##[group]` line becomes a
/// "▸ title" header, and the lines up to `##[endgroup]` carry its group so the view can fold them.
pub fn format_log(s: &str, fold: &mut Fold) -> Vec<LogLine> {
    let owned;
    let s = if s.contains("\r\n") {
        owned = s.replace("\r\n", "\n");
        owned.as_str()
    } else {
        s
    };
    let body = s.strip_suffix('\n').unwrap_or(s);
    if s.is_empty() { return vec![]; }
    let mut out = Vec::new();
    for raw in body.split('\n') {
        let mut l = raw.strip_prefix('\u{FEFF}').unwrap_or(raw); // GitHub's logs start with a BOM
        // "2026-10-19T14:33:38.5145900Z " prefix
        let b = l.as_bytes();
        if b.len() > 11 && b[..11].is_ascii() && b[4] == b'-' && b[10] == b'T' && l.chars().nth(20).is_some()
            && let Some(z) = b[..b.len().min(32)].iter().position(|&c| c == b'Z') {
                l = &l[z + 1..];
                l = l.strip_prefix(' ').unwrap_or(l); // only the separator; runner indentation is meaningful
            }
        if l.starts_with("##[endgroup]") {
            fold.open = None;
            continue;
        }
        if let Some(t) = l.strip_prefix("##[group]") {
            let id = fold.next;
            fold.next += 1;
            fold.open = Some(id);
            out.push(LogLine { text: format!("▸ {t}"), style: LogStyle::Header, group: None, header: Some(id) });
            continue;
        }
        let (text, style) = if let Some(t) = l.strip_prefix("##[error]") {
            (t, LogStyle::Error)
        } else if let Some(t) = l.strip_prefix("##[warning]") {
            (t, LogStyle::Warning)
        } else if let Some(t) = l.strip_prefix("##[notice]") {
            (t, LogStyle::Notice)
        } else if let Some(t) = l.strip_prefix("##[debug]") {
            (t, LogStyle::Debug)
        } else if let Some(t) = l.strip_prefix("[command]") {
            (t, LogStyle::Command)
        } else if l.starts_with('⭐') {
            (l, LogStyle::Star)
        } else if l.contains('✅') {
            (l, LogStyle::Success)
        } else if is_problem(l) {
            (l, LogStyle::Problem)
        } else {
            (l, LogStyle::Plain)
        };
        out.push(LogLine { text: text.to_string(), style, group: fold.open, header: None });
    }
    out
}

/// Error lines (red) and ##[warning] lines (orange), for the click-to-jump list. Group headers are skipped. At
/// most 200 per chunk; `base` is the index of the chunk's first line.
pub fn find_problems(lines: &[LogLine], base: usize) -> Vec<LogProblem> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.header.is_none() && (is_problem(&l.text) || matches!(l.style, LogStyle::Error | LogStyle::Problem | LogStyle::Warning)))
        .take(200)
        .map(|(i, l)| LogProblem { line: l.text.trim().to_string(), index: base + i, warning: l.style == LogStyle::Warning })
        .collect()
}

/// The last `limit` bytes of `data` (which starts at byte `start` of the log), from a line start: (bytes, their start).
pub fn tail(data: Vec<u8>, start: usize, limit: usize) -> (Vec<u8>, usize) {
    if data.len() <= limit { return (data, start); }
    let mut cut = data.len() - limit;
    if let Some(nl) = data[cut..].iter().position(|&b| b == b'\n') { cut += nl + 1; }
    (data[cut..].to_vec(), start + cut)
}

/// A tail request ("bytes=<offset>-") answered by a server that ignores Range comes back whole, from byte 0: keep
/// only what's past the offset.
pub fn new_bytes(data: Vec<u8>, start: usize, offset: usize) -> Vec<u8> {
    if start != 0 || offset == 0 { return data; }
    if data.len() > offset { data[offset..].to_vec() } else { vec![] }
}

// MARK: - Job log

/// The detail page fetches the whole log; only beyond 16 MB does it start from the tail.
pub const INITIAL_TAIL: usize = 16 << 20;
/// A live tail that keeps growing drops its oldest output past this (bytes of text).
pub const MAX_SHOWN: usize = 24 << 20;
/// The newest ones.
pub const MAX_PROBLEMS: usize = 1000;

/// One job's log, tailed while the job runs. The work is split so the network and formatting run off the UI
/// thread while the UI owns the state: `let req = log.poll_request(); let resp = req.fetch(&client, &repo);
/// let next = log.apply(resp);` — or [`JobLog::run`] does that loop on a shared `Mutex<JobLog>`.
#[derive(Debug, Clone)]
pub struct JobLog {
    pub job: ActionJob,
    pub features: CiFeatures,
    pub lines: Vec<LogLine>,
    pub problems: Vec<LogProblem>,
    /// Bytes before what's shown (huge logs start from the tail; a long live tail drops its oldest output).
    pub truncated_bytes: usize,
    pub error: Option<String>,
    /// The server has no log for this job yet (GitHub serves it once the job ends).
    pub waiting_for_log: bool,
    /// ##[group]s the user opened; all start folded, as on the web.
    pub expanded: HashSet<u32>,
    /// Bumps on every change, so the view can follow the tail.
    pub version: u64,
    offset: usize,
    /// Bytes after the last newline (a chunk can split a line or a UTF-8 char).
    pending: Vec<u8>,
    /// A group can open in one chunk and close in the next.
    fold: Fold,
    /// The live tail dropped its oldest output.
    trimmed: bool,
    shown: usize,
    loaded: bool,
    started: bool,
    polled: bool,
    /// GitHub publishes a finished job's log a few seconds after the job ends.
    grace: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogOp {
    /// The first read, or a retry while there was no log yet.
    Initial,
    Tail,
    Earlier,
    Whole,
}

/// What to fetch. Carries copies of the state the fetch needs, so fetching borrows nothing.
#[derive(Debug, Clone)]
pub struct LogRequest {
    op: LogOp,
    job_id: i64,
    refresh_job: bool,
    features: CiFeatures,
    range: Option<String>,
    offset: usize,
    pending: Vec<u8>,
    fold: Fold,
    limit: usize,
}

pub struct LogResponse {
    op: LogOp,
    job: Option<ActionJob>,
    result: Result<Fetched>,
}

struct Fetched {
    /// Bytes the server sent: 0 for a tail that brought nothing new.
    len: usize,
    /// Where what's shown starts (after cutting a whole log to its tail).
    start: usize,
    /// The log offset read up to: the next tail starts here.
    end: usize,
    lines: Vec<LogLine>,
    pending: Vec<u8>,
    fold: Fold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogNext {
    /// Poll again after this long.
    Again(Duration),
    /// The job finished and its log is complete, or it never got one.
    Done,
}

impl LogRequest {
    /// Blocking: the job (when polling), the log bytes, and their formatting.
    pub fn fetch(self, client: &Client, repo: &RemoteRepo) -> LogResponse {
        let job = if self.refresh_job { client.ci_job(repo, self.job_id, &self.features).ok() } else { None };
        let result = client.action_log(repo, self.job_id, self.range.as_deref()).map(|(data, start, total)| self.format(data, start, total));
        LogResponse { op: self.op, job, result }
    }

    fn format(&self, data: Vec<u8>, start: usize, total: Option<usize>) -> Fetched {
        let mut fold = self.fold;
        match self.op {
            LogOp::Initial | LogOp::Tail => {
                let len = data.len();
                let (data, start, end) = if self.op == LogOp::Initial {
                    let end = total.unwrap_or(start + len);
                    // GitHub's blob store and Gitea send it whole: keep the same tail Forgejo would have sent.
                    let (d, s) = tail(data, start, INITIAL_TAIL);
                    (d, s, end)
                } else {
                    let d = new_bytes(data, start, self.offset);
                    let end = total.unwrap_or(self.offset + d.len());
                    (d, start, end)
                };
                let mut buf = self.pending.clone();
                buf.extend_from_slice(&data);
                let (chunk, pending) = match buf.iter().rposition(|&b| b == b'\n') {
                    Some(nl) => {
                        let rest = buf.split_off(nl + 1);
                        (buf, rest)
                    }
                    None => (vec![], buf),
                };
                let lines = format_log(&String::from_utf8_lossy(&chunk), &mut fold);
                Fetched { len, start, end, lines, pending, fold }
            }
            LogOp::Earlier | LogOp::Whole => {
                let mut data = data;
                if start == 0 && data.len() > self.limit { data.truncate(self.limit); } // a server that ignored Range
                let lines = format_log(&String::from_utf8_lossy(&data), &mut fold);
                Fetched { len: data.len(), start, end: 0, lines, pending: vec![], fold }
            }
        }
    }
}

impl JobLog {
    pub fn new(job: ActionJob, features: CiFeatures) -> Self {
        Self {
            job,
            features,
            lines: vec![],
            problems: vec![],
            truncated_bytes: 0,
            error: None,
            waiting_for_log: false,
            expanded: HashSet::new(),
            version: 0,
            offset: 0,
            pending: vec![],
            fold: Fold::default(),
            trimmed: false,
            shown: 0,
            loaded: false,
            started: false,
            polled: false,
            grace: 6,
        }
    }

    pub fn is_running(&self) -> bool {
        !CheckState::from_job(&self.job.status).is_done()
    }

    /// The whole text as shown ("Copy Log").
    pub fn text(&self) -> String {
        let mut s = String::with_capacity(self.shown);
        for l in &self.lines {
            s += &l.text;
            s.push('\n');
        }
        s
    }

    /// A group header was clicked.
    pub fn toggle_group(&mut self, id: u32) {
        if !self.expanded.remove(&id) { self.expanded.insert(id); }
        self.version += 1;
    }

    /// Opens the group holding line `index`, so a problem line inside a folded group can be shown.
    pub fn unfold(&mut self, index: usize) {
        if let Some(g) = self.lines.get(index).and_then(|l| l.group)
            && self.expanded.insert(g) { self.version += 1; }
    }

    /// Whether line `index` is inside a folded group.
    pub fn is_hidden(&self, index: usize) -> bool {
        self.lines.get(index).and_then(|l| l.group).is_some_and(|g| !self.expanded.contains(&g))
    }

    pub fn poll_request(&mut self) -> LogRequest {
        let first = !self.started;
        self.started = true;
        let op = if self.loaded { LogOp::Tail } else { LogOp::Initial };
        let range = match op {
            LogOp::Tail => Some(format!("bytes={}-", self.offset)),
            // The whole log, or its last 16 MB on Forgejo (GitHub's blob store and Gitea don't promise suffix ranges).
            _ => (!self.features.github_shaped()).then(|| format!("bytes=-{INITIAL_TAIL}")),
        };
        self.request(op, range, !first, self.fold, 0)
    }

    fn request(&self, op: LogOp, range: Option<String>, refresh_job: bool, fold: Fold, limit: usize) -> LogRequest {
        LogRequest {
            op,
            job_id: self.job.id,
            refresh_job,
            features: self.features.clone(),
            range,
            offset: self.offset,
            pending: if matches!(op, LogOp::Initial | LogOp::Tail) { self.pending.clone() } else { vec![] },
            fold,
            limit,
        }
    }

    /// "Load Full Log": what was cut off before the tail. None when everything is shown.
    pub fn earlier_request(&self) -> Option<LogRequest> {
        if self.truncated_bytes == 0 { return None; }
        if self.trimmed {
            // After the live tail dropped output, what's shown no longer starts at a byte we know: read it all
            // again, up to where the tail has read (pending: emitted later).
            let upto = self.offset.saturating_sub(self.pending.len());
            return Some(self.request(LogOp::Whole, Some(format!("bytes=0-{}", upto.saturating_sub(1))), false, Fold::default(), upto));
        }
        let fold = Fold { open: None, next: self.fold.next + 1_000_000 }; // ids apart from the tail's
        Some(self.request(LogOp::Earlier, Some(format!("bytes=0-{}", self.truncated_bytes - 1)), false, fold, self.truncated_bytes))
    }

    pub fn apply(&mut self, r: LogResponse) -> LogNext {
        if let Some(j) = r.job { self.job = j; }
        match r.op {
            LogOp::Initial => match r.result {
                Ok(f) => {
                    self.offset = f.end;
                    self.truncated_bytes = f.start;
                    self.waiting_for_log = false;
                    self.error = None;
                    self.loaded = true;
                    self.pending = f.pending;
                    self.fold = f.fold;
                    self.append(f.lines);
                }
                Err(Error::Cancelled) => return LogNext::Done,
                Err(e) => {
                    // poll gives up after its grace
                    if self.is_running() || self.waiting_for_log { self.waiting_for_log = true } else { self.error = e.user_message().or(self.error.take()) }
                }
            },
            LogOp::Tail => match r.result {
                Ok(f) if f.len > 0 => {
                    self.offset = f.end;
                    self.pending = f.pending;
                    self.fold = f.fold;
                    self.append(f.lines);
                }
                Err(Error::Cancelled) => return LogNext::Done,
                _ => {}
            },
            LogOp::Earlier => {
                match r.result {
                    Ok(f) => {
                        let n = f.lines.len();
                        let found = find_problems(&f.lines, 0);
                        self.shown += f.lines.iter().map(|l| l.text.len() + 1).sum::<usize>();
                        self.lines.splice(0..0, f.lines);
                        let mut ps = found;
                        ps.extend(self.problems.drain(..).map(|p| LogProblem { index: p.index + n, ..p }));
                        let drop = ps.len().saturating_sub(MAX_PROBLEMS);
                        self.problems = ps.split_off(drop);
                        self.truncated_bytes = 0;
                        self.version += 1;
                    }
                    Err(e) => self.error = e.user_message().or(self.error.take()),
                }
                return LogNext::Done;
            }
            LogOp::Whole => {
                match r.result {
                    Ok(f) => {
                        let mut found = find_problems(&f.lines, 0);
                        self.shown = f.lines.iter().map(|l| l.text.len() + 1).sum();
                        self.lines = f.lines;
                        self.fold = f.fold;
                        self.expanded.clear();
                        let drop = found.len().saturating_sub(MAX_PROBLEMS);
                        self.problems = found.split_off(drop);
                        self.truncated_bytes = 0;
                        self.trimmed = false;
                        self.version += 1;
                    }
                    Err(e) => self.error = e.user_message().or(self.error.take()),
                }
                return LogNext::Done;
            }
        }
        // The very first read is followed at once by the first poll (job refresh + tail), as the Swift loop did.
        if !self.polled {
            self.polled = true;
            return LogNext::Again(Duration::ZERO);
        }
        if !self.is_running() {
            if self.loaded {
                self.flush_pending();
                return LogNext::Done; // finished: one last read above, then stop
            }
            self.grace -= 1;
            if self.grace < 0 {
                self.waiting_for_log = false;
                if self.error.is_none() { self.error = Some(format!("{} has no log for this job.", self.features.name())); }
                return LogNext::Done;
            }
        }
        LogNext::Again(if self.loaded { LOG_TAIL_INTERVAL } else { LOG_WAIT_INTERVAL })
    }

    fn append(&mut self, lines: Vec<LogLine>) {
        if lines.is_empty() { return; }
        let base = self.lines.len();
        let found = find_problems(&lines, base);
        self.shown += lines.iter().map(|l| l.text.len() + 1).sum::<usize>();
        self.lines.extend(lines);
        self.problems.extend(found);
        self.trim_front();
        if self.problems.len() > MAX_PROBLEMS {
            let n = self.problems.len() - MAX_PROBLEMS;
            self.problems.drain(..n);
        }
        self.version += 1;
    }

    fn flush_pending(&mut self) {
        if self.pending.is_empty() { return; }
        let mut s = String::from_utf8_lossy(&self.pending).into_owned();
        s.push('\n');
        self.pending.clear();
        let mut f = self.fold;
        let lines = format_log(&s, &mut f);
        self.fold = f;
        self.append(lines);
    }

    /// A live tail past [`MAX_SHOWN`]: drop the oldest quarter (whole lines), so a job that logs for hours doesn't
    /// grow without end. What's dropped counts as not loaded: Load Full Log brings it back.
    pub fn trim_front(&mut self) {
        if self.shown <= MAX_SHOWN { return; }
        let cut = self.shown - MAX_SHOWN * 3 / 4;
        let (mut dropped, mut n) = (0, 0);
        while n < self.lines.len() - 1 && dropped <= cut {
            dropped += self.lines[n].text.len() + 1;
            n += 1;
        }
        self.lines.drain(..n);
        self.shown -= dropped;
        self.truncated_bytes += dropped; // about: formatting changed some lines
        self.trimmed = true;
        self.problems.retain(|p| p.index >= n);
        for p in &mut self.problems { p.index -= n; }
    }

    /// The Swift poll loop, for a log shared with the UI: fetches outside the lock, applies inside it, calls
    /// `changed` after every apply. Returns when the job's log is complete or `cancel` fires.
    pub fn run(log: &Mutex<JobLog>, client: &Client, repo: &RemoteRepo, cancel: &Cancel, mut changed: impl FnMut()) {
        let client = client.with_cancel(cancel);
        loop {
            let req = log.lock().unwrap().poll_request();
            let resp = req.fetch(&client, repo);
            if cancel.is_cancelled() { return; }
            let next = log.lock().unwrap().apply(resp);
            changed();
            match next {
                LogNext::Done => return,
                LogNext::Again(d) => {
                    if !cancel.sleep(d) { return; }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RemoteRepo {
        RemoteRepo::parse("https://git.example.com/acme/web.git").unwrap()
    }

    #[test]
    fn detects_ci_features_from_version() {
        let gh = CiFeatures::detect(true, None);
        assert!(gh.forge == Forge::GitHub && gh.github_shaped() && gh.rerun_run && gh.rerun_failed && gh.rerun_job && gh.cancel);

        let fj17 = CiFeatures::detect(false, Some("17.0.0-100-2254cb0+gitea-1.22.0"));
        assert!(fj17.forge == Forge::Forgejo && !fj17.github_shaped() && fj17.actions);
        assert!(fj17.rerun_run && fj17.rerun_job && fj17.cancel && !fj17.rerun_failed);
        // Newer Forgejo releases report a bare version, without "+gitea-": still Forgejo, not Gitea.
        let bare = CiFeatures::detect(false, Some("17.0.0"));
        assert!(bare.forge == Forge::Forgejo && !bare.github_shaped() && bare.rerun_run);
        let fj16 = CiFeatures::detect(false, Some("16.0.2+gitea-1.22.0"));
        assert!(fj16.cancel && !fj16.rerun_run && !fj16.rerun_job);
        let fj11 = CiFeatures::detect(false, Some("11.0.16+gitea-1.22.0"));
        assert!(fj11.actions && !fj11.cancel && !fj11.rerun_run);

        let gt124 = CiFeatures::detect(false, Some("1.24.6"));
        assert!(gt124.forge == Forge::Gitea && !gt124.actions && gt124.actions(Some("failure"), "failure").is_empty());
        let gt125 = CiFeatures::detect(false, Some("1.25.1"));
        assert!(gt125.actions && gt125.github_shaped() && !gt125.rerun_run);
        let gt126 = CiFeatures::detect(false, Some("1.26.0"));
        assert!(gt126.rerun_run && gt126.rerun_failed && gt126.rerun_job && !gt126.cancel);
        assert!(CiFeatures::detect(false, Some("1.27.0+dev-160-g55186ca39b")).cancel);

        // Version unreadable: keep Actions (as before), offer nothing that writes.
        let unknown = CiFeatures::detect(false, None);
        assert!(unknown.forge == Forge::Forgejo && unknown.actions && unknown.actions(Some("failure"), "failure").is_empty());
    }

    #[test]
    fn offers_rerun_and_cancel_by_state() {
        use CiAction::*;
        let gh = CiFeatures::detect(true, None);
        assert_eq!(gh.actions(Some("failure"), "failure"), [RerunJob, RerunFailed, RerunAll]);
        assert_eq!(gh.actions(Some("success"), "success"), [RerunJob, RerunAll]);
        // A job can't be re-run while its run is still moving (every forge refuses it).
        assert_eq!(gh.actions(Some("running"), "success"), [Cancel]);
        assert_eq!(gh.actions(Some("cancelled"), "cancelled"), [RerunJob, RerunFailed, RerunAll]);
        assert_eq!(gh.actions(Some("running"), "running"), [Cancel]);
        let fj = CiFeatures::detect(false, Some("17.0.0+gitea-1.22.0"));
        assert_eq!(fj.actions(Some("failure"), "failure"), [RerunJob, RerunAll]);

        let r = repo();
        assert_eq!(fj.path(RerunJob, &r, 7, 42), "/repos/acme/web/actions/jobs/42/rerun");
        assert_eq!(fj.path(Cancel, &r, 7, 42), "/repos/acme/web/actions/runs/7/cancel");
        let gt = CiFeatures::detect(false, Some("1.26.0"));
        assert_eq!(gt.path(RerunJob, &r, 7, 42), "/repos/acme/web/actions/runs/7/jobs/42/rerun");
        assert_eq!(gh.path(RerunFailed, &r, 7, 42), "/repos/acme/web/actions/runs/7/rerun-failed-jobs");
        assert_eq!(gh.path(RerunAll, &r, 7, 42), "/repos/acme/web/actions/runs/7/rerun");

        let e = |s: &str| Error::Http { server: s.into(), status: 403, message: "nope".into() };
        assert!(gh.explain(&e("GitHub")).contains("Actions: Read and write"));
        assert!(gh.explain(&e("GitHub")).contains("(403)"));
        assert!(fj.explain(&e("Forgejo")).contains("write access"));
        assert!(notice(&fj, Some(&e("Forgejo"))).unwrap().contains("Forgejo won't list"));
        assert!(notice(&fj, None).is_none());
        assert!(notice(&CiFeatures::detect(false, Some("1.24.6")), None).unwrap().contains("no Actions API"));
    }

    #[test]
    fn maps_github_runs_and_jobs() {
        let json = r#"
        {"total_count":1,"jobs":[{"id":555,"run_id":99,"run_attempt":2,"name":"build (macos)","html_url":"https://github.com/acme/web/actions/runs/99/job/555",
          "labels":["macos-15"],"status":"in_progress","conclusion":null,"started_at":"2026-11-01T10:00:00Z","completed_at":null,
          "steps":[{"number":1,"name":"Set up job","status":"completed","conclusion":"success","started_at":"2026-11-01T10:00:00Z","completed_at":"2026-11-01T10:00:02Z"},
                   {"number":2,"name":"Test","status":"in_progress","conclusion":null,"started_at":"2026-11-01T10:00:02Z","completed_at":null},
                   {"number":3,"name":"Upload","status":"queued","conclusion":null}]}]}"#;
        #[derive(Deserialize)]
        struct Page {
            jobs: Vec<GhJob>,
        }
        let job = serde_json::from_str::<Page>(json).unwrap().jobs.remove(0).forgejo();
        assert!(job.id == 555 && job.run_id == 99 && job.attempt == Some(2) && job.runs_on == Some(vec!["macos-15".into()]) && job.status == "running");
        let steps = job.steps.unwrap();
        assert_eq!(steps.iter().map(|s| s.status.as_str()).collect::<Vec<_>>(), ["success", "running", "waiting"]);
        assert_eq!(steps[0].stopped.as_deref(), Some("2026-11-01T10:00:02Z"));

        assert_eq!(gh_status(Some("completed"), Some("timed_out")), "failure");
        assert_eq!(gh_status(Some("completed"), Some("neutral")), "success");
        assert_eq!(gh_status(Some("completed"), Some("cancelled")), "cancelled");
        assert_eq!(gh_status(Some("queued"), None), "waiting");
        assert_eq!(gh_status(Some("success"), None), "success"); // a Gitea build that already speaks Forgejo's statuses

        let run = GhRun { id: 99, run_number: Some(12), display_title: Some("Fix login".into()), status: Some("completed".into()), conclusion: Some("failure".into()), ..Default::default() }.forgejo();
        assert!(run.id == 99 && run.index_in_repo == 12 && run.title.as_deref() == Some("Fix login") && run.status.as_deref() == Some("failure"));
    }

    fn job(id: i64, name: &str, status: &str) -> ActionJob {
        ActionJob { id, run_id: 99, attempt: Some(1), name: name.into(), status: status.into(), ..Default::default() }
    }
    fn status(id: i64, st: &str, ctx: &str, url: Option<&str>, cr: Option<CheckRunRef>) -> CommitStatus {
        CommitStatus { id, status: st.into(), context: ctx.into(), target_url: url.map(str::to_string), check_run: cr, ..Default::default() }
    }

    #[test]
    fn matches_checks_to_jobs() {
        let j = job(555, "build", "failure");
        let other = job(556, "lint", "success");
        let runs = [ActionRun { id: 99, index_in_repo: 12, ..Default::default() }];
        let jobs = HashMap::from([(99, vec![j.clone(), other.clone()])]);

        // GitHub: an Actions check run's id is its job's id; a third-party check run matches nothing.
        let actions = status(555, "failure", "build", None, Some(CheckRunRef { id: 555, app: Some("github-actions".into()), ..Default::default() }));
        assert_eq!(job_for(&actions, &runs, &jobs).map(|j| j.id), Some(555));
        let third = status(9001, "success", "codecov/patch", Some("https://codecov.io/x"), Some(CheckRunRef { id: 9001, app: Some("codecov".into()), ..Default::default() }));
        assert!(job_for(&third, &runs, &jobs).is_none());

        // Forgejo: run index and job number from the status's target_url, the job number from the job's html_url.
        let mut a = j.clone();
        a.html_url = Some("https://git.example.com/acme/web/actions/runs/12/jobs/0/attempt/1".into());
        let mut b = other.clone();
        b.html_url = Some("https://git.example.com/acme/web/actions/runs/12/jobs/1/attempt/1".into());
        let by_url = status(1, "success", "CI / lint (push)", Some("/acme/web/actions/runs/12/jobs/1"), None);
        assert_eq!(job_for(&by_url, &runs, &HashMap::from([(99, vec![a, b])])).map(|j| j.id), Some(556));

        // Gitea linking the run by id, with job URLs that carry no usable number: the job name in the context decides.
        let by_name = status(2, "failure", "CI / build (pull_request)", Some("https://gitea.example.com/acme/web/actions/runs/99/jobs/5"), None);
        assert_eq!(job_for(&by_name, &runs, &jobs).map(|j| j.id), Some(555));
        let unrelated = status(3, "success", "deploy", Some("https://ci.example.com/build/3"), None);
        assert!(job_for(&unrelated, &runs, &jobs).is_none());

        // A pending status with a running job reads as running; rows sort failing first.
        let c = Check { status: status(4, "pending", "x", None, None), job: Some(job(1, "x", "running")) };
        assert_eq!(c.state(), CheckState::Running);
        assert_eq!(CheckState::from_status("pending", Some("Has started running")), CheckState::Running);
        assert_eq!(CheckState::from_status("pending", Some("Blocked by required conditions")), CheckState::Pending);
        assert!(CheckState::Failing < CheckState::Skipped);
    }

    fn texts(l: &[LogLine]) -> String {
        l.iter().map(|l| format!("{}\n", l.text)).collect()
    }

    #[test]
    fn formats_github_logs_with_foldable_groups() {
        let raw = "\u{FEFF}2026-11-01T10:00:00.1234567Z ##[group]Run actions/checkout@v4\n".to_string()
            + "2026-11-01T10:00:00.2Z with:\n"
            + "2026-11-01T10:00:00.3Z   repository: acme/web\n"
            + "2026-11-01T10:00:00.4Z ##[endgroup]\n"
            + "2026-11-01T10:00:01.0Z Syncing repository\n"
            + "2026-11-01T10:00:02.0Z ##[warning]Node.js 16 actions are deprecated.\n"
            + "2026-11-01T10:00:03.0Z ##[error]Process completed with exit code 1.\r\n";
        let out = format_log(&raw, &mut Fold::default());
        assert_eq!(texts(&out), "▸ Run actions/checkout@v4\nwith:\n  repository: acme/web\nSyncing repository\nNode.js 16 actions are deprecated.\nProcess completed with exit code 1.\n");
        // The header opens its group, the body carries it, what follows the group doesn't.
        assert_eq!(out[0].header, Some(0));
        assert_eq!(out[1].group, Some(0));
        assert_eq!(out[2].group, Some(0));
        assert_eq!(out[3].group, None);

        let problems = find_problems(&out, 0);
        assert_eq!(problems.iter().map(|p| p.line.as_str()).collect::<Vec<_>>(), ["Node.js 16 actions are deprecated.", "Process completed with exit code 1."]);
        assert_eq!(problems.iter().map(|p| p.warning).collect::<Vec<_>>(), [true, false]);

        // A group opened in one chunk carries on into the next; the next group gets a new id.
        let mut fold = Fold::default();
        format_log("2026-11-01T10:00:00Z ##[group]Install\n", &mut fold);
        assert!(fold.open == Some(0) && fold.next == 1);
        let second = format_log("2026-11-01T10:00:01Z added 12 packages\n2026-11-01T10:00:02Z ##[endgroup]\n2026-11-01T10:00:03Z ##[group]Test\n", &mut fold);
        assert_eq!(second[0].group, Some(0));
        assert!(fold.open == Some(1) && fold.next == 2);
    }

    #[test]
    fn formats_action_logs() {
        let raw = "2026-10-19T14:33:38.5145900Z ##[group]Getting Git version info\n2026-10-19T14:33:38.5145900Z git version 2.50\n2026-10-19T14:33:38.5145900Z ##[endgroup]\n2026-10-19T14:33:39.0Z ⭐ Run Main Build\n2026-10-19T14:33:40.0Z error[E0308]: mismatched types\n2026-10-19T14:33:41.0Z   ❌  Failure - Main Build\n";
        let out = format_log(raw, &mut Fold::default());
        assert_eq!(texts(&out), "▸ Getting Git version info\ngit version 2.50\n⭐ Run Main Build\nerror[E0308]: mismatched types\n  ❌  Failure - Main Build\n");
        let problems = find_problems(&out, 0);
        assert_eq!(problems.iter().map(|p| p.line.as_str()).collect::<Vec<_>>(), ["error[E0308]: mismatched types", "❌  Failure - Main Build"]);
    }

    #[test]
    fn keeps_only_new_bytes_when_range_is_ignored() {
        let full = b"line 1\nline 2\nline 3\n".to_vec();
        assert_eq!(new_bytes(full.clone(), 0, 7), b"line 2\nline 3\n"); // 200 with the whole log
        assert_eq!(new_bytes(b"line 3\n".to_vec(), 14, 14), b"line 3\n"); // 206
        assert_eq!(new_bytes(full.clone(), 0, 0), full);
        assert!(new_bytes(full.clone(), 0, full.len()).is_empty());
    }

    #[test]
    fn tail_keeps_whole_lines() {
        let (t, start) = tail(b"aaaa\nbbbb\ncccc\n".to_vec(), 0, 7);
        assert_eq!(t, b"cccc\n");
        assert_eq!(start, 10);
        assert_eq!(tail(b"aaaa\nbbbb\ncccc\n".to_vec(), 5, 100).1, 5);
    }

    #[test]
    fn live_tail_drops_its_oldest_output() {
        let mut m = JobLog::new(job(1, "j", "running"), CiFeatures::detect(false, Some("11.0.0")));
        let line = "x".repeat(99);
        let n = MAX_SHOWN / 100 + 1000;
        m.lines = (0..n).map(|_| LogLine { text: line.clone(), style: LogStyle::Plain, group: None, header: None }).collect();
        m.shown = n * 100;
        m.problems = vec![LogProblem { line: "old".into(), index: 5, warning: false }, LogProblem { line: "new".into(), index: n - 1, warning: false }];
        m.trim_front();
        assert!(m.shown <= MAX_SHOWN);
        assert_eq!(m.problems.iter().map(|p| p.line.as_str()).collect::<Vec<_>>(), ["new"]);
        assert_eq!(m.problems[0].index, m.lines.len() - 1);
        assert!(m.truncated_bytes > 0);
        assert!(m.earlier_request().is_some_and(|r| r.op == LogOp::Whole));
    }

    fn fetched(op: LogOp, req: &LogRequest, data: &[u8], start: usize, total: Option<usize>, job: Option<ActionJob>) -> LogResponse {
        LogResponse { op, job, result: Ok(req.format(data.to_vec(), start, total)) }
    }

    #[test]
    fn tails_a_running_job_then_stops_when_it_ends() {
        let mut m = JobLog::new(job(1, "j", "running"), CiFeatures::detect(false, Some("17.0.0")));
        let r = m.poll_request();
        assert_eq!(r.range.as_deref(), Some("bytes=-16777216"));
        assert!(!r.refresh_job);
        // A chunk that ends mid-line keeps the rest pending.
        assert_eq!(m.apply(fetched(LogOp::Initial, &r, b"one\ntw", 0, Some(6), None)), LogNext::Again(Duration::ZERO));
        assert_eq!(texts(&m.lines), "one\n");
        let r = m.poll_request();
        assert_eq!(r.range.as_deref(), Some("bytes=6-"));
        assert!(r.refresh_job);
        // 416: nothing new.
        assert_eq!(m.apply(LogResponse { op: LogOp::Tail, job: None, result: Ok(r.format(vec![], 0, None)) }), LogNext::Again(LOG_TAIL_INTERVAL));
        let r = m.poll_request();
        let done = job(1, "j", "success");
        assert_eq!(m.apply(fetched(LogOp::Tail, &r, b"o\nthree", 6, Some(13), Some(done))), LogNext::Done);
        assert_eq!(texts(&m.lines), "one\ntwo\nthree\n"); // finished: the pending tail flushed
        assert_eq!(m.offset, 13);
    }

    #[test]
    fn gives_up_waiting_for_a_log_after_the_grace() {
        let mut m = JobLog::new(job(1, "j", "success"), CiFeatures::detect(true, None));
        let r = m.poll_request();
        assert_eq!(r.range, None); // GitHub: the whole log
        let miss = || LogResponse { op: LogOp::Initial, job: None, result: Err(Error::Http { server: "Log".into(), status: 404, message: String::new() }) };
        assert_eq!(m.apply(miss()), LogNext::Again(Duration::ZERO));
        let mut n = 0;
        while m.apply(miss()) != LogNext::Done { n += 1; }
        assert_eq!(n, 6);
        assert!(m.error.is_some());
        // Cancellation is not an error.
        let mut c = JobLog::new(job(1, "j", "running"), CiFeatures::detect(true, None));
        assert_eq!(c.apply(LogResponse { op: LogOp::Initial, job: None, result: Err(Error::Cancelled) }), LogNext::Done);
        assert!(c.error.is_none());
    }

    #[test]
    fn loads_earlier_output_in_front() {
        let mut m = JobLog::new(job(1, "j", "success"), CiFeatures::detect(false, Some("17.0.0")));
        let r = m.poll_request();
        let big: Vec<u8> = b"error: early\n".to_vec();
        m.apply(fetched(LogOp::Initial, &r, b"late\n##[error]boom\n", 100, Some(118), None));
        assert_eq!(m.truncated_bytes, 100);
        assert_eq!(m.problems[0].index, 1);
        let r = m.earlier_request().unwrap();
        assert_eq!(r.range.as_deref(), Some("bytes=0-99"));
        assert_eq!(m.apply(fetched(LogOp::Earlier, &r, &big, 0, None, None)), LogNext::Done);
        assert_eq!(texts(&m.lines), "error: early\nlate\nboom\n");
        assert_eq!(m.problems.iter().map(|p| p.index).collect::<Vec<_>>(), [0, 2]);
        assert_eq!(m.truncated_bytes, 0);
        assert!(m.earlier_request().is_none());
    }

    #[test]
    fn durations_and_dates() {
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601("2026-11-01T10:00:00Z"), Some(1_793_527_200));
        assert_eq!(parse_iso8601("2026-11-01T13:00:00+03:00"), Some(1_793_527_200));
        assert_eq!(duration(Some("2026-11-01T10:00:00Z"), Some("2026-11-01T10:01:05.5Z"), 0).as_deref(), Some("1m 5s"));
        assert_eq!(duration(Some("2026-11-01T10:00:00Z"), None, 1_793_527_209).as_deref(), Some("9s"));
        assert_eq!(duration(Some("2026-11-01T10:00:00Z"), Some("2026-10-01T10:00:00Z"), 0), None);
    }
}

#[cfg(test)]
mod live {
    //! Read-only against api.github.com with the `gh` CLI's token: cargo test -p loupe-forge -- --ignored github
    use super::*;

    #[test]
    #[ignore]
    fn github_check_runs_jobs_and_log() {
        let c = Client::new("https://github.com", &crate::account::gh_token("github.com").expect("gh auth login"), true);
        let repo = RemoteRepo::parse("https://github.com/actions/checkout").unwrap();
        #[derive(Deserialize)]
        struct R {
            id: i64,
            head_sha: String,
        }
        #[derive(Deserialize)]
        struct Runs {
            workflow_runs: Vec<R>,
        }
        let latest: Runs = c.get("/repos/actions/checkout/actions/runs", &[("per_page", "1".into()), ("status", "completed".into())]).unwrap();
        let sha = &latest.workflow_runs[0].head_sha;
        let mut checks = Checks::new(sha);
        checks.refresh(&c, &repo);
        assert!(checks.error.is_none(), "{:?}", checks.error);
        // A job of the latest run: GitHub serves old jobs with their steps purged.
        let run_id = latest.workflow_runs[0].id;
        let job = checks.checks.iter().filter_map(|c| c.job.clone()).find(|j| j.run_id == run_id && (j.status == "success" || j.status == "failure")).expect("a matched job");
        let f = checks.features.clone().unwrap();
        let full = c.ci_job(&repo, job.id, &f).unwrap();
        assert!(full.steps.as_ref().is_some_and(|s| !s.is_empty()), "{job:?} {full:?}");
        // 302 to a blob host, fetched without the token.
        let log = Mutex::new(JobLog::new(job, f));
        JobLog::run(&log, &c, &repo, &Cancel::new(), || {});
        let log = log.into_inner().unwrap();
        assert!(log.lines.iter().any(|l| l.header.is_some()), "{:?}", log.error);
    }
}

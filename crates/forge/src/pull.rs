//! Pull request list and detail data: loading rules, the on-disk list cache, viewed state (keyed by a fingerprint
//! of each file's diff), review drafts, reviewer states.
//!
//! Polling rules (PullRequestsView.swift):
//! - The list loads on showing only when its state changed or it's older than [`RELOAD_AFTER`] (30 s): a quick
//!   round trip through another tab shows what's loaded. Only the open list auto-refreshes, every [`POLL`] (60 s),
//!   and on app activation; closed/all are paged and refreshing would throw away scrolled pages. A selected PR
//!   stops the list poll (the detail replaces the list; deselecting reloads it).
//! - An open PR reloads on showing unless loaded in the last 30 s, then every 60 s, on app activation and on
//!   Refresh. Every load refreshes its CI checks too ([`crate::ci::Checks::refresh`]), not only a new push.
//! - One load at a time: a request while one runs makes it run once more after, not alongside.

use crate::client::PULL_PAGE;
use crate::models::*;
use crate::{git, Client, Error, RemoteRepo, Result};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const RELOAD_AFTER: Duration = Duration::from_secs(30);
pub const POLL: Duration = Duration::from_secs(60);
/// Opened PRs kept in memory: a 5,000-file PR's model holds hundreds of MB.
pub const KEPT_DETAILS: usize = 2;

// MARK: - List

#[derive(Debug, Clone)]
pub struct PullList {
    /// "open" | "closed" | "all"
    pub state: String,
    pub pulls: Vec<PullRequest>,
    pub error: Option<String>,
    pub last_updated: Option<Instant>,
    /// The state `pulls` holds.
    pub loaded_state: Option<String>,
    /// PR number.
    pub selected: Option<i64>,
    page: usize,
    exhausted: bool,
}

impl Default for PullList {
    fn default() -> Self {
        Self { state: "open".into(), pulls: vec![], error: None, last_updated: None, loaded_state: None, selected: None, page: 1, exhausted: false }
    }
}

/// A finished list load: the pages that were fetched for `state`.
pub struct ListLoad {
    pub state: String,
    pub result: Result<(Vec<PullRequest>, usize)>,
}

impl PullList {
    /// Coming back to the list within a poll interval shows what's loaded.
    pub fn should_load_on_show(&self) -> bool {
        self.loaded_state.as_deref() != Some(&self.state) || self.last_updated.is_none_or(|t| t.elapsed() > RELOAD_AFTER)
    }

    /// The detail replaces the list, and deselecting reloads it: polling it meanwhile is wasted.
    pub fn wants_load(&self) -> bool {
        self.selected.is_none()
    }

    /// Blocking. Reloads from page 1. The open list is small, so it is fetched completely; closed/all keep just the
    /// first page.
    pub fn fetch(client: &Client, repo: &RemoteRepo, state: &str) -> ListLoad {
        let result = (|| {
            let mut all = client.pulls(repo, state, 1)?;
            let mut p = 1;
            if state == "open" {
                while all.len() == p * PULL_PAGE {
                    p += 1;
                    all.extend(client.pulls(repo, state, p)?);
                }
            }
            Ok((all, p))
        })();
        ListLoad { state: state.into(), result }
    }

    /// Returns whether `pulls` changed. An unchanged poll must not re-render the list or rewrite the cache; a
    /// failed or cancelled one keeps the list it already shows.
    pub fn apply(&mut self, load: ListLoad, repo: &RemoteRepo) -> bool {
        if load.state != self.state { return false; } // state switched mid-flight
        match load.result {
            Ok((all, p)) => {
                let changed = all != self.pulls;
                if changed { self.pulls = all; }
                self.page = p;
                self.exhausted = self.pulls.len() < p * PULL_PAGE;
                self.error = None;
                self.last_updated = Some(Instant::now());
                self.loaded_state = Some(load.state.clone());
                if load.state == "open" && changed { save_cache(repo, self.pulls.clone()); }
                changed
            }
            Err(e) => {
                self.error = e.user_message().or(self.error.take());
                false
            }
        }
    }

    /// The next page to fetch on scrolling to the end, if there is one: (state, page).
    pub fn more(&self) -> Option<(String, usize)> {
        (!self.exhausted).then(|| (self.state.clone(), self.page + 1))
    }

    pub fn apply_more(&mut self, state: &str, page: usize, r: Result<Vec<PullRequest>>) {
        if state != self.state || page != self.page + 1 { return; }
        match r {
            Ok(next) => {
                let seen: HashSet<i64> = self.pulls.iter().map(|p| p.id).collect();
                self.exhausted = next.len() < PULL_PAGE;
                self.pulls.extend(next.into_iter().filter(|p| !seen.contains(&p.id)));
                self.page += 1;
            }
            Err(e) => self.error = e.user_message().or(self.error.take()),
        }
    }

    /// "Opened by me".
    pub fn visible(&self, mine: bool, me: Option<&str>) -> Vec<&PullRequest> {
        self.pulls.iter().filter(|p| !mine || Some(p.user.login.as_str()) == me).collect()
    }

    /// The window shows the last known open list instantly at launch; the fetch replaces it.
    pub fn restore_cache(&mut self, repo: &RemoteRepo) {
        if self.state != "open" || !self.pulls.is_empty() { return; }
        if let Some(cached) = std::fs::read(cache_path(repo)).ok().and_then(|d| serde_json::from_slice(&d).ok()) { self.pulls = cached; }
    }
}

/// ~/Library/Caches/dev.extypen.loupe/pulls-<host>-<owner>-<name>.json, as the Swift app wrote it.
pub fn cache_path(r: &RemoteRepo) -> PathBuf {
    let dir = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/Caches/dev.extypen.loupe");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("pulls-{}-{}-{}.json", r.host, r.owner, r.name).replace(':', "_"))
}

/// Off the calling thread, one write after another so an older list never lands last.
fn save_cache(r: &RemoteRepo, pulls: Vec<PullRequest>) {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let path = cache_path(r);
    std::thread::spawn(move || {
        let _g = LOCK.get_or_init(Default::default).lock().unwrap();
        if let Ok(d) = serde_json::to_vec(&pulls) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, d).is_ok() { let _ = std::fs::rename(&tmp, &path); }
        }
    });
}

/// Opened PRs, most recent last. Only the open PR and the last [`KEPT_DETAILS`] stay; going back to the list lets
/// go of the rest.
#[derive(Debug)]
pub struct DetailCache<T> {
    pub details: HashMap<i64, T>,
    recent: Vec<i64>,
}

impl<T> Default for DetailCache<T> {
    fn default() -> Self {
        Self { details: HashMap::new(), recent: vec![] }
    }
}

impl<T> DetailCache<T> {
    pub fn get_or_insert_with(&mut self, n: i64, selected: Option<i64>, make: impl FnOnce() -> T) -> &mut T {
        self.recent.retain(|&x| x != n);
        self.recent.push(n);
        if let std::collections::hash_map::Entry::Vacant(e) = self.details.entry(n) {
            e.insert(make());
            self.trim(selected);
        }
        self.details.get_mut(&n).unwrap()
    }

    /// Call when the selection changes too (deselecting trims to the recent ones).
    pub fn trim(&mut self, selected: Option<i64>) {
        let mut keep: HashSet<i64> = self.recent.iter().rev().take(KEPT_DETAILS).copied().collect();
        keep.extend(selected);
        self.recent.retain(|n| keep.contains(n));
        self.details.retain(|n, _| keep.contains(n));
    }
}

// MARK: - Detail

/// Where to fetch and diff when the server won't send the diff.
#[derive(Debug, Clone, Default)]
pub struct LocalCheckout {
    pub dir: String,
    pub remote: String,
    pub git_auth: Vec<(String, String)>,
}

/// Everything a PR detail shows besides its diff, loaded in parallel.
#[derive(Debug, Clone)]
pub struct PullDetail {
    pub pr: PullRequest,
    pub reviews: Vec<Review>,
    pub conversation: Vec<IssueComment>,
    /// None when they couldn't be listed: the timeline does without them rather than fail the load.
    pub commits: Option<Vec<PrCommit>>,
    pub comments: Vec<ReviewComment>,
}

/// Blocking. A failed load is retried on the next showing; the caller keeps what it shows.
pub fn load_detail(client: &Client, repo: &RemoteRepo, n: i64) -> Result<PullDetail> {
    let (pr, reviews, conversation, commits) = std::thread::scope(|s| {
        let reviews = s.spawn(|| client.reviews(repo, n));
        let conv = s.spawn(|| client.issue_comments(repo, n));
        let commits = s.spawn(|| client.pull_commits(repo, n));
        (client.pull(repo, n), reviews.join().unwrap(), conv.join().unwrap(), commits.join().unwrap())
    });
    let (pr, reviews, conversation) = (pr?, reviews?, conversation?);
    let comments = client.review_comments(repo, n, &reviews)?;
    let commits = match commits {
        Err(Error::Cancelled) => return Err(Error::Cancelled),
        c => c.ok(),
    };
    Ok(PullDetail { pr, reviews, conversation, commits, comments })
}

/// The PR's diff across polls. Polling: an unchanged diff must not rebuild the table (scroll, selection and
/// highlighting would reset), so [`DiffSource::fetch`] answers None when nothing changed.
#[derive(Debug, Clone, Default)]
pub struct DiffSource {
    last_digest: Option<u64>,
    /// Head the local fallback diff was made at.
    local_sha: Option<String>,
}

impl DiffSource {
    /// Blocking. The server's diff, or for a PR too big for GitHub to serve, the same three-dot diff computed by
    /// local git. None when it's the diff already shown.
    pub fn fetch(&mut self, client: &Client, repo: &RemoteRepo, pr: &PullRequest, local: &LocalCheckout) -> Result<Option<Vec<u8>>> {
        // The local diff of a head already diffed (polling, no new push): skip the work.
        if self.local_sha.as_deref() == Some(&pr.head.sha) && self.last_digest.is_some() { return Ok(None); }
        let data = match client.diff(repo, pr.number) {
            Err(Error::DiffTooLarge) => {
                fetch_head(pr, local, client.cancel())?;
                // From the base sha, not the branch tip: once the PR is merged the tip contains it and `tip...head`
                // is empty. Byte-identical to GitHub's own diff.
                let range = format!("{}...{}", pr.base.sha, pr.head.sha);
                let d = git::run(&["diff", "--no-color", "--no-ext-diff", "-M", &range], &local.dir, &[], client.cancel())?;
                self.local_sha = Some(pr.head.sha.clone());
                d
            }
            r => r?,
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        data.hash(&mut h);
        let digest = h.finish();
        if self.last_digest == Some(digest) { return Ok(None); }
        self.last_digest = Some(digest);
        Ok(Some(data))
    }
}

/// Brings the PR's head and base commits into the local repository (`refs/pull/N/head` works for forks too).
/// Fetching the base branch brings in pr.base.sha. A no-op when both are already here.
pub fn fetch_head(pr: &PullRequest, local: &LocalCheckout, cancel: Option<&crate::Cancel>) -> Result<()> {
    let has = |sha: &str| git::run(&["cat-file", "-e", &format!("{sha}^{{commit}}")], &local.dir, &[], cancel).is_ok();
    if has(&pr.head.sha) && has(&pr.base.sha) { return Ok(()); }
    if let Some(c) = cancel { c.check()?; }
    let head = format!("+refs/pull/{}/head:refs/remotes/{}/pr/{}", pr.number, local.remote, pr.number);
    let base = format!("+refs/heads/{0}:refs/remotes/{1}/{0}", pr.base.ref_, local.remote);
    git::run(&["fetch", "--no-tags", &local.remote, &head, &base], &local.dir, &local.git_auth, None).map(|_| ())
}

// MARK: - Viewed

/// `DiffLine.Kind` raw values, which the fingerprint mixes in.
pub mod line_kind {
    pub const CONTEXT: u8 = 0;
    pub const ADDED: u8 = 1;
    pub const REMOVED: u8 = 2;
    pub const HUNK: u8 = 3;
    pub const META: u8 = 4;
}

/// FNV-1a over each line's text (without its +/-/space) and kind: stable across launches, so a viewed mark
/// survives a restart and a file changed by a new push turns unviewed. Same value as the Swift app's.
pub fn fingerprint<'a>(lines: impl IntoIterator<Item = (&'a str, u8)>) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for (text, kind) in lines {
        for b in text.bytes() { h = (h ^ b as u64).wrapping_mul(0x100000001b3); }
        h = (h ^ kind as u64).wrapping_mul(0x100000001b3);
    }
    h
}

/// The PR's files in tree order (dirs first, so next/prev follows the sidebar), their fingerprints, what's
/// viewed, the filter and the selection. Every method that changes `viewed` returns true when it did: persist it
/// then with [`crate::settings::Prefs::set_viewed`].
#[derive(Debug, Clone, Default)]
pub struct PullFiles {
    paths: Vec<String>,
    fingerprints: HashMap<String, u64>,
    pub viewed: HashMap<String, u64>,
    filter: String,
    pub selected: Option<String>,
    filtered: Vec<usize>,
    under: HashMap<String, Vec<usize>>,
}

impl PullFiles {
    pub fn new(viewed: HashMap<String, u64>) -> Self {
        Self { viewed, ..Default::default() }
    }

    /// New files from a (changed) diff, in tree order with their fingerprints. Keeps the selection when the file
    /// is still there; otherwise selects the first unviewed file.
    pub fn set_files(&mut self, files: Vec<(String, u64)>) {
        self.fingerprints = files.iter().cloned().collect();
        self.paths = files.into_iter().map(|(p, _)| p).collect();
        self.derive();
        if self.selected.as_ref().is_none_or(|s| !self.fingerprints.contains_key(s)) {
            self.selected = self.paths.iter().find(|p| !self.is_viewed(p)).or(self.paths.first()).cloned();
        }
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn set_filter(&mut self, q: &str) {
        self.filter = q.into();
        self.derive();
    }

    fn derive(&mut self) {
        let q = self.filter.to_lowercase();
        self.filtered = (0..self.paths.len()).filter(|&i| q.is_empty() || self.paths[i].to_lowercase().contains(&q)).collect();
        self.under.clear();
        for (k, &i) in self.filtered.iter().enumerate() {
            let p = &self.paths[i];
            for (j, _) in p.match_indices('/') { self.under.entry(p[..j].to_string()).or_default().push(k); }
        }
    }

    pub fn filtered(&self) -> impl Iterator<Item = &str> {
        self.filtered.iter().map(|&i| self.paths[i].as_str())
    }

    /// The filtered files below a directory.
    pub fn under(&self, dir: &str) -> impl Iterator<Item = &str> {
        self.under.get(dir).into_iter().flatten().map(|&k| self.paths[self.filtered[k]].as_str())
    }

    pub fn is_viewed(&self, path: &str) -> bool {
        self.viewed.get(path).is_some_and(|v| self.fingerprints.get(path) == Some(v))
    }

    pub fn toggle_viewed(&mut self, path: &str) -> bool {
        if self.is_viewed(path) {
            self.viewed.remove(path);
        } else {
            let Some(&f) = self.fingerprints.get(path) else { return false };
            self.viewed.insert(path.into(), f);
        }
        true
    }

    /// Directory checkbox: mark everything under it viewed, or clear it if it all already is.
    pub fn toggle_viewed_dir(&mut self, dir: &str) -> bool {
        let fs: Vec<String> = self.under(dir).map(str::to_string).collect();
        let all = fs.iter().all(|f| self.is_viewed(f));
        for f in fs {
            if all {
                self.viewed.remove(&f);
            } else if let Some(&fp) = self.fingerprints.get(&f) {
                self.viewed.insert(f, fp);
            }
        }
        true
    }

    pub fn viewed_count(&self) -> usize {
        self.paths.iter().filter(|p| self.is_viewed(p)).count()
    }

    /// A folder's viewed progress for the tree: (done, total).
    pub fn viewed_progress(&self, dir: &str) -> (usize, usize) {
        let (mut done, mut total) = (0, 0);
        for f in self.under(dir) {
            total += 1;
            done += self.is_viewed(f) as usize;
        }
        (done, total)
    }

    /// Mark the current file viewed and jump to the next unviewed one — the core big-PR loop.
    pub fn mark_viewed_and_advance(&mut self) -> bool {
        let Some(cur) = self.selected.clone() else { return false };
        let changed = !self.is_viewed(&cur) && self.toggle_viewed(&cur);
        let list: Vec<&str> = self.filtered().collect();
        if let Some(idx) = list.iter().position(|p| *p == cur) {
            let next = list[idx + 1..].iter().chain(&list[..idx]).find(|p| !self.is_viewed(p)).map(|p| p.to_string());
            if next.is_some() { self.selected = next; }
        }
        changed
    }

    /// Next/previous file in the filtered list.
    pub fn move_by(&mut self, delta: isize) {
        let list: Vec<&str> = self.filtered().collect();
        let Some(idx) = list.iter().position(|p| Some(*p) == self.selected.as_deref()) else {
            self.selected = list.first().map(|p| p.to_string());
            return;
        };
        let to = (idx as isize + delta).clamp(0, list.len() as isize - 1) as usize;
        self.selected = Some(list[to].to_string());
    }
}

// MARK: - Reviews and comments

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReviewerState {
    Approved,
    ChangesRequested,
    Pending,
}

/// One state per reviewer, as the web sidebar shows it: a push dismisses earlier approvals and re-requested
/// reviewers appear as REQUEST_REVIEW / requested_reviewers — those are pending, not approved.
pub fn reviewer_states(reviews: &[Review], pr: &PullRequest, github: bool) -> Vec<(String, ReviewerState)> {
    let mut latest: HashMap<&str, &Review> = HashMap::new();
    for r in reviews {
        if r.dismissed == Some(true) || r.stale == Some(true) || !["APPROVED", "REQUEST_CHANGES", "REQUEST_REVIEW"].contains(&r.state.as_str()) { continue; }
        if let Some(u) = r.user.as_ref().map(|u| u.login.as_str())
            && latest.get(u).is_none_or(|l| l.id < r.id) { latest.insert(u, r); }
    }
    let mut out: HashMap<String, ReviewerState> = latest
        .into_iter()
        .map(|(u, r)| {
            let s = match r.state.as_str() {
                "APPROVED" => ReviewerState::Approved,
                "REQUEST_CHANGES" => ReviewerState::ChangesRequested,
                _ => ReviewerState::Pending,
            };
            (u.to_string(), s)
        })
        .collect();
    // Forgejo records a re-request as a newer REQUEST_REVIEW review (handled above) and also lists past reviewers in
    // requested_reviewers. GitHub keeps the old approval and lists only open requests, so there an approver in
    // requested_reviewers was re-requested.
    for u in pr.requested_reviewers.iter().flatten() {
        let cur = out.get(&u.login);
        if cur.is_none() || (github && cur == Some(&ReviewerState::Approved)) { out.insert(u.login.clone(), ReviewerState::Pending); }
    }
    let mut v: Vec<_> = out.into_iter().collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineComment {
    pub author: String,
    pub body: String,
    pub new_line: Option<i64>,
    pub old_line: Option<i64>,
    /// Set for the viewer's unsubmitted drafts.
    pub draft_id: Option<String>,
}

/// Per path: the server's review comments, then the drafts ("You").
pub fn inline_comments(comments: &[ReviewComment], drafts: &[DraftComment]) -> HashMap<String, Vec<InlineComment>> {
    let mut m: HashMap<String, Vec<InlineComment>> = HashMap::new();
    for c in comments {
        m.entry(c.path.clone()).or_default().push(InlineComment {
            author: c.user.as_ref().map_or("?".into(), |u| u.login.clone()),
            body: c.body.clone(),
            new_line: (c.position > 0).then_some(c.position),
            old_line: (c.original_position > 0).then_some(c.original_position),
            draft_id: None,
        });
    }
    for d in drafts {
        m.entry(d.path.clone()).or_default().push(InlineComment {
            author: "You".into(),
            body: d.body.clone(),
            new_line: d.new_line,
            old_line: d.old_line,
            draft_id: Some(d.id.clone()),
        });
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn pr(n: i64) -> PullRequest {
        PullRequest {
            id: n,
            number: n,
            title: "t".into(),
            state: "open".into(),
            user: User { login: "a".into(), avatar_url: None },
            head: BranchRef { ref_: "b".into(), sha: "1".into(), label: None },
            base: BranchRef { ref_: "main".into(), sha: "0".into(), label: None },
            ..Default::default()
        }
    }

    #[test]
    fn keeps_only_the_most_recent_details() {
        let mut l: DetailCache<i64> = DetailCache::default();
        for n in 1..=5 { l.get_or_insert_with(n, Some(n), || n * 10); }
        assert_eq!(l.details.keys().copied().collect::<HashSet<_>>(), HashSet::from([4, 5]));
        l.get_or_insert_with(4, Some(4), || unreachable!()); // reopening reuses
        l.get_or_insert_with(3, Some(3), || 30);
        assert_eq!(l.details.keys().copied().collect::<HashSet<_>>(), HashSet::from([3, 4]));
        l.trim(None);
        assert!(l.details.len() <= KEPT_DETAILS);
    }

    #[test]
    fn dismissed_approvals_become_pending_after_push() {
        // A PR after a new push: earlier approvals dismissed, alice re-requested.
        let pr: PullRequest = serde_json::from_str(r#"{"id":1,"number":7347,"title":"t","state":"open","user":{"login":"yasir"},"head":{"ref":"h","sha":"79eb"},"base":{"ref":"main","sha":"b"},"html_url":"x","updated_at":"x","requested_reviewers":[{"login":"alice"},{"login":"carol"}]}"#).unwrap();
        let reviews: Vec<Review> = serde_json::from_str(r#"[{"id":86195,"user":{"login":"alice"},"state":"APPROVED","body":"","stale":true,"dismissed":true},{"id":86199,"user":{"login":"bob"},"state":"APPROVED","body":"","stale":true,"dismissed":true},{"id":86202,"user":{"login":"alice"},"state":"REQUEST_REVIEW","body":"","stale":false,"dismissed":false},{"id":90000,"user":{"login":"carol"},"state":"APPROVED","body":"","stale":false,"dismissed":false}]"#).unwrap();
        let states: HashMap<_, _> = reviewer_states(&reviews, &pr, false).into_iter().collect();
        assert_eq!(states.get("alice"), Some(&ReviewerState::Pending));
        assert_eq!(states.get("bob"), None); // dismissed and not re-requested: not shown
        assert_eq!(states.get("carol"), Some(&ReviewerState::Approved)); // a fresh approval still counts, though Forgejo lists carol as requested
        // GitHub lists only open requests: a requested approver was re-requested.
        let gh: HashMap<_, _> = reviewer_states(&reviews, &pr, true).into_iter().collect();
        assert_eq!(gh.get("carol"), Some(&ReviewerState::Pending));
    }

    #[test]
    fn fingerprint_is_fnv1a_over_text_and_kind() {
        assert_eq!(fingerprint([]), 0xcbf29ce484222325);
        let a = fingerprint([("let x = 1", line_kind::ADDED), ("y", line_kind::CONTEXT)]);
        assert_eq!(a, fingerprint([("let x = 1", line_kind::ADDED), ("y", line_kind::CONTEXT)]));
        assert_ne!(a, fingerprint([("let x = 1", line_kind::REMOVED), ("y", line_kind::CONTEXT)]));
        // FNV-1a of "a" then kind 0.
        let mut h: u64 = 0xcbf29ce484222325;
        h = (h ^ 0x61).wrapping_mul(0x100000001b3);
        h = h.wrapping_mul(0x100000001b3);
        assert_eq!(fingerprint([("a", 0)]), h);
    }

    #[test]
    fn viewed_follows_the_fingerprint_and_advances() {
        let mut f = PullFiles::new(HashMap::from([("src/a.rs".to_string(), 1), ("src/b.rs".to_string(), 99)]));
        f.set_files(vec![("src/a.rs".into(), 1), ("src/b.rs".into(), 2), ("src/c/d.rs".into(), 3), ("z.md".into(), 4)]);
        assert!(f.is_viewed("src/a.rs"));
        assert!(!f.is_viewed("src/b.rs")); // changed by a new push
        assert_eq!(f.selected.as_deref(), Some("src/b.rs")); // first unviewed
        assert_eq!(f.viewed_progress("src"), (1, 3));
        assert!(f.mark_viewed_and_advance());
        assert_eq!(f.selected.as_deref(), Some("src/c/d.rs"));
        assert!(f.toggle_viewed_dir("src"));
        assert_eq!(f.viewed_progress("src"), (3, 3));
        f.toggle_viewed_dir("src");
        assert_eq!(f.viewed_count(), 0);
        f.set_filter("MD");
        assert_eq!(f.filtered().collect::<Vec<_>>(), ["z.md"]);
        f.move_by(1);
        assert_eq!(f.selected.as_deref(), Some("z.md"));
    }

    #[test]
    fn list_apply_keeps_what_it_shows() {
        // "all": only the open list writes the disk cache, and a test must not touch ~/Library/Caches.
        let r = RemoteRepo::parse("http://127.0.0.1:1/o/r.git").unwrap();
        let load = |result| ListLoad { state: "all".into(), result };
        let mut l = PullList { state: "all".into(), ..Default::default() };
        assert!(l.should_load_on_show());
        assert!(l.apply(load(Ok((vec![pr(1)], 1))), &r));
        assert!(!l.should_load_on_show());
        assert!(!l.apply(load(Ok((vec![pr(1)], 1))), &r)); // unchanged
        assert!(!l.apply(load(Err(Error::Cancelled)), &r));
        assert!(l.error.is_none() && l.pulls.len() == 1);
        l.apply(load(Err(Error::msg("offline"))), &r);
        assert_eq!(l.error.as_deref(), Some("offline"));
        assert_eq!(l.pulls.len(), 1);
        assert!(l.more().is_none()); // a short page: exhausted
        l.state = "closed".into();
        assert!(!l.apply(load(Ok((vec![], 1))), &r)); // switched mid-flight
        assert_eq!(l.pulls.len(), 1);
    }

    #[test]
    fn inline_comments_join_drafts() {
        let c = ReviewComment { id: 1, path: "a".into(), body: "x".into(), position: 0, original_position: 4, ..Default::default() };
        let d = DraftComment::new("a", Some(2), None, "mine");
        let m = inline_comments(&[c], std::slice::from_ref(&d));
        assert_eq!(m["a"].len(), 2);
        assert_eq!((m["a"][0].new_line, m["a"][0].old_line), (None, Some(4)));
        assert_eq!(m["a"][1].draft_id.as_deref(), Some(d.id.as_str()));
        assert_eq!(m["a"][0].author, "?");
    }
}

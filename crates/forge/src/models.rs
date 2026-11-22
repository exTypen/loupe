//! Forgejo / Gitea API v1 shapes. GitHub responses are mapped onto these, so views never know which server
//! they talk to. Field names are the servers' (and the Swift app's on-disk PR cache's).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct User {
    pub login: String,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct BranchRef {
    #[serde(rename = "ref")]
    pub ref_: String,
    pub sha: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct PullRequest {
    pub id: i64,
    pub number: i64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub draft: Option<bool>,
    pub user: User,
    pub head: BranchRef,
    pub base: BranchRef,
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
    pub changed_files: Option<i64>,
    pub comments: Option<i64>,
    pub mergeable: Option<bool>,
    pub merged: Option<bool>,
    /// GitHub's list has no `merged`; this tells merged from closed.
    pub merged_at: Option<String>,
    pub requested_reviewers: Option<Vec<User>>,
    pub html_url: String,
    pub updated_at: String,
    pub created_at: Option<String>,
}

impl PullRequest {
    pub(crate) fn normalized(mut self) -> Self {
        if self.merged.is_none() && self.merged_at.is_some() { self.merged = Some(true); }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Review {
    pub id: i64,
    pub user: Option<User>,
    /// APPROVED | REQUEST_CHANGES | COMMENT | REQUEST_REVIEW (GitHub's mapped onto these).
    pub state: String,
    #[serde(default)]
    pub body: String,
    pub commit_id: Option<String>,
    pub stale: Option<bool>,
    pub dismissed: Option<bool>,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ReviewComment {
    pub id: i64,
    pub user: Option<User>,
    pub path: String,
    pub body: String,
    /// Line in the new file, 0 when commenting on the old side.
    #[serde(default)]
    pub position: i64,
    /// Line in the old file.
    #[serde(default)]
    pub original_position: i64,
    pub created_at: Option<String>,
    pub commit_id: Option<String>,
    pub pull_request_review_id: Option<i64>,
    /// The code the comment was made on, ending at its line.
    pub diff_hunk: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Signature {
    pub name: Option<String>,
    pub date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct CommitDetail {
    pub message: String,
    pub author: Option<Signature>,
    pub committer: Option<Signature>,
}

/// A commit on a pull request's branch, as both servers list it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct PrCommit {
    pub sha: String,
    pub commit: CommitDetail,
    pub author: Option<User>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct IssueComment {
    pub id: i64,
    pub user: Option<User>,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct CommitStatus {
    pub id: i64,
    /// pending | success | error | failure | warning | skipped
    pub status: String,
    pub context: String,
    pub description: Option<String>,
    pub target_url: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    /// GitHub check runs only.
    #[serde(skip)]
    pub check_run: Option<CheckRunRef>,
}

/// What a GitHub check run carries beyond its status row. `id` equals the Actions job id for check runs made by
/// GitHub Actions, which is how a row finds its job.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct CheckRunRef {
    pub id: i64,
    /// "github-actions" for Actions jobs.
    pub app: Option<String>,
    /// output.summary, Markdown.
    pub summary: Option<String>,
    /// output.annotations_count.
    pub annotations: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct CheckAnnotation {
    pub path: String,
    pub start_line: Option<i64>,
    /// notice | warning | failure
    pub annotation_level: Option<String>,
    pub title: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ActionRun {
    pub id: i64,
    pub index_in_repo: i64,
    pub title: Option<String>,
    pub workflow_id: Option<String>,
    pub status: Option<String>,
    pub event: Option<String>,
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ActionStep {
    pub number: i64,
    pub name: String,
    pub status: String,
    pub started: Option<String>,
    pub stopped: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ActionJob {
    pub id: i64,
    pub run_id: i64,
    pub attempt: Option<i64>,
    pub name: String,
    pub html_url: Option<String>,
    pub needs: Option<Vec<String>>,
    pub runs_on: Option<Vec<String>>,
    pub task_id: Option<i64>,
    /// success | failure | cancelled | skipped | waiting | running | blocked
    pub status: String,
    pub steps: Option<Vec<ActionStep>>,
}

/// An unsubmitted inline review comment. Persisted as JSON in the settings (`drafts:<host>/<owner>/<repo>#<n>`),
/// in the Swift app's shape: `id` is an uppercase UUID string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DraftComment {
    pub id: String,
    pub path: String,
    #[serde(rename = "newLine")]
    pub new_line: Option<i64>,
    #[serde(rename = "oldLine")]
    pub old_line: Option<i64>,
    pub body: String,
}

impl DraftComment {
    pub fn new(path: impl Into<String>, new_line: Option<i64>, old_line: Option<i64>, body: impl Into<String>) -> Self {
        Self { id: uuid::Uuid::new_v4().to_string().to_uppercase(), path: path.into(), new_line, old_line, body: body.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReviewEvent {
    Comment,
    Approve,
    RequestChanges,
}

impl ReviewEvent {
    /// Forgejo's event names.
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewEvent::Comment => "COMMENT",
            ReviewEvent::Approve => "APPROVED",
            ReviewEvent::RequestChanges => "REQUEST_CHANGES",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MergeStyle {
    Merge,
    Rebase,
    Squash,
}

impl MergeStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            MergeStyle::Merge => "merge",
            MergeStyle::Rebase => "rebase",
            MergeStyle::Squash => "squash",
        }
    }
}

//! Loupe's git layer: the git CLI runner and its parsers, diffs, three-way merge, conflicts, LFS, submodules, undo,
//! interactive rebase, history, and the repository / worktree state with its file watching. No UI: calls block, and
//! the app runs them on gpui's background executor.

pub mod conflicts;
pub mod diff;
pub mod file_tree;
pub mod git;
pub mod history;
pub mod lfs;
pub mod merge3;
pub mod model;
pub mod rebase;
pub mod submodules;
pub mod undo;
pub mod watch;

pub use git::{Cancel, Error, FileChange, Kind, Result, Worktree};

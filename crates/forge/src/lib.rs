//! Forge API clients (Forgejo, Gitea, GitHub), pull requests, CI, accounts + Keychain, commit AI and the persisted
//! settings model. No UI, no async runtime: every network or process call blocks, so call it from a background
//! thread (gpui's `background_spawn`). Work a view no longer needs is stopped with a [`Cancel`]; a cancelled call
//! returns [`Error::Cancelled`], whose [`Error::user_message`] is `None` — a stopped request is never shown as a
//! failure and never replaces what a view already shows.
//!
//! Updates: the Swift app used Sparkle 2 (`Updates.swift`). It ran only inside a bundled Loupe.app whose Info.plist
//! had an https `SUFeedURL` (the GitHub release appcast) and a non-empty `SUPublicEDKey`; otherwise the "Check for
//! Updates…" menu item and the Settings › General "Updates" section (automatically check / automatically download)
//! were hidden. Not ported: the GPUI app needs its own updater.

pub mod account;
pub mod ai;
pub mod avatar;
pub mod ci;
pub mod client;
pub mod git;
pub mod models;
pub mod pull;
pub mod pull_create;
pub mod settings;

pub use account::{Account, Keychain, RemoteRepo};
pub use client::Client;
pub use models::*;

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The caller stopped wanting the result. Not a failure: never show it.
    Cancelled,
    /// A non-2xx answer. Displays as "GitHub 403: message", the form error explanations match on.
    Http { server: String, status: u16, message: String },
    /// GitHub won't serve a diff this large (over 300 files or 20k lines); diff locally instead.
    DiffTooLarge,
    /// Creating a pull request for a branch that already has an open one.
    PullExists,
    /// Network, decoding, git or process failure, as a message.
    Other(String),
}

impl Error {
    /// What to show for a failure, or `None` for a cancellation.
    pub fn user_message(&self) -> Option<String> {
        match self {
            Error::Cancelled => None,
            e => Some(e.to_string()),
        }
    }
    pub(crate) fn msg(s: impl Into<String>) -> Self {
        Error::Other(s.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Cancelled => write!(f, "cancelled"),
            Error::Http { server, status, message } => write!(f, "{server} {status}: {message}"),
            Error::DiffTooLarge => write!(f, "The server won't serve a diff this large."),
            Error::PullExists => write!(f, "A pull request for this branch already exists."),
            Error::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// A cancellation flag shared between a view and the work it started. Clone it into the background task; call
/// [`Cancel::cancel`] when the view moves on. Sleeps through [`Cancel::sleep`] wake up at once on cancel.
#[derive(Clone, Default, Debug)]
pub struct Cancel(Arc<(Mutex<bool>, Condvar)>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
    pub fn is_cancelled(&self) -> bool {
        *self.0.0.lock().unwrap()
    }
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() { Err(Error::Cancelled) } else { Ok(()) }
    }
    /// Sleeps `d` unless cancelled first. Returns false when cancelled.
    pub fn sleep(&self, d: Duration) -> bool {
        let (m, c) = &*self.0;
        let g = m.lock().unwrap();
        let (g, _) = c.wait_timeout_while(g, d, |cancelled| !*cancelled).unwrap();
        !*g
    }
}

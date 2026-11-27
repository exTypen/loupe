//! Loupe's GPUI front end. Shared: theme, ui, diff, state, shell (frozen for the view agents); views/: one module
//! per view owner (docs/gpui/VIEWS.md).
pub mod theme;
pub mod ui;
pub mod diff;
pub mod state;
pub mod shell;
pub mod views;
#[cfg(target_os = "macos")]
pub mod updates;

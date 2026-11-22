//! Loupe's palette: near-black canvas, a slightly lighter sidebar,
//! bordered rounded cards, muted grey text and small, sparing accents. Frozen during the parallel port: ask the
//! integrator for a new token instead of hard-coding a color in a view.
use gpui_kit::{Hsla, Pixels, px, rgb, rgba};

fn c(v: u32) -> Hsla { rgb(v).into() }
fn ca(v: u32) -> Hsla { rgba(v).into() }

// Surfaces, darkest to lightest.
pub fn canvas() -> Hsla { c(0x141414) }          // main content background
pub fn sidebar() -> Hsla { c(0x1a1a1a) }         // sidebar and title bar
pub fn card() -> Hsla { c(0x181818) }            // cards, code blocks, diff body
pub fn card_header() -> Hsla { c(0x1d1d1d) }     // card title strips, sticky file headers
pub fn raised() -> Hsla { c(0x232323) }          // inputs, pills, popovers, menus
pub fn hover() -> Hsla { ca(0xffffff0d) }        // row hover (white 5%)
pub fn selected() -> Hsla { c(0x2a2a2a) }        // selected row (sidebar "Commit ve push" row)
pub fn border() -> Hsla { c(0x2b2b2b) }          // 1px card and divider lines
pub fn border_strong() -> Hsla { c(0x3a3a3a) }   // focused input, pill outline

// Text.
pub fn text() -> Hsla { c(0xe6e6e6) }            // primary
pub fn text_muted() -> Hsla { c(0x9a9a9a) }      // secondary: paths, counts, timestamps
pub fn text_faint() -> Hsla { c(0x666666) }      // tertiary: line numbers, shortcuts (⌘1), placeholders

// Accents. Use one per meaning; never decorate with them.
pub fn accent() -> Hsla { c(0xe5a50a) }          // amber: bullets, the one primary highlight, warnings
pub fn blue() -> Hsla { c(0x3b82f6) }            // unread / active dot, links, focus
pub fn green() -> Hsla { c(0x3fb950) }           // success, "Done", added
pub fn red() -> Hsla { c(0xf85149) }             // failure, removed, destructive
pub fn purple() -> Hsla { c(0xa371f7) }          // merged

// Diff tints over card().
pub fn added_bg() -> Hsla { ca(0x2ea04326) }
pub fn added_word() -> Hsla { ca(0x2ea04366) }
pub fn removed_bg() -> Hsla { ca(0xf8514926) }
pub fn removed_word() -> Hsla { ca(0xf8514966) }
pub fn hunk_bg() -> Hsla { ca(0x3b82f61a) }

// Shape and type.
pub const RADIUS_CARD: Pixels = px(10.);
pub const RADIUS_ROW: Pixels = px(8.);
pub const RADIUS_PILL: Pixels = px(999.);
pub const UI_FONT: &str = ".SystemUIFont";
pub const CODE_FONT: &str = "Menlo";
pub const UI_SIZE: Pixels = px(13.);
pub const CODE_SIZE: Pixels = px(12.);
pub const ROW_HEIGHT: Pixels = px(28.);          // sidebar and list rows
pub const CODE_LINE_HEIGHT: Pixels = px(18.);

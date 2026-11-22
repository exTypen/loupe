# Porting Loupe from SwiftUI/AppKit to GPUI

extypen wasn't happy with the Swift app's performance, so Loupe is moving to GPUI. This file is the brief every
agent on the port works from.

## The goal

- **UX and behaviour stay the same.** Every feature, shortcut, flow, piece of state and edge case in
  `Sources/Loupe/*.swift` keeps working the same way. The Swift sources are the spec: read the file for your area
  before you write its Rust, and port its behaviour, including the comments that explain why a thing is done a
  particular way. The vault note `/Users/extypen/vaults/extypen-claude/Projects/loupe/index.md` records verified
  API facts, the lessons learned and why things are the way they are. Grep it for your area.
- **The look is the app's own, not macOS's.** See `docs/gpui/design-reference.png`. It's a near-black canvas with a
  slightly lighter sidebar, grouped sidebar sections with counts, rounded rows and a soft selected-row fill,
  bordered rounded cards with a header strip, monospace code blocks, pill buttons, small amber, green and blue
  accents, muted grey secondary text and faint shortcut hints (⌘1). Don't spend time imitating native controls
  (NSToolbar, vibrancy, native menus). Make it look good in that style instead.
- **Faster than the Swift app.** A 150k-line diff opens in under 1 s and scrolls at display rate. Idle CPU is
  effectively zero: nothing animates or polls while nothing changes. Work a view no longer shows gets cancelled.
  The last five Swift commits (`git log -5 main`) were all about this, so keep their rules.

## Stack

- `gpui-kit = 0.10.0` is one dependency that re-exports gpui (`gpui_kit::div`, `gpui_kit::Context`, …) plus
  gpui-component (`gpui_kit::component::…`: sidebar, list, table, input/editor, dialog, notification, tab,
  resizable, popover, menu, `diff` — a virtualized patch viewer). Read the crate source under
  `~/.cargo/registry/src/*/gpui-component-0.10.0/src` and `gpui-pre-0.3.12/src`. The docs are at
  https://gpui-kit.com. Use these components wherever they fit, and restyle them with `theme.rs` colors.
- The toolchain is pinned to Rust 1.100 in `rust-toolchain.toml`, because gpui needs ≥1.95.
- Build with `cargo build`, test with `cargo test -p <crate>`, run with `cargo run -p loupe`.

## Layout and ownership

```
crates/core   loupe-core   git CLI runner + parsers, diff, merge3, conflicts, LFS, submodules, undo,
                           interactive rebase, worktree/status/log models, FS watching. No gpui.
crates/code   loupe-code   syntax highlighting, code index, tag queries, SQL schema, YAML, change lens,
                           file icons data. No gpui.
crates/forge  loupe-forge  Forgejo/Gitea/GitHub API, PRs, reviews, CI, PR create, accounts + Keychain,
                           commit AI. No gpui.
app           loupe        GPUI front end. theme.rs (palette, frozen), ui/ (shared widgets), shell/ (window,
                           sidebar, tabs), diff/ (diff renderer), and one module per view later.
```

You own only the files your task names. If you need something from a file you don't own, write your own
local version, or note it in your final report as a request. Never edit someone else's file. `theme.rs` is
frozen: if you need another color, ask for it in your report and use the closest existing token until then.

## Rules

- **Git** goes through the `git` CLI, as in `Git.swift`: `GIT_OPTIONAL_LOCKS=0`; read-only commands can be
  cancelled; stderr is drained concurrently and waited on with a timeout (fsmonitor keeps the pipe open). Hooks,
  signing and credential helpers have to keep working.
- **Background work** runs on gpui's background executor (`cx.background_spawn`) or plain threads in the core
  crates. No tokio unless a crate forces it. HTTP uses blocking `ureq` on the background, unless the forge agent
  finds something better and says why.
- **Persisted state stays compatible.** The Keychain service names, account storage and settings keys are the
  ones the Swift app used, so an existing install keeps its accounts and settings. Settings migrate from
  `~/Library/Preferences/dev.extypen.loupe.plist` (the `plist` crate).
- **Tests.** Port the Swift tests for your area from `Tests/LoupeTests` as Rust tests. Live/network tests are
  `#[ignore]`.
- **Commits.** Work in your own git worktree and branch (your task names them). Commit there with a message
  that says what now works. No `Co-Authored-By` trailers. **Never push.** Never touch `main` or the `~/loupe`
  checkout.
- No speculative abstraction. Small, direct, readable Rust. Comments explain why, not what.

## Report

Your final message must cover: what works, verified how (tests and run output), what's missing compared with
the Swift original, the public API other agents will use (types and functions), and requests for files you
don't own.

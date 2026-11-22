# Loupe user-flow catalog (Swift reference for the GPUI port)

Every place a user can act in the Swift app (`/Users/extypen/loupe/Sources/Loupe`, at commit fde8251), with what it does. Testers drive the Swift app and the GPUI app side by side against the mock backend, flow by flow, and compare render and reaction. The Swift code is the spec; where the vault note disagrees, the code wins and the disagreement is listed in "Known issues".

Conventions:
- `Source:` paths are relative to `/Users/extypen/loupe/Sources/Loupe/`. "vault" means `/Users/extypen/vaults/extypen-claude/Projects/loupe/index.md`.
- Every flow has the same fields: Trigger, Preconditions, Render, Reaction, Edge cases, Source.
- IDs are stable. Add new flows at the end of a prefix and never renumber.
- Flows marked PLAUSIBLE in the known-issues list were traced from code but not confirmed in the running app.

Features testers may expect that the Swift app does NOT have (so there are no flows for them): split/side-by-side diff, a whitespace-ignore toggle, context expansion other than Full mode, a "show anyway" gate for large diffs, image diffs for non-LFS images, reset soft/mixed/hard, commit checkout, creating a branch or tag from a commit, git redo, file/folder History scopes, a file quick-open (⌘K switches worktrees), reply/resolve/edit/delete of server review comments, suggestions, closing/reopening a PR, delete-branch-on-merge, syncing viewed state to the forge, CI status dots in the PR list, ANSI colours and find in CI logs, and clickable file:line problem lines.

## Counts

| Prefix | Area | Flows |
|---|---|---|
| SH | App shell, sidebar, spaces, toolbar | 39 |
| KB | Menu and shortcut map | 22 |
| RW | Repos and worktrees | 9 |
| CH | Changes: toolbar, status list, staging, discard | 42 |
| CM | Commit, amend, commit AI | 13 |
| SS | Stash | 10 |
| FI | Blame / File History panel | 6 |
| DV | Diff viewer | 45 |
| LF | LFS | 9 |
| SM | Submodules | 7 |
| CO | Change overview | 22 |
| LN | Overview lenses | 29 |
| CB | Code browser, go-to-definition, outline | 18 |
| QS | Quick switcher | 7 |
| FT | File tree component | 9 |
| HI | History | 22 |
| RB | Interactive rebase | 13 |
| UN | Undo | 8 |
| MG | Merge editor and conflicts | 26 |
| PR | Pull Requests list | 22 |
| PD | PR detail | 33 |
| RV | Conversation and review | 17 |
| FG | Forge API behaviours | 11 |
| PC | PR create | 20 |
| CI | CI | 39 |
| ST | Settings and accounts | 20 |
| UP | Updates | 4 |
| MS | Test hooks and misc | 16 |
| | **Total** | **538** |

# 1. App shell, sidebar, spaces, toolbar, menus and shortcuts

Paths are relative to /Users/extypen/loupe/Sources/Loupe/. The vault note is /Users/extypen/vaults/extypen-claude/Projects/loupe/index.md.

**Corrections to the vault note:**
- It says "Switch Branch" is in the worktree context menu. In the current code it is only in the toolbar's branch popover.
- It says History's first page is 300 commits. The latest commit changed that to 100.

## App launch and window

### SH-01 App launch and main window
- **Trigger:** Launch Loupe (Dock, Finder, `open`, or the bare SwiftPM binary).
- **Preconditions:** None. For the restored state, UserDefaults holds `spaces`, `currentSpace`, `recent` and `collapsed`.
- **Render:**
  - One window titled "Loupe" (WindowGroup id "main"), 1440×900 by default, minimum 860×560, with a unified toolbar.
  - Window restoration is off, so the window always opens at the default frame.
  - The sidebar column is 200–360 pt wide (ideal 250); the detail is on the right.
- **Reaction:**
  - `applicationDidFinishLaunching` sets URLCache to 32 MB memory and 128 MB disk, applies the saved Appearance and starts Sparkle (`Updates.shared`).
  - It then calls `setActivationPolicy(.regular)` and `NSApp.activate()`.
  - `AppModel.init` sets `selection` from the current space's saved selection, then runs `refreshAllWorktrees()` (`git worktree list --porcelain` per repo across all spaces, then status per worktree) and `loadTokens()` (Keychain read off the main thread).
- **Edge cases:**
  - If 0.3 s after launch no window is visible: an existing window is ordered front regardless. If SwiftUI never created a window, Loupe sends itself a kAEReopenApplication Apple Event so one opens (the fix for launching while another app is frontmost, or `open -g`).
  - Legacy defaults `repos` and `selection` migrate into a "Default" space (id `default`) when `spaces` is absent.
  - Tokens load once at launch. Until then the account rows show no "No token" warning (`tokensLoaded` gates it).
  - Vault: reading the Keychain on the main thread used to freeze launch for 4–9 s; tokens are now read once off-main.
- **Source:** App.swift:64-75, App.swift:110-131, Model.swift:10-17, Model.swift:55-68

### SH-02 Reactivating with no visible window
- **Trigger:** Click the Dock icon or Cmd-Tab to Loupe while its window is hidden or un-ordered.
- **Preconditions:** The app is running and no window is visible.
- **Render:** The main window comes to the front.
- **Reaction:** `applicationDidBecomeActive` calls `makeKeyAndOrderFront` on the first window that can become main.
- **Edge cases:** Does nothing if a window is already visible.
- **Source:** App.swift:133-137

### SH-03 Closing the window quits the app
- **Trigger:** The window's close button, or File › Close (⌘W, system item).
- **Preconditions:** None.
- **Render:** The app quits.
- **Reaction:** `applicationShouldTerminateAfterLastWindowClosed` returns true.
- **Edge cases:**
  - Vault (0.9.1): quitting no longer hangs. A daemon git spawns (fsmonitor) inherits the stderr pipe and never closes it, so after git exits Loupe now waits at most 2 s for stderr.
  - Before the fix, about 90 threads parked. Testers should check that quit is immediate with `core.fsmonitor=true` repos.
- **Source:** App.swift:138, Git.swift:58-67

### SH-04 Empty state (no worktree selected)
- **Trigger:** Launch with no repos, or `selection == nil` (for example an empty space).
- **Preconditions:** No selection.
- **Render:** ContentUnavailableView with:
  - Label "No Worktree Selected", icon `arrow.triangle.branch`
  - Description "Open a repository to review its changes and pull requests."
  - A prominent "Open Repository…" button
- **Reaction:** The button runs `openRepository` (see RW-01).
- **Edge cases:** Switching to an empty space shows this state.
- **Source:** App.swift:190-202

### SH-05 Detail view per selection, and the window title
- **Trigger:** Select a worktree row.
- **Preconditions:** A repo is open.
- **Render:**
  - `WorktreeView(model:)` with `.id(sel)`, so it is rebuilt for each selection.
  - The tab content follows `@AppStorage("tab")`: "changes" (the default), "history" or "pulls".
  - The navigation title is the branch name, "detached HEAD", or empty while `headOID` is empty. The subtitle is the ~-abbreviated path.
  - The system title is removed and drawn by the branch button instead.
- **Reaction:**
  - `AppModel.selection.didSet` releases the old model's heavy state: diff, commit detail, browser, PR details (`releaseHeavyState`).
  - It restores the new model's diff, stores the selection in the current space and pushes the path to the front of `recent` (at most 30).
- **Edge cases:**
  - Selecting a worktree whose repo is in another space (⌘K, PR checkout) switches `currentSpace` to that space; the sidebar slides there.
  - Setting `selection = nil` clears the space's saved selection.
- **Source:** App.swift:190-192, ChangesView.swift:3-25, Model.swift:25-37, Model.swift:638-647

### SH-06 Window-wide error alert
- **Trigger:** Any `app.error` assignment: adding a non-repo, a failed worktree remove, and so on.
- **Preconditions:** None.
- **Render:** An alert titled "Error" with the message text and an "OK" button.
- **Reaction:** OK, or dismissing, sets `app.error = nil`.
- **Edge cases:**
  - Cancellation errors (CancellationError, URLError.cancelled) never show (`userMessage` returns nil, so the previous error is kept).
  - Per-worktree git errors use a separate alert titled "Git" (see SH-25).
- **Source:** App.swift:179-181, Git.swift:3-9

### SH-07 Checks overlay host
- **Trigger:** Set by the CI and PR area (`app.checksOverlay`).
- **Preconditions:** A PR with checks.
- **Render:** A window-wide `ChecksOverlay` over the split view.
- **Reaction:** Closing it animates `checksOverlay = nil` over 0.15 s. See the CI area.
- **Source:** App.swift:170-174, Model.swift:50

### SH-08 Pull request create sheet host
- **Trigger:** Worktree context menu › "Create Pull Request…" (SH-20), or the PR area.
- **Preconditions:** The worktree has a branch and a forge client (account plus token).
- **Render:** `PullCreateSheet` for `app.model(for: path)`.
- **Reaction:** Dismissing sets `creatingPull = nil`. See the CI/PR create area.
- **Source:** App.swift:176-178, App.swift:371-374

### SH-09 Quick switcher sheet host (⌘K)
- **Trigger:** View › Go to Worktree… (⌘K).
- **Preconditions:** None.
- **Render:** The `QuickSwitcher` sheet.
- **Reaction:** Sets `app.showSwitcher = true`. Picking a worktree sets `selection`, which may switch the space (SH-05). See the quick switcher area.
- **Edge cases:** Vault: the list is ordered by `recent` (MRU) and the previous worktree is preselected, so ⌘K then ↩ toggles back.
- **Source:** App.swift:92, App.swift:175, Model.swift:35-44

## Sidebar

### SH-10 Sidebar layout
- **Trigger:** Always shown, unless the sidebar is toggled off (KB-04).
- **Preconditions:** At least one repo.
- **Render:**
  - With more than one space, a bold title3 space name sits at the top of the page.
  - Then a `.sidebar` List with one Section per repo in space order. The header is a chevron (rotated 90° when expanded) plus the repo folder name, semibold and middle-truncated.
  - Worktree rows are listed main first, in git's order.
  - A bottom bar on a `.bar` background with a top divider holds: "plus.rectangle.on.folder" (help "Open Repository…"), the space dots and "plus" (help "New Space").
- **Reaction:** See SH-11 to SH-21.
- **Edge cases:**
  - With one space there is no space title; the dot row still shows a single dot.
  - Bare worktrees are filtered out.
- **Source:** App.swift:216-258, App.swift:288-358, Model.swift:168

### SH-11 Worktree row rendering
- **Trigger:** Any worktree in an expanded repo.
- **Preconditions:** Various worktree states (see FIXTURES).
- **Render:**
  - Icon: `folder` (secondary) for the main worktree; `arrow.triangle.merge` (purple) when merged; otherwise `arrow.triangle.branch` (accent).
  - Title: "Root" for the main worktree. Otherwise the folder name with the "<repo>-", "<repo>_" or "<repo>." prefix stripped ("web-qa" under "web" shows "qa"), as long as something is left.
  - Subtitle (caption, secondary): the branch, or "detached @ <7-char sha>".
  - Trailing, in order: a purple "Merged" capsule (never on the main worktree); "↑N" if ahead; "↓N" if behind; a quaternary capsule with the count of distinct changed paths; `lock.fill` if locked.
  - The counts keep their width; the title and branch truncate instead.
  - Tooltip: "<path>\n<branch>", or the path alone.
- **Reaction:** Clicking selects the row (List selection is bound to `app.selection`; tag is `wt.path`).
- **Edge cases:**
  - Merged is decided by `Git.isMerged` against the first of `<remote>/HEAD`, `<remote>/main`, `<remote>/master`, `main`, `master` that resolves.
  - The check is skipped when the branch is the target itself or `<remote>/<branch>`, and runs only when HEAD or the target oid moves.
  - With commits ahead, merged means `merge-tree --write-tree` equals the target tree (squash or rebase).
  - With none ahead, merged means the reflog has a "commit" entry, so a freshly cut branch is not merged.
  - Known ceiling (ponytail comment): a PR checked out and merged with a merge commit but never committed on locally reads as not merged.
  - The change count is `Set(files.map(\.path)).count`, so a file both staged and unstaged counts once.
- **Source:** App.swift:569-613, Model.swift:230-244, Model.swift:324-336, Git.swift:184-206

### SH-12 Collapse and expand a repo section
- **Trigger:** Click the repo header (the chevron and name button).
- **Preconditions:** A repo with worktrees.
- **Render:** The chevron rotates over 0.15 s easeOut and the rows hide. A collapsed header shows a quaternary capsule with the summed change count of all its worktrees, only if greater than 0.
- **Reaction:** Inserts the repo path into, or removes it from, `app.collapsed`, which persists as the UserDefaults array `collapsed`.
- **Edge cases:** Collapse state survives relaunch and applies in every space.
- **Source:** App.swift:298-321, Model.swift:41-43

### SH-13 Header "+" (New Worktree)
- **Trigger:** The "plus" button on a repo header (help "New Worktree in <repo>…").
- **Preconditions:** A repo.
- **Render:** Opens the AddWorktreeSheet (RW-05).
- **Reaction:** `addingWorktreeFor = repo`.
- **Source:** App.swift:322-324, App.swift:259-261

### SH-14 Drag a repo header to reorder
- **Trigger:** Drag a repo header onto another repo header in the same space.
- **Preconditions:** At least two repos in the current space.
- **Render:**
  - The drag preview is the repo name on a regularMaterial rounded rectangle.
  - While hovering a target header, a 2 pt accent capsule line shows 4 pt above it.
- **Reaction:** `moveRepo(r, before: target)` with a snappy animation; the order persists in `spaces`.
- **Edge cases:**
  - The drop is rejected if the dragged string is not a repo of this page's space.
  - Dropping onto itself does nothing.
  - There is no "drop after the last repo" target. A repo can only be moved before another, so to make it last, drag the last repo up instead.
- **Source:** App.swift:328-339, Model.swift:113-120

### SH-15 Drag a repo header onto a space dot
- **Trigger:** Drag a repo header onto a dot in the bottom bar.
- **Preconditions:** A repo in the current space. More than one space is useful.
- **Render:** The repo disappears from the current list.
- **Reaction:** `moveRepos([repo], to: spaceID)`, written in one pass so it persists once.
- **Edge cases:**
  - Only repos in the current space are accepted (`items.filter(app.repoPaths.contains)`).
  - Dropping on the current space's own dot moves the repo to the end of its list.
  - If the selected worktree belonged to the moved repo and the target is not the current space, the selection jumps to the current space's first repo, or nil.
- **Source:** App.swift:243-248, Model.swift:105-111

### SH-16 Repo header context menu
- **Trigger:** Right-click a repo header.
- **Preconditions:** A repo.
- **Render:** "New Worktree…", "Reveal in Finder", "Prune Stale Worktrees", "Move to Space ▸" (only when more than one space; lists the other spaces by name), a divider, "Remove from Loupe".
- **Reaction:**
  - New Worktree… opens RW-05.
  - Reveal in Finder selects the repo folder in Finder.
  - Prune Stale Worktrees runs `git worktree prune` in the repo, then `refreshWorktrees`. Errors are swallowed with `try?`.
  - Move to Space › X runs `moveRepos([repo], to: X)`.
  - Remove from Loupe is RW-03.
- **Edge cases:** Remove from Loupe has no confirmation.
- **Source:** App.swift:340-353

### SH-17 Worktree row context menu: open and copy
- **Trigger:** Right-click a worktree row.
- **Preconditions:** A worktree.
- **Render:** "Reveal in Finder", "Open in Terminal", "Open in Zed", "Open in VS Code", "Copy Path".
- **Reaction:**
  - Reveal in Finder uses `activateFileViewerSelecting`.
  - The three Open items open the folder with bundle ids `com.apple.Terminal`, `dev.zed.Zed` and `com.microsoft.VSCode`.
  - Copy Path clears the pasteboard and copies the absolute path as a string.
- **Edge cases:** If an app is not installed, `NSSound.beep()` plays and nothing opens.
- **Source:** App.swift:365-370, App.swift:564-567

### SH-18 Worktree row context menu: Remove Worktree…
- **Trigger:** Right-click a non-main worktree › "Remove Worktree…" (destructive).
- **Preconditions:** A linked worktree.
- **Render:** RW-06's confirmation dialog.
- **Reaction:** `removing = (repo, wt)`.
- **Edge cases:** The item is hidden on the main worktree (`wt.path == repo`).
- **Source:** App.swift:375-378

### SH-19 Hover tooltips in the sidebar
- **Trigger:** Hover the controls below.
- **Render:**
  - Worktree row: "<path>\n<branch>".
  - Header "+": "New Worktree in <repo>…".
  - Space dot: the space name.
  - Bottom bar: "Open Repository…" and "New Space".
- **Reaction:** None (`.help` only; no `onHover` in these files).
- **Source:** App.swift:611, App.swift:324, App.swift:241, App.swift:233, App.swift:252

### SH-20 Worktree context menu: Create Pull Request…
- **Trigger:** Right-click a worktree › "Create Pull Request…".
- **Preconditions:** The worktree has a branch (not detached) and `model.client` is non-nil: the remote is matched to an account in the repo's space and the token is loaded.
- **Render:** A divider, then the item. It is hidden otherwise.
- **Reaction:** `app.creatingPull = wt.path` opens SH-08.
- **Source:** App.swift:371-374, Model.swift:537-542

### SH-21 Selection highlight, and headers that can't be selected
- **Trigger:** Click a row.
- **Render:** Standard sidebar highlight. Headers use `.selectionDisabled()` so the first row's highlight does not wrap the header.
- **Reaction:** SH-05.
- **Source:** App.swift:290, App.swift:327

## Spaces

### SH-22 Space dots: switch by click
- **Trigger:** Click a dot in the bottom bar.
- **Preconditions:** At least two spaces.
- **Render:**
  - The current space's dot is `circle.fill` (primary); the others are `circle` (secondary), 8 pt.
  - The pages slide from the old space to the new one (snappy, 0.28 s). Only the shown space stays in the view tree at rest.
- **Reaction:**
  - `switchSpace(to:)` sets `currentSpace` (persisted under key `currentSpace`) and `selection = space.selection`, so each space remembers its own selection.
  - The detail follows.
- **Edge cases:** Clicking the current dot does nothing (guard `id != currentSpace`).
- **Source:** App.swift:235-242, App.swift:284-286, Model.swift:77-81, App.swift:422-428

### SH-23 Space swipe (two-finger horizontal trackpad)
- **Trigger:** A two-finger horizontal swipe over the sidebar.
- **Preconditions:** At least two spaces and a trackpad.
- **Render:**
  - The pages follow the fingers. Past the first or last space the movement is rubber-banded at 0.35×.
  - On release they spring (critically damped, k=500, at rest in about 0.3 s) to the nearest space. A flick faster than 0.6 pages/s goes to the next space in that direction.
- **Reaction:**
  - Each gesture's first 4 pt are buffered to decide its axis. Horizontal moves the pages; vertical gestures and momentum are replayed to the list (scrolling still works).
  - Mouse-wheel events (no precise deltas) always go to the list.
  - The space switch is committed only at rest (`rest` calls `move(page - index)` with no animation). Touching again catches the pages mid-spring.
- **Edge cases:**
  - Lifting without moving flushes the held events.
  - Index bounds are re-checked through `switchSpace(by:)`.
  - The overlay hit-tests only scroll events; clicks, drags and menus pass through.
  - Vault: a local scrollWheel monitor never got phase `.ended` under the List, and `trackSwipeEvent` crashed on rapid swipes. Committing mid-slide froze the animation.
- **Source:** App.swift:387-430, App.swift:436-560, Model.swift:83-86

### SH-24 New Space (sidebar "+")
- **Trigger:** The bottom bar's "plus" button (help "New Space").
- **Preconditions:** None.
- **Render:** A new empty space named "Space N" (N = space count + 1). It becomes current, so the sidebar shows an empty list and the detail shows SH-04.
- **Reaction:** `addSpace()` appends a space with a UUID id and switches to it (snappy animation).
- **Edge cases:** Names can repeat ("Space 3" twice after a deletion).
- **Source:** App.swift:251-252, Model.swift:88-92

### SH-26 Space context menu: Rename…
- **Trigger:** Right-click the space title (shown only with more than one space) or a space dot › "Rename…".
- **Preconditions:** A space.
- **Render:** An alert titled "Rename Space" with a "Name" text field prefilled with the current name, and "Rename" / "Cancel" buttons.
- **Reaction:** Rename trims whitespace and sets `spaces[i].name`, which persists.
- **Edge cases:** An empty or whitespace-only name is ignored.
- **Source:** App.swift:222-226, App.swift:242, App.swift:266-273, App.swift:279-280

### SH-27 Space context menu: Delete…
- **Trigger:** Right-click the space title or a dot › "Delete…" (destructive).
- **Preconditions:** More than one space (the item is disabled otherwise).
- **Render:** A confirmation dialog titled 'Delete space “<name>”?' with the message "Its repositories move to the neighbouring space. Its accounts are signed out and their tokens removed from the Keychain." Button: "Delete" (destructive).
- **Reaction:**
  - `deleteSpace`: repos move to the previous space (the next one if it was the first).
  - Each of the space's accounts has its Keychain item deleted (`setToken(nil)`) and is removed from `accounts`.
  - If it was the current space, the app switches to the target. The space is then removed.
- **Edge cases:**
  - A guard repeats the more-than-one-space check.
  - Deleting a non-current space keeps the current view.
- **Source:** App.swift:274-276, App.swift:281, Model.swift:94-103

### SH-28 Jump across spaces by selection
- **Trigger:** ⌘K to a worktree in another space, opening a repo already in a different space (RW-01), or a PR checkout.
- **Preconditions:** At least two spaces.
- **Render:** The sidebar slides to the owning space.
- **Reaction:** `selection.didSet` finds the repo's space and sets `currentSpace`.
- **Source:** Model.swift:31-34

## Toolbar and tabs

### SH-25 Per-worktree git error alert
- **Trigger:** `model.error` is set by refreshStatus, perform(), remote operations and similar.
- **Render:** An alert titled "Git" with git's stderr (trimmed) or "git <sub> failed (<code>)", and an "OK" button.
- **Reaction:** OK clears `model.error`.
- **Source:** ChangesView.swift:97-99, Git.swift:72-74, Model.swift:412-417

### SH-29 Toolbar: Fetch
- **Trigger:** The toolbar button "Fetch" (`arrow.triangle.2.circlepath`, help "Fetch all remotes").
- **Preconditions:** A worktree with remotes. For HTTPS remotes, an account in the repo's space for authentication.
- **Render:** The icon rotates while `busy == "Fetching"`. All three remote buttons are disabled while any operation runs.
- **Reaction:**
  - Runs `git fetch --all --prune` with the `gitAuth` env: `GIT_CONFIG_COUNT=2`, an empty `credential.<baseURL>.helper` followed by an inline helper that echoes `LOUPE_GIT_USER` / `LOUPE_GIT_TOKEN`.
  - Then refreshes status, bumps `refsVersion` and refreshes the repo's worktrees.
- **Edge cases:**
  - Errors go to the "Git" alert.
  - SSH remotes ignore `gitAuth`.
  - `GIT_TERMINAL_PROMPT=0`, so git never prompts.
- **Source:** ChangesView.swift:30-34, Model.swift:503-509, Model.swift:547-554

### SH-30 Toolbar: Pull
- **Trigger:** "Pull" (`arrow.down`). Help is "Pull N commits (fast-forward only)" when behind, otherwise "Pull (fast-forward only)".
- **Preconditions:** An upstream. Behind or not.
- **Render:** The icon pulses while pulling; the button is disabled while busy.
- **Reaction:** Runs `git pull --ff-only` (gitAuth) under an undo recipe `.reset("Pull", published: false)`, so ⌘Z can undo the pull and a toast appears. See the History/undo area.
- **Edge cases:** Diverged branches fail with git's error in the "Git" alert.
- **Source:** ChangesView.swift:35-39, Model.swift:503-506

### SH-31 Toolbar: Push
- **Trigger:** "Push" (`arrow.up`). Help is "Push N commits" when ahead, otherwise "Push".
- **Preconditions:** A remote.
- **Render:** The icon pulses while pushing; the button is disabled while busy.
- **Reaction:** Runs `git push --set-upstream <remoteName> HEAD` (gitAuth), then posts `.branchPushed` with the path (PullCreate listens).
- **Edge cases:** `remoteName` is the remote that has an account, else "origin", else the first one (picked once per model).
- **Source:** ChangesView.swift:40-44, Model.swift:523-535

### SH-32 Toolbar: branch button and popover
- **Trigger:** Click the branch title in the toolbar (help "<branch>\n<~path>\nClick to switch branch").
- **Render:** Two lines: the branch (headline, with a chevron.down) and the ~path (caption), capped at 300 pt and truncated. The popover shows `BranchPicker`.
- **Reaction:** See the Changes or History area for BranchPicker; per the vault, picking `origin/x` runs `git switch x`.
- **Source:** ChangesView.swift:46-62

### SH-33 Toolbar: view picker (Changes / History / Pull Requests)
- **Trigger:** The segmented picker in the principal position, or ⌘1/⌘2/⌘3 (KB-05 to KB-07).
- **Render:** Three title-only segments: "Changes", "History", "Pull Requests".
- **Reaction:** Writes `@AppStorage("tab")` to "changes", "history" or "pulls". This is global, so every worktree shows the same tab.
- **Edge cases:**
  - Latest commits: History reloads only when HEAD or remote refs moved. The PR list and an open PR reload only after 30 s. The switch itself costs no git processes.
  - An unknown tab value falls back to Changes.
- **Source:** ChangesView.swift:16-20, ChangesView.swift:63-71

### SH-34 Toolbar: Compact / Full picker
- **Trigger:** The segmented picker shown only while a diff is on screen (`controls.full`).
- **Render:**
  - Compact segment: `rectangle.compress.vertical`, help "Compact: the changes with a few lines around them".
  - Full segment: `rectangle.expand.vertical`, help "Full: whole files with the changes inline (⌥⌘F)".
- **Reaction:** Toggles the UserDefaults key `prFullMode`, shared by Changes, History and PRs. See the diff viewer area.
- **Source:** ChangesView.swift:73-81

### SH-35 Toolbar: Overview / Files / Conversation picker
- **Trigger:** The segmented picker, present when a child view publishes `DiffControls.view`.
- **Render:** Image-only segments: `square.grid.2x2` (Overview), `doc.text` (Files), `bubble.left.and.bubble.right` (Conversation, PR only).
- **Reaction:** Area-specific; see the History and PR areas. The stress driver drives this control (MS-12).
- **Source:** ChangesView.swift:82-95

### SH-36 Undo toast host
- **Trigger:** A destructive action, or an undo.
- **Render:** A bottom capsule toast ("Discard of a.txt · Undo" or "Undone: …"), gone after 6 s.
- **Reaction:** See the History/undo area.
- **Source:** ChangesView.swift:22, Undo.swift:375-406

## File watching and refresh

### SH-37 Live refresh from file-system events
- **Trigger:** Any file change in a worktree or its git dir (editor saves, terminal git commands).
- **Preconditions:** A worktree model exists. It is created for every listed worktree at refresh.
- **Render:** Badges, file lists and diffs update about 0.4 s later: FSEvents latency 0.15 s plus a 250 ms debounce.
- **Reaction:**
  - The watcher covers the worktree plus its absolute git dir when that lies outside it (a linked worktree's `.git/worktrees/<name>`).
  - On relevant events it runs `refreshStatus`:
    - `git status --porcelain=v2 -z --branch --untracked-files=all`
    - `git stash list --format=%H`
    - operation detection (rebase-merge/apply, CHERRY_PICK_HEAD, REVERT_HEAD, MERGE_HEAD, rebase-merge/amend)
    - LFS and submodule refresh, diff reload, merged check
- **Edge cases:** These do not refresh the worktree (latest commit; vault: on fleetdriver, 8.4 s of CPU and 160 git processes over 3 idle minutes became 0.4 s and none):
  - noise: `*.lock`, `/.git/objects/`, `/.git/lfs/`, and `/.git/modules/*/objects|lfs`
  - paths under another worktree nested in this one
  - paths under `<gitdir>/worktrees/`
  - paths git ignores (one `git check-ignore -z --stdin` per batch; an ignored folder holding no tracked file is cached, so later events need no git)

  Further cases:
  - A change to `/.gitignore` or `/info/exclude` clears the cache and refreshes.
  - If `check-ignore` fails, the worktree refreshes (fail-open).
  - The diff table is not reset when the diff text is unchanged, so scroll and selection are kept.
  - The selected file survives moving between staged and unstaged.
- **Source:** Model.swift:298-321, Model.swift:573-633, Model.swift:649-656, Model.swift:660-706

### SH-38 Refresh (⌘R)
- **Trigger:** Review › Refresh (⌘R).
- **Reaction:** Posts `.refresh`. WorktreeView refreshes status; History reloads; the PR list or an open PR reloads.
- **Edge cases:** Does not re-list worktrees. Use the header menu's Prune, or wait for a worktree event.
- **Source:** App.swift:100, ChangesView.swift:100, History.swift:337, PullRequestsView.swift:134, PullRequestsView.swift:303

### SH-39 Merging a PR shows the merge in History
- **Trigger:** A PR merge in the PR area.
- **Reaction:** `showMerged(into:)` runs `git fetch <remote> <base>` and sets tab = "history" with scope `branch(<remote>/<base>)`. The reload selects the top commit, the merge. See the PR area.
- **Source:** Model.swift:511-519

## Menu and shortcut map

Every customizable action's default is in Shortcuts.swift:90-112. All `.shortcut()` bindings read user overrides live; the menus rebuild when `revision` changes (App.swift:65).

### KB-01 Loupe › Check for Updates…
- **Trigger:** The app menu, after "About Loupe".
- **Preconditions:** A bundled .app with an https `SUFeedURL` and a non-empty `SUPublicEDKey`.
- **Render:** Hidden when Sparkle is not configured (`swift run`, or a build without a key). Disabled while `!canCheck`.
- **Reaction:** UP-02.
- **Source:** App.swift:77, Updates.swift:43-50

### KB-02 Loupe › Settings… (⌘,)
- **Trigger:** The system Settings item.
- **Render:** The Settings window (ST-01).
- **Edge cases:** Vault: synthetic ⌘, keystrokes via System Events don't open it; click the menu item instead.
- **Source:** App.swift:104-106

### KB-03 File › Open Repository… (⌘O, customizable)
- **Trigger:** The File menu. It replaces the New Window item, so there is no ⌘N.
- **Render:** Always enabled.
- **Reaction:** RW-01.
- **Source:** App.swift:78-80, Shortcuts.swift:92

### KB-04 View › Toggle Sidebar (⇧⌘S, customizable)
- **Trigger:** The View menu, after the system sidebar group.
- **Render:** Always enabled.
- **Reaction:** `NSApp.sendAction(toggleSidebar:)` through NSSplitViewController, the same path as the split view's own sidebar button.
- **Edge cases:** Vault: an open stutter with a heavy commit diff, about 100 animation hitches for 6 toggles.
- **Source:** App.swift:82-85, Shortcuts.swift:94

### KB-05 View › Changes (⌘1)
### KB-06 View › History (⌘2)
### KB-07 View › Pull Requests (⌘3)
- **Trigger:** The View menu; all three are customizable.
- **Render:** Always enabled.
- **Reaction:** Writes UserDefaults "tab" to "changes", "history" or "pulls"; WorktreeView follows (SH-33).
- **Edge cases:** With no selection they still write the key, which takes effect on the next selection.
- **Source:** App.swift:88-90, Shortcuts.swift:95-97

### KB-08 View › Go to Worktree… (⌘K, customizable)
- **Trigger:** The View menu.
- **Render:** Always enabled.
- **Reaction:** SH-09.
- **Source:** App.swift:92, Shortcuts.swift:93

### KB-09 Review › Next File (⌥↓, customizable)
### KB-10 Review › Previous File (⌥↑, customizable)
- **Trigger:** The Review menu.
- **Render:** Always enabled.
- **Reaction:** Posts `.moveFile` with +1 or -1.
  - Changes: moves the selection through `listed` (conflicted, then staged, then unstaged), clamped at the ends.
  - History: `model.move(d)`.
  - An open PR: `pull.move(d)`.
- **Edge cases:** Nothing happens with no listener or no selected file.
- **Source:** App.swift:95-96, ChangesView.swift:306-310, History.swift:336, PullRequestsView.swift:304

### KB-11 Review › Mark Viewed & Next (⇧⌘V, customizable)
- **Trigger:** The Review menu.
- **Render:** Always enabled.
- **Reaction:** Posts `.markViewed`; only an open PR listens (`markViewedAndAdvance`).
- **Edge cases:**
  - Also posted by a bare "v" in the diff table (DiffTableView.swift:879).
  - Vault: the old bare-`v` menu shortcut fired while typing, which is why the menu uses ⇧⌘V.
- **Source:** App.swift:97, PullRequestsView.swift:305

### KB-12 Review › Compact / Full Mode (⌥⌘F, customizable)
- **Trigger:** The Review menu.
- **Render:** Always enabled.
- **Reaction:** Toggles the bool `prFullMode`.
- **Edge cases:** It flips even when no diff is shown and the toolbar picker is hidden.
- **Source:** App.swift:98, Shortcuts.swift:104

### KB-13 Review › Refresh (⌘R, customizable)
- **Reaction:** SH-38.
- **Source:** App.swift:100, Shortcuts.swift:98

### KB-14 Edit › Undo (⌘Z, fixed)
- **Trigger:** The Edit menu. It replaces the system Undo/Redo group.
- **Render:** The title is the selected worktree's `UndoStack.title`, or "Undo".
- **Reaction:**
  - If a text field is first responder, the text undo runs.
  - Otherwise, if the stack is empty or nothing is selected, a beep plays.
  - Otherwise `model.undoLast()`. See the History/undo area.
- **Edge cases:** Not customizable; it is a hard-coded `.keyboardShortcut("z")`.
- **Source:** App.swift:81, Undo.swift:357-367

### KB-15 Edit › Redo (⇧⌘Z, fixed)
- **Reaction:** Sends `redo:` to the responder chain, so it works in text fields only. There is no git redo (ponytail comment).
- **Source:** Undo.swift:368-370

### KB-16 Commit (⌘↩, customizable; not in a menu)
- **Trigger:** The commit box button. See the Changes area.
- **Source:** ChangesView.swift:690, Shortcuts.swift:99

### KB-17 Back to List (⌘[, customizable)
- **Trigger:** The PR detail back button, and the CI job log back button. See the PR and CI areas.
- **Source:** PullRequestsView.swift:328, CI.swift:718, Shortcuts.swift:103

### KB-18 Navigate Back (⌃-) and Navigate Forward (⌃⇧-), customizable
- **Trigger:** The code browser's back and forward buttons. They are disabled when their stack is empty. See the code browser area.
- **Source:** CodeBrowser.swift:246-248, Shortcuts.swift:105-106

### KB-19 Next Conflict (⌥⌘↓) and Previous Conflict (⌥⌘↑), customizable
- **Trigger:** The merge editor's chevron buttons. See the merge editor area.
- **Source:** MergeEditor.swift:727-728, Shortcuts.swift:107-108

### KB-20 Use Ours (⌥⌘←) and Use Theirs (⌥⌘→), customizable
- **Trigger:** The merge editor's "Use <ours>" and "Use <theirs>" buttons. See the merge editor area.
- **Source:** MergeEditor.swift:823-824, Shortcuts.swift:109-110

### KB-21 Sheet keys
- **Trigger:** Keys inside sheets.
- **Reaction:**
  - AddAccountSheet: "Cancel" is Esc (`.cancelAction`); "Add" is ↩ (`.defaultAction`).
  - AddWorktreeSheet: "Cancel" and "Create" via the cancellation and confirmation placements (Esc and ↩).
- **Source:** Settings.swift:296-297, App.swift:664-669

### KB-22 System menu items not overridden
- **Items:**
  - Loupe: About, Services, Hide (⌘H), Hide Others, Quit (⌘Q)
  - Edit: Cut, Copy, Paste, Select All
  - View: Enter Full Screen (⌃⌘F)
  - Window: Minimize (⌘M), Zoom
  - Help
- **Edge cases:**
  - No `SidebarCommands()` is added, so the only sidebar menu item is KB-04.
  - No ⌘N, because the newItem group is replaced.
- **Source:** App.swift:76-102

# 2. Repos and worktrees management

## Repos and worktrees

### RW-01 Open Repository (panel)
- **Trigger:** File › Open Repository… (⌘O), the sidebar bottom "plus.rectangle.on.folder" button, or the empty-state button.
- **Preconditions:** None.
- **Render:** An NSOpenPanel for directories only, with multiple selection allowed and the prompt "Open".
- **Reaction:**
  - For each URL, `addRepo`: `git rev-parse --path-format=absolute --git-common-dir`.
  - main = common minus "/.git", otherwise the URL path.
  - If the repo is not in the current space, `moveRepos([main], to: current)`. A repo lives in exactly one space, so opening it again moves it from another space.
  - Then `refreshWorktrees`, and `selection = url.path`.
- **Edge cases:**
  - Cancel does nothing.
  - A non-repo shows the alert "Error: <path> is not a git repository."
  - Opening a linked worktree adds its main repo and selects the linked worktree.
  - Opening a repo already in the current space just selects it.
  - For a common dir not ending in `/.git` (bare repo or submodule), main = the URL path. A submodule's main worktree is remapped through `core.worktree` (`--git-dir=<gitdir> rev-parse --show-toplevel`).
- **Source:** App.swift:79, App.swift:147-155, App.swift:232-233, Model.swift:141-152, Git.swift:152-161

### RW-02 Open a repository by drag and drop
- **Trigger:** Drop folders from Finder anywhere on the window.
- **Reaction:** Each file URL goes through `addRepo` (RW-01). Returns true.
- **Edge cases:** Dropping a file rather than a folder means git runs with a file as its working directory, so it fails and shows the not-a-git-repository error.
- **Source:** App.swift:182-187

### RW-03 Remove a repo from Loupe
- **Trigger:** Repo header context menu › "Remove from Loupe", or Settings › Spaces › space › Repositories › "Remove" (help "Remove from Loupe. Nothing on disk changes.").
- **Reaction:**
  - Removes the repo from every space.
  - Calls `forget` for each worktree: stops its watcher, drops its model, UndoStack and Rewrites.
  - Clears `worktrees[path]`.
- **Edge cases:**
  - No confirmation.
  - The selection is not cleared (see BROKEN).
- **Source:** App.swift:352, Settings.swift:248-249, Model.swift:129-133, Model.swift:154-160

### RW-04 Prune stale worktrees
- **Trigger:** Repo header context menu › "Prune Stale Worktrees".
- **Preconditions:** A worktree folder deleted outside git (prunable).
- **Reaction:** `git worktree prune` in the repo, then `refreshWorktrees`. The prunable row disappears.
- **Edge cases:** Errors are silent.
- **Source:** App.swift:343

### RW-05 New Worktree sheet
- **Trigger:** The header "+", the header menu's "New Worktree…", or "New Worktree from Here" in History (which passes `initialBase`).
- **Preconditions:** A repo with branches.
- **Render:** A grouped Form, 480 wide, titled "New Worktree":
  - A segmented "Branch" picker: "New branch" (the default) / "Existing branch".
  - New branch: a "Name" field (prompt "feature/my-change") and a "Based on" picker: "HEAD" (empty), the initialBase as its first 10 characters if it is not a branch, then every branch.
  - Existing branch: a "Branch" picker with "Choose…" and every branch.
  - A "Location" field and a "Choose…" button (NSSavePanel).
  - A red error line when something fails.
  - Toolbar: "Cancel" (Esc) and "Create" (↩). Create is disabled while the branch is empty, the path is empty, or a create is running.
- **Reaction:**
  - On appear: `git for-each-ref --format=%(refname:short) --sort=-committerdate refs/heads refs/remotes`, without `*/HEAD`. This fills both pickers, newest first.
  - The location is suggested as a sibling of the repo, `<repo>-<branch with / → ->`, with "origin-" removed. It reads `<repo>-worktree` until a branch is typed, and stops updating once the user edits it or picks a path.
  - Create with a new branch: `git worktree add -b <name> <path> [<base>]`.
  - Create with an existing local branch: `git worktree add <path> <branch>`.
  - Create with an existing remote branch "remote/x" that is not a local ref: `git worktree add -b x <path> remote/x`.
  - Then `refreshWorktrees`, `selection = path`, and the sheet closes.
- **Edge cases:**
  - Errors (branch already exists, path exists, branch already checked out elsewhere) show inline in red and the sheet stays open.
  - Picking "origin/x" while a local "x" already exists fails with "already exists" (see BROKEN).
  - Esc and ↩ come from the cancellation and confirmation placements.
- **Source:** App.swift:259-261, App.swift:322, App.swift:341, App.swift:617-707, Model.swift:180-186

### RW-06 Remove Worktree confirmation
- **Trigger:** SH-18.
- **Render:** A confirmation dialog titled 'Remove worktree “<folder>”?' with the message "The directory is deleted. The branch is kept." Buttons: "Remove" and "Force Remove (discard changes)" (both destructive), plus the system Cancel.
- **Reaction:**
  - `git worktree remove [--force] <path>` in the repo, then `forget(path)`.
  - If it was selected, `selection = repo` (the main worktree).
  - Then `refreshWorktrees`.
- **Edge cases:**
  - A dirty worktree with plain Remove fails; git's message ("contains modified or untracked files, use --force") shows in the "Error" alert.
  - A locked worktree fails even with Force, because git needs `-f -f`.
- **Source:** App.swift:262-265, App.swift:360-363, Model.swift:188-193

### RW-07 Worktree added or removed outside Loupe
- **Trigger:** `git worktree add/remove/prune` in a terminal.
- **Preconditions:** The repo is open in Loupe.
- **Render:** The sidebar rows appear or disappear without a manual refresh.
- **Reaction:**
  - The FSWatcher flags `worktreesChanged` when a directory directly under `.git/worktrees/` appears or vanishes. Then `refreshWorktrees`.
  - Vanished entries are forgotten.
  - If the selected one vanished, the selection moves to the repo (vault 0.9.1: this used to loop, rebuilding a model and watcher on the missing folder).
- **Edge cases:**
  - Deleting a worktree folder with `rm -rf` leaves `.git/worktrees/<name>`, so git still lists it as prunable. The row stays (isPrunable is unused); RW-04 clears it.
  - The worktrees array is only written when it changed.
  - The main worktree's watcher also catches this, because the main folder contains `.git`.
- **Source:** Model.swift:166-178, Model.swift:579-583, Model.swift:669-686

### RW-08 Check out a PR in a worktree
- **Trigger:** The PR area's checkout action.
- **Reaction:**
  - If a worktree on branch `pr/N` exists anywhere, it is selected.
  - Otherwise: `git fetch <remote> +refs/pull/N/head:refs/remotes/<remote>/pr/N` (gitAuth), then `git worktree add -B pr/N <repoParent>/<repo>-pr-N <remote>/pr/N`, then refresh, then select it if the folder exists.
  - The busy label is "Checking out PR #N". See the PR area.
- **Source:** Model.swift:556-569

### RW-09 Startup and refresh of every repo's worktrees
- **Trigger:** Launch.
- **Reaction:**
  - All spaces' repos are listed in parallel: `git worktree list --porcelain`, parsed for HEAD, branch (refs/heads stripped), bare, locked and prunable.
  - Each non-bare worktree gets a model: remote detection (`git remote -v`; prefer the remote with an account, then origin, then the first) and status.
- **Edge cases:** A repo whose folder is gone makes `refreshWorktrees` return silently. The header stays, with no rows.
- **Source:** Model.swift:162-178, Model.swift:282-287, Model.swift:523-535, Git.swift:163-181

# 3. Changes (status list, staging, hunks, discard, commit, commit AI, amend, stash, blame/file history)

Notes before the catalog:
- **FileInspector.swift is the Blame / File History panel**, not a general file inspector. The FI flows cover that.
- **Stash apply, pop and drop live in History.swift.** I cover them under SS and cite History.swift lines.
- **The worktree toolbar (Fetch, Pull, Push, branch picker, tab picker) is in ChangesView.swift.** I list it as CH-01 to CH-07 so nothing slips through. The app-shell agent may have the same flows; remove duplicates.
- **The vault note is stale in places.** index.md:136 says line-level staging and a conflict UI are not built. index.md:164 describes a card-style ConflictPane. index.md:165-166 describe a separate File window with "Blame Before This Commit" (index.md:220 already says that window was removed). index.md:168 says the commit model is "haiku". The current code wins; corrections are listed under BROKEN/DEAD.

---

> CH-01..CH-07 (worktree toolbar) overlap SH-25 and SH-29..SH-33: both were traced independently from the same code. Run them once and record the result under both IDs.

## Worktree toolbar (ChangesView.swift `WorktreeView`)

### CH-01 Fetch
- **Trigger:** Toolbar (left, `.navigation` placement) button "Fetch", icon `arrow.triangle.2.circlepath`, help "Fetch all remotes".
- **Preconditions:** Repo with at least one remote, `model.busy == nil`.
- **Render:** While it runs the icon rotates (`symbolEffect(.rotate, isActive: busy == "Fetching")`). No separate spinner. All three remote buttons are disabled while `busy != nil`.
- **Reaction:** `git fetch --all --prune`, run with the `gitAuth` environment (the space's token through `GIT_CONFIG_COUNT`, HTTPS remotes only). It goes through `perform("Fetching")`, has no undo recipe, and then refreshes status. Afterwards `refsVersion += 1` (History reloads the next time it is shown) and `app.refreshWorktrees(repo)`.
- **Edge cases:** A failure shows the "Git" alert with git's stderr and an OK button. A cancellation shows nothing (`userMessage` returns nil).
- **Source:** ChangesView.swift:30-34, Model.swift:503-509, Model.swift:412-417, Model.swift:547-554, ChangesView.swift:97-99

### CH-02 Pull
- **Trigger:** Toolbar button "Pull", icon `arrow.down`. Help reads "Pull N commits (fast-forward only)" when `behind > 0`, otherwise "Pull (fast-forward only)".
- **Preconditions:** A branch with an upstream, behind by at least one commit.
- **Render:** The icon pulses while `busy == "Pulling"`. Disabled while busy.
- **Reaction:** `git pull --ff-only` with `gitAuth`. The undo recipe is `.reset("Pull", published: false)`, so Edit › Undo Pull runs `git reset --keep <before>`. It is recorded only if HEAD moved and no operation was left stopped. There is no toast.
- **Edge cases:** Diverged history makes git refuse the fast-forward, which shows the "Git" alert. Nothing is recorded when HEAD did not move.
- **Source:** ChangesView.swift:35-39, Model.swift:503-509, Undo.swift:100-111

### CH-03 Push
- **Trigger:** Toolbar button "Push", icon `arrow.up`. Help reads "Push N commits" when `ahead > 0`, otherwise "Push".
- **Preconditions:** A branch with commits to push.
- **Render:** The icon pulses while busy is "Pushing".
- **Reaction:** `git push --set-upstream <remoteName> HEAD` with `gitAuth`, then posts the `.branchPushed` notification with the worktree path (the PR-create banner listens). No undo.
- **Edge cases:** `remoteName` is the remote that has an account, else "origin", else the first remote (Model.swift:532-534). A rejected push shows the alert.
- **Source:** ChangesView.swift:40-44, Model.swift:523-535

### CH-04 Branch button and picker popover
- **Trigger:** Click the toolbar title button (branch name, a chevron.down, and the tilde path on a second line).
- **Preconditions:** Several local branches, some remote-only branches, and one branch checked out in another worktree.
- **Render:**
  - The title is the branch, or "detached HEAD". It is empty until the first status arrives (`headOID` empty).
  - Help: "<branch>\n<path>\nClick to switch branch".
  - The popover is 340×420. It has a TextField "Switch to branch…", a red selectable error line, then a List with sections "Local" and "Remote".
  - Each row has an icon (`arrow.triangle.branch` for local, `cloud` for remote) and the name, middle-truncated.
  - The current branch has a checkmark and its row is disabled.
  - A branch checked out in another worktree shows that worktree's folder name as a caption, with help "Checked out in <wt>: selecting it opens that worktree".
- **Reaction:**
  - Locals: `git for-each-ref --format=%(refname:lstrip=2)%09%(worktreepath) --sort=-committerdate refs/heads`.
  - Remotes: the same command for `refs/remotes`. It hides `*/HEAD` and any remote whose name after the first "/" exists as a local branch.
  - Typing filters both lists case-insensitively.
- **Edge cases:** Sections with no matches are hidden. The list is disabled while a switch runs.
- **Source:** ChangesView.swift:46-61, ChangesView.swift:132-193

### CH-05 Switch branch
- **Trigger:** Click a row in the branch picker, or press Return in the filter field (it picks the first match that is not the current branch, locals first).
- **Preconditions:** Same as CH-04, plus a dirty tree for the error case.
- **Render:** On success the popover dismisses. On failure an inline red error appears in the popover.
- **Reaction:**
  - **Branch checked out in another worktree:** `app.selection` moves to that worktree. If the worktree is unknown, worktrees are refreshed first. The popover dismisses. No git switch runs.
  - **Remote branch:** `git switch -c <name-after-first-slash> --track <remote/name>`.
  - **Local branch:** `git switch <b>`.
  - Inside `undo.track`, Loupe records the undo "Switch to <b>", which runs `git switch <previous>` or `git switch --detach <oid>`. There is no toast. Then `refreshWorktrees` runs and the popover dismisses.
- **Edge cases:** Return with an empty filter switches to the most recently committed non-current branch. Conflicting local changes make git refuse, and the message shows inline.
- **Source:** ChangesView.swift:147-148, ChangesView.swift:195-220

### CH-06 View switch (Changes / History / Pull Requests)
- **Trigger:** Toolbar segmented picker "Changes" / "History" / "Pull Requests". Also View menu "Changes" (⌘1), "History" (⌘2), "Pull Requests" (⌘3).
- **Preconditions:** None.
- **Render:** The picker is title-only. The value is `@AppStorage("tab")`, which is global, not per worktree.
- **Reaction:** Swaps the detail view between ChangesView, HistoryView and PullRequestsView.
- **Edge cases:** Because the tab is global, it stays when you switch worktrees.
- **Source:** ChangesView.swift:6, ChangesView.swift:14-21, ChangesView.swift:63-71, App.swift:88-90

### CH-07 Git error alert and Refresh
- **Trigger:** Any failed worktree operation. Review › Refresh (⌘R) posts `.refresh`.
- **Preconditions:** None.
- **Render:** An alert titled "Git" with the error text and one "OK" button.
- **Reaction:** OK clears `model.error`. Refresh calls `refreshStatus()`, which runs `git status --porcelain=v2 -z --branch --untracked-files=all` and `git stash list --format=%H`, detects the operation, refreshes LFS and submodules, and reloads the diff.
- **Edge cases:** AI failures also appear in this alert, still titled "Git".
- **Source:** ChangesView.swift:97-100, Model.swift:298-321, App.swift:100

## Changes layout

### CH-08 Changes tab layout and banners
- **Trigger:** Changes tab selected.
- **Preconditions:** Any repository.
- **Render:**
  - From top to bottom:
    1. The OperationBanner (when an operation is in progress or there are conflicts), otherwise the PullCreateBanner.
    2. The LFSMissingBanner.
    3. The SubmoduleBanner.
    4. A ResizableSplit (key "changesListWidth", 220–380 pt).
  - The left column holds the file list (Compact) or the worktree tree (Full), then the "N stashes" link if `stashCount > 0`, then a Divider and the CommitBox.
  - The right column is chosen in this order:
    1. The Overview.
    2. The Full-mode file pane.
    3. The ConflictPane (for a conflicted selection).
    4. The DiffPane.
  - A code-navigation results panel (220 pt) is inset at the bottom when it has results.
- **Reaction:** None by itself.
- **Edge cases:** The right column has a minimum width of 360. In Full mode a conflicted file shows the Full file pane, not the merge editor (line 266 is checked before line 267).
- **Source:** ChangesView.swift:232-284

### CH-09 Overview / Files switch for uncommitted work
- **Trigger:** Toolbar segmented control with `square.grid.2x2` (help "Overview: what the change does") and `doc.text` (help "Files: the diff").
- **Preconditions:** At least one changed file. With no files the control is hidden.
- **Render:**
  - ChangeOverview of HEAD (or the empty tree when the branch is unborn) against the worktree.
  - One entry per path: deleted files have no new side, added and untracked files have no old side, and a rename uses `origPath`.
  - Sorted by path.
- **Reaction:**
  - Clicking a path in the Overview leaves the Overview and selects that path's unstaged entry, or its staged entry if there is no unstaged one.
  - Any change of the file selection also leaves the Overview.
- **Edge cases:** The Overview's key is `"<headOID> <statusVersion>"`, so it refreshes on every status refresh.
- **Source:** ChangesView.swift:254-265, ChangesView.swift:279-281, ChangesView.swift:285-288

### CH-10 Compact / Full mode in Changes
- **Trigger:** Toolbar segmented control (`rectangle.compress.vertical` / `rectangle.expand.vertical`, shown only while a diff is showing). Also Review › Compact / Full Mode (⌥⌘F).
- **Preconditions:** A file is selected.
- **Render:** Full mode replaces the list with the worktree tree (CH-41) and the diff with the file pane (CH-42).
- **Reaction:**
  - `prFullMode` is toggled and `loadDiff()` runs. Full mode loads the file with `-U1000000` and `wholeFile`.
  - Going to Full: the selected file's path becomes `browser.selectedPath`.
  - Going back to Compact: the browser's path is selected in the list if it is a changed file.
  - In Compact, a jump (⌘-click a definition, a nav result, back or forward) to a file that has no changes turns Full mode on automatically.
- **Edge cases:** Untracked, added and deleted files never load whole (Model.swift:396).
- **Source:** ChangesView.swift:73-81, ChangesView.swift:289-299, Model.swift:388-405, App.swift:98

## Status list

### CH-11 Sections and counts
- **Trigger:** Changes tab in Compact mode.
- **Preconditions:** A mix of conflicted, staged, unstaged and untracked files.
- **Render:**
  - **"Conflicts <n>":** only when conflicts exist, with no header button.
  - **"Staged <n>":** only when something is staged, with a borderless caption button "Unstage All".
  - **"Changes <n>":** always shown, with a "Stage All" button only when it is non-empty.
  - Counts are entry counts in secondary monospaced digits.
  - A path that is both staged and modified appears in both Staged and Changes, with ids "S:path" and "W:path".
  - Conflicted files appear only under Conflicts.
- **Reaction:** See CH-24 and CH-25.
- **Edge cases:** The list order (`listed`) is conflicts, then staged, then unstaged, each in tree order (folders first, natural name sort).
- **Source:** ChangesView.swift:332-352, ChangesView.swift:375-382, Model.swift:230-239, Git.swift:228-265

### CH-12 Folder tree and collapsing
- **Trigger:** Click a folder row in any section.
- **Preconditions:** Nested paths, including a single-child directory chain such as `a/b/c/file`.
- **Render:**
  - A folder row has a chevron (rotated 90° when open), a `folder.fill` icon (open) or `folder` (closed), and the name in secondary, head-truncated.
  - Single-directory chains are compressed into one row, for example "central_api/src".
  - Rows are indented by depth.
- **Reaction:** Toggles the collapse key "<section>/<dir>". Each section (conflicts, staged, changes) folds independently.
- **Edge cases:** Folder rows cannot be selected (`.selectionDisabled()`). The collapse state is view `@State`: it is not persisted and resets when the view is recreated. Right-clicking a folder gives an empty menu.
- **Source:** ChangesView.swift:318, ChangesView.swift:385-412, FileTree.swift:15-60

### CH-13 File row rendering
- **Trigger:** None (render only).
- **Preconditions:** Modified, added, deleted, renamed, untracked, conflicted, type-changed, submodule and LFS-locked files.
- **Render:**
  - The row shows a 12 pt spacer, then the file-type icon (`shippingbox` for a submodule), then the file name only.
  - An LFS lock badge (the owner) appears when the path is locked.
  - The StatusIcon comes last:
    - added / untracked: `plus.square`, green
    - deleted: `minus.square`, red
    - renamed / copied: `arrow.right.square`, blue
    - conflicted: `exclamationmark.square`, orange
    - anything else: `dot.square`, yellow
  - The StatusIcon tooltip is Added, Untracked, Deleted, Renamed, Copied, Conflicted, Type changed or Modified.
  - The row tooltip is the full path.
- **Reaction:** None.
- **Edge cases:** A rename shows only the new name in the list. The old name appears in the diff header.
- **Source:** ChangesView.swift:403-410, ChangesView.swift:447-478, FileIcons.swift:70-107

### CH-14 Clean working tree
- **Trigger:** No changes.
- **Preconditions:** A clean repo.
- **Render:**
  - The list shows "Working Tree Clean" (`checkmark.circle`) with "Nothing to commit on <branch>." (an empty description when detached). The "Changes 0" header stays underneath.
  - The diff side shows "No Changes" (`checkmark.seal`), "Edits in this worktree show up here as you make them."
  - The Overview/Files and Compact/Full toolbar controls are hidden.
  - "Stash…" is disabled.
- **Reaction:** None.
- **Edge cases:** None.
- **Source:** ChangesView.swift:355-359, ChangesView.swift:538-539, ChangesView.swift:279

### CH-15 Selection (single, ⌘-click, ⇧-click, ⌘A) and which diff shows
- **Trigger:** Click, ⌘-click or ⇧-click file rows, or press ⌘A.
- **Preconditions:** At least three files across sections.
- **Render:** The diff pane shows one file, `selectedFileID`.
- **Reaction:**
  - The selection binding computes which rows were just added. The diff shows the first newly added row in list order.
  - If nothing was added, the diff keeps the current file if it is still selected, or else the first selected row in list order.
  - Bulk actions use `selectedFileIDs`.
- **Edge cases:** Unlike FileTreeList (FileTree.swift:125), this List does not swallow ⌘A.
- **Source:** ChangesView.swift:321-331, Model.swift:407-408

### CH-16 Keyboard navigation
- **Trigger:** ↑/↓ in the list (native List behavior). Review › Next File (⌥↓) and Previous File (⌥↑).
- **Preconditions:** A file is selected.
- **Reaction:** The `.moveFile` notification moves the selection by ±1 through `listed`, clamped at both ends. It does nothing when nothing is selected.
- **Edge cases:** HistoryView listens to the same notification, so only the mounted tab reacts. In Full mode it still moves the list selection, which the tree then follows.
- **Source:** ChangesView.swift:306-310, App.swift:95-96, Shortcuts.swift:100-101

### CH-17 Selection on status refresh
- **Trigger:** Any status refresh: FS watcher (250 ms debounce), ⌘R, or after any action.
- **Preconditions:** The selected file changes state (it gets staged, committed, or disappears).
- **Reaction:**
  - If the selected id disappears, Loupe selects the same path under its other id (W↔S). Failing that it selects the first file, or nothing.
  - The multi-selection is intersected with the ids still present. If it ends up empty, it becomes the shown file.
  - The diff reloads but is not replaced when its text is unchanged (scroll and selection are kept).
- **Source:** Model.swift:298-321, Model.swift:400-404, Model.swift:649-656

### CH-18 File context menu
- **Trigger:** Right-click a file row. The menu applies to the selection, or to the clicked row when it is outside the selection.
- **Preconditions:** A mixed selection of staged, unstaged, untracked, submodule and LFS files.
- **Render:** Items in order, each shown only when it applies:
  1. "Stage" / "Stage N Files": the unstaged part of the selection.
  2. "Unstage" / "Unstage N Files": the staged part.
  3. "Discard Changes…" / "Discard Changes N Files…" (destructive): the unstaged non-submodule part.
  4. "Stash" / "Stash N Files": N counts unique paths.
  5. Submodule items: "Open Submodule" (one submodule; disabled unless it is checked out) and "Update to Recorded Commit(s)".
  6. LFS items, when LFS is installed: "Track .<ext> with LFS", "Track This File with LFS" and, when the server supports locking, "Lock[ N Files]" / "Unlock[ N Files]".
  7. Divider, then "Blame" and "File History": only for a single file that is not untracked, not added, and not a submodule.
  8. Divider, then "Reveal in Finder", "Open", "Copy Path" / "Copy Paths".
- **Reaction:** See CH-19, CH-20, CH-22, CH-32 and SS-03.
- **Edge cases:** Right-clicking empty space or a folder gives an empty menu. Conflicted files count as unstaged, so Stage and Discard are offered for them (see BROKEN/DEAD).
- **Source:** ChangesView.swift:360, ChangesView.swift:415-444, Submodules.swift:132-147, LFS.swift:166-189

### CH-19 Reveal in Finder / Open / Copy Path
- **Trigger:** Context menu "Reveal in Finder", "Open", "Copy Path(s)".
- **Preconditions:** One or more files.
- **Reaction:**
  - Reveal: `NSWorkspace.activateFileViewerSelecting` with the absolute URLs.
  - Open: `NSWorkspace.open` on each file.
  - Copy: clears the pasteboard and puts the worktree-relative paths on it, joined with "\n".
- **Edge cases:** Revealing or opening a deleted file points at a missing path. The OS decides what happens.
- **Source:** ChangesView.swift:436-443

### CH-20 Blame / File History from the context menu
- **Trigger:** Context menu "Blame" or "File History".
- **Preconditions:** A single tracked file that is not added.
- **Reaction:** Selects the file and sets the `filePanel` default to blame or history. Unlike the header buttons, this sets the panel rather than toggling it. See FI-01 and FI-02.
- **Source:** ChangesView.swift:431-435

### CH-21 Submodule and LFS rows
- **Trigger:** Select a submodule row, or use the submodule/LFS context items.
- **Preconditions:** A submodule with changes, a LFS pointer file, and a locked file.
- **Render:**
  - A submodule shows SubmodulePane instead of a diff.
  - An LFS pointer diff shows LFSObjectView.
  - Locked files show LFSLockBadge.
  - The SubmoduleBanner has a "Not Now" button.
- **Reaction:** Owned by the LFS/submodule code: `openSubmodule`, `updateSubmodules`, `lfsTrack`, `lfsLock`.
- **Edge cases:** Submodules are excluded from Discard (`git restore` leaves a submodule as it is).
- **Source:** ChangesView.swift:406, ChangesView.swift:421, ChangesView.swift:429-430, ChangesView.swift:499, ChangesView.swift:513

## Staging

### CH-22 Stage files (context menu)
- **Trigger:** Context menu "Stage" / "Stage N Files".
- **Preconditions:** Unstaged modified, deleted and untracked files.
- **Reaction:** `git add -A -- <paths>` through `perform("Staging")`. The undo recipe is `.files("Stage <name>" | "Stage N Files", [], index: true, toast: false)`: it saves `git write-tree` before and after. Edit › Undo then reads "Undo Stage a.txt" and runs `read-tree -m <before>`. There is no toast. Status refreshes and the file moves to Staged with the same path selected.
- **Edge cases:**
  - Undo is refused (and dropped) when the index changed since: "The staged changes changed since Stage a.txt, so undoing it would overwrite that. Nothing was changed."
  - With conflicts in the index `write-tree` fails, so nothing is recorded.
- **Source:** ChangesView.swift:419, Model.swift:419-423, Undo.swift:112-118, Undo.swift:254-263, Undo.swift:349-351

### CH-23 Space toggles staging
- **Trigger:** Space with focus in the file list.
- **Preconditions:** A selection that is all unstaged, mixed, or all staged.
- **Reaction:** If any selected entry is unstaged, Space stages the unstaged ones (`stage(unstaged)`). If every selected entry is staged, Space unstages them all.
- **Edge cases:** With an empty selection the key passes through (`.ignored`). A conflicted selection gets staged, with any markers still in it.
- **Source:** ChangesView.swift:361-368

### CH-24 Stage All
- **Trigger:** The "Stage All" button in the Changes section header.
- **Preconditions:** Unstaged files exist. Without them the button is hidden.
- **Reaction:** `stage(model.unstaged)`, which leaves conflicted files out. The undo label is "Stage N Files" (or "Stage <name>" for one file).
- **Edge cases:** All paths go in argv, which could get very long with thousands of files.
- **Source:** ChangesView.swift:350, Model.swift:233

### CH-25 Unstage All
- **Trigger:** The "Unstage All" button in the Staged section header.
- **Preconditions:** Staged files.
- **Reaction:** `unstage(model.staged)`. See CH-26 for the git commands.
- **Source:** ChangesView.swift:344

### CH-26 Unstage files (context menu, header, Space)
- **Trigger:** Context menu "Unstage" / "Unstage N Files". Diff header button "Unstage File" (when the shown file is staged). Space on an all-staged selection.
- **Preconditions:** Staged files, including a staged rename.
- **Reaction:**
  - First `git rev-parse --verify HEAD`.
  - With HEAD: `git restore --staged -- <path> [<origPath>]` for each file, so a staged rename passes both paths.
  - The undo recipe is `.files("Unstage …", [], index: true, toast: false)`.
- **Edge cases:** On an unborn branch see CH-27.
- **Source:** ChangesView.swift:420, ChangesView.swift:504, Model.swift:425-432

### CH-27 Unstage on an unborn branch
- **Trigger:** Same as CH-26.
- **Preconditions:** A new repo with no commits and staged files.
- **Reaction:** `git rm --cached -r -q -- <paths>`.
- **Source:** Model.swift:426-430

### CH-28 Stage File / Discard… / Unstage File in the diff header
- **Trigger:** DiffHeader buttons. For an unstaged file: "Discard…" and "Stage File" (prominent). For a staged file: "Unstage File".
- **Preconditions:** A selected file with a loaded diff.
- **Reaction:** Stage File calls `stage([f])`. Unstage File calls `unstage([f])`. Discard… opens the confirmation from CH-32 for this one file.
- **Edge cases:** Buttons are small (`controlSize(.small)`). Conflicted files never reach this header in Compact mode because they open the merge editor.
- **Source:** ChangesView.swift:502-509, ChangesView.swift:590

### CH-29 Stage or unstage a hunk (rendered by the diff viewer)
- **Trigger:** In the hunk header row: "Stage Hunk" for an unstaged diff, "Unstage Hunk" for a staged diff. Also the diff context menu items "Stage Hunk" / "Unstage Hunk".
- **Preconditions:** A modified tracked file with at least two hunks.
- **Render:** `hunkActions` are `[.unstage]` for staged files, `[]` for untracked files, otherwise `[.discard, .stage]`.
- **Reaction:**
  - `model.apply(action, lines)` builds a patch from the hunk's lines (`FileDiff.patch(selecting:reverse:)`).
  - Stage: `git apply --cached --whitespace=nowarn -` with the patch on stdin.
  - Unstage: `git apply --cached --reverse --whitespace=nowarn -`.
  - The undo is `.files("Stage in <name>" | "Unstage in <name>", [], index: true, toast: false)`.
- **Edge cases:**
  - The action is skipped when the shown diff is stale (it belongs to a different file than the selection).
  - It is skipped when the patch is empty (no changed lines selected).
  - Untracked files have no hunk buttons at all.
- **Source:** ChangesView.swift:515-527, Model.swift:445-463, DiffTableView.swift:1127-1143, Diff.swift:42-70

### CH-30 Stage, unstage or discard selected lines (rendered by the diff viewer)
- **Trigger:** Select diff lines, right-click, then "Stage Line" / "Stage N Lines", "Unstage Line(s)", or "Discard Line…" / "Discard N Lines…".
- **Preconditions:** A hunk with several added and removed lines.
- **Reaction:**
  - The lines used are the selected changed lines when the click is inside the selection, otherwise the clicked line if it is a change.
  - The patch keeps unselected removed lines as context when applying forward, and unselected added lines as context when reversing.
  - From there it goes through the same `apply` path as CH-29. A line discard first shows the CH-36 alert.
- **Edge cases:** Clicking a context line outside the selection shows no line items.
- **Source:** DiffTableView.swift:701-714, Diff.swift:42-70, Model.swift:445-463

### CH-31 Undo of stage and unstage
- **Trigger:** Edit › "Undo Stage a.txt" (⌘Z) while focus is not in a text field.
- **Preconditions:** A stage action just happened, and nothing has changed since.
- **Reaction:** `git read-tree -m <indexBefore>`. A toast "Undone: Stage a.txt" appears with an `arrow.uturn.backward.circle.fill` icon.
- **Edge cases:**
  - If the entry no longer applies, a "Can't Undo" alert explains why.
  - With an empty stack the Mac beeps.
  - A second ⌘Z while one is running is ignored.
  - When the firstResponder is an NSText, ⌘Z is a text undo instead.
- **Source:** Undo.swift:144-161, Undo.swift:329-347, Undo.swift:357-372

## Discard

### CH-32 Discard files (context menu)
- **Trigger:** Context menu "Discard Changes…" / "Discard Changes N Files…".
- **Preconditions:** Unstaged tracked modifications and untracked files.
- **Render:** A confirmation dialog titled "Discard changes?" with the destructive button "Discard N file(s)" and Cancel. Message: "Tracked changes are reverted; untracked files are moved to the Trash. Edit › Undo brings them back."
- **Reaction:**
  - `perform("Discarding")` runs `git restore --worktree -- <tracked paths>`. This restores to the index version, so any staged part stays.
  - Each untracked file goes to the Trash with `FileManager.trashItem`.
  - The undo recipe is `.files("Discard of <name>" | "Discard of N Files", paths + origPaths)`. It saves the blobs with `hash-object -w --no-filters --stdin-paths`, along with symlinks and executable bits.
  - A toast appears: "Discard of a.txt" with `checkmark.circle.fill` and an "Undo" button (help "Undo (⌘Z)"). It disappears after 6 s.
- **Edge cases:**
  - Submodules are filtered out.
  - The Undo button is only shown while the entry is still the top of the stack.
  - Undo is refused when a file changed since: "<path> changed since Discard of a.txt, so undoing it would overwrite that. Nothing was changed." The entry is dropped.
  - Undo of a trashed untracked file recreates it from the saved blob.
- **Source:** ChangesView.swift:301-305, ChangesView.swift:421-422, Model.swift:434-442, Undo.swift:112-118, Undo.swift:228-315, Undo.swift:375-407

### CH-33 Delete key discards the selection
- **Trigger:** ⌫ (`onDeleteCommand`) with focus in the file list.
- **Preconditions:** Unstaged files selected.
- **Reaction:** Opens the CH-32 confirmation for the selected unstaged non-submodule files. It does nothing when only staged files are selected.
- **Source:** ChangesView.swift:369-372

### CH-34 Discard… in the diff header
- **Trigger:** The "Discard…" button in the DiffHeader, or in the Full-mode file pane.
- **Preconditions:** An unstaged selected file.
- **Reaction:** The CH-32 confirmation for that one file.
- **Source:** ChangesView.swift:506, ChangesView.swift:781

### CH-35 Discard an untracked file (to the Trash)
- **Trigger:** CH-32, CH-33 or CH-34 on an untracked file. A "Discard" hunk action on an untracked file would also route here, but none is offered.
- **Preconditions:** An untracked file.
- **Reaction:** The file is moved to the Trash (not deleted) and an undo is recorded with a toast.
- **Edge cases:** A failure moving to the Trash shows the alert.
- **Source:** Model.swift:439-440, Model.swift:448

### CH-36 Discard a hunk (rendered by the diff viewer)
- **Trigger:** The red "Discard" button in the hunk header, or the diff context menu "Discard Hunk…".
- **Preconditions:** An unstaged modified file.
- **Render:** A modal NSAlert "Discard these changes?" with the informative text "The change is removed from the working tree. Edit › Undo brings it back." Buttons: "Discard" (destructive) and "Cancel".
- **Reaction:** `git apply --reverse --whitespace=nowarn -` with the patch on stdin. The undo is `.files("Discard in <name>", [path])`, with a toast "Discard in a.txt · Undo".
- **Edge cases:** Cancel does nothing.
- **Source:** ChangesView.swift:517-527, Model.swift:449-461

### CH-37 Discard lines
- **Trigger:** Diff context menu "Discard Line…" / "Discard N Lines…".
- **Preconditions:** Same as CH-30.
- **Reaction:** The same alert and command as CH-36, applied to the selected lines only.
- **Source:** DiffTableView.swift:705-708, ChangesView.swift:518-526

### CH-38 Undo toast and refusals (shared)
- **Trigger:** Any action recorded with `toast: true` (file discard, hunk discard, Pop, Drop), any undo, or the toast's "Undo" button.
- **Render:** A capsule at the bottom of the worktree view in regular material, with a shadow, sliding in from the bottom. The action toast shows a check icon. The undone toast reads "Undone: <label>" with a uturn icon. It auto-hides after 6 s.
- **Reaction:**
  - The Undo button calls `undoLast()`.
  - A refusal shows the "Can't Undo" alert with the message. For a pushed commit the alert has "Revert Commit" and "Cancel". Revert runs `git revert --no-edit [-m 1] <sha>`.
- **Edge cases:**
  - When HEAD moves outside Loupe, entries made at other HEADs are pruned and the toast disappears.
  - The stack holds at most 50 entries per worktree.
- **Source:** Undo.swift:79-86, Undo.swift:144-174, Undo.swift:329-347, Undo.swift:375-407, ChangesView.swift:22

## Diff pane

### CH-39 Diff pane states
- **Trigger:** Changes in the selection.
- **Render:**
  - A submodule shows SubmodulePane.
  - A loaded diff shows the header and table.
  - While the first diff loads there is a spinner. The previous diff stays visible while the next one loads, to avoid a flash.
  - With no files: "No Changes".
  - With files but no selection: "No File Selected" (`doc.text.magnifyingglass`).
  - A binary file shows "Binary File" (`doc.zipper`).
  - An LFS pointer shows LFSObjectView.
  - Markdown can be shown as a preview.
  - A conflicted file shows the merge editor (MG-05).
- **Reaction:**
  - Unstaged file: `git diff --no-color --no-ext-diff -M -U3 -- <path> [<origPath>]`.
  - Staged file: the same with `--cached`.
  - Untracked file: `git diff --no-color --no-ext-diff --no-index -U3 -- /dev/null <path>`. Exit status 1 is allowed here.
- **Source:** ChangesView.swift:492-544, Git.swift:267-277

### CH-40 DiffHeader controls
- **Trigger:** The header row.
- **Render:**
  - File icon, then "old → new" for a rename, then the selectable path, then "+N" (green) and "−N" (red).
  - A "Diff / Before / After" segmented control, only when the file has both additions and deletions and is not being previewed as Markdown.
  - Blame and History toggles, only for a FileTarget (not untracked or added).
  - A Divider, a "Preview" checkbox for Markdown, then the action buttons.
- **Reaction:** Changing the side sets the `diffSide` default and runs `loadDiff()`. Before and After load the whole file.
- **Source:** ChangesView.swift:546-606, ChangesView.swift:300

## Full mode

### CH-41 Worktree tree (Full mode)
- **Trigger:** Full mode on.
- **Preconditions:** A worktree with tracked, changed and untracked files.
- **Render:**
  - A spinner until the tree loads.
  - Every file from `git ls-files -co --exclude-standard -z`.
  - Unchanged files appear in secondary color, changed files in primary with their StatusIcon and any LFS badge.
  - Each row's tooltip is the path.
- **Reaction:** Selecting a file sets `browser.selectedPath` and opens it in a tab. The tree reloads whenever `statusVersion` or `changeCount` changes.
- **Source:** ChangesView.swift:723-748, CodeBrowser.swift:121-126

### CH-42 Full-mode file pane
- **Trigger:** Open a file from the tree.
- **Render:**
  - FileTabBar.
  - DiffHeader with nav controls. Counts show only for changed files. Changed files get "Unstage File" or "Discard…" + "Stage File".
  - "No File Open": "Open one from the tree, or ⌘-click a name to go to its definition."
  - "Empty File" (`doc`) and "Binary File".
- **Reaction:**
  - ⌘W closes the current tab, and does nothing when no tab is open.
  - The file loads again on every `statusVersion` change.
  - In Full mode with no tabs open, the list's selected change opens.
- **Edge cases:** There are no hunk or line actions in Full mode (no `hunkActions`). Conflicted files show the plain file, not the merge editor.
- **Source:** ChangesView.swift:751-804

---

## Commit, amend, commit AI

### CM-01 Commit box render
- **Trigger:** The Changes tab, at the bottom of the left column.
- **Render:**
  - A rounded TextField "Summary", with the ✨ button (`sparkles`) to its right when an AI tool is set up.
  - A TextEditor 56 pt tall with a tertiary "Description" placeholder.
  - A row with the "Amend" checkbox, a Spacer, "Stash…", and the prominent Commit button (min width 70).
  - The background is secondary.
- **Reaction:** None.
- **Edge cases:** There are no character limits or counters.
- **Source:** ChangesView.swift:639-696

### CM-02 Summary and description editing
- **Trigger:** Typing in Summary or Description.
- **Reaction:**
  - `model.commitMessage` is one string. The Summary edits its first line and the Description edits everything after it, with leading newlines stripped.
  - They are joined as `summary + "\n\n" + description`, or just the summary when the description is empty.
  - Newlines typed or pasted into Summary become spaces.
- **Edge cases:** A Return typed at the very start of the Description is swallowed by the strip. The message is per worktree and survives tab switches.
- **Source:** ChangesView.swift:700-711, Model.swift:260

### CM-03 Commit
- **Trigger:** The Commit button, whose label is "Commit", "Commit N" (N = staged entries) or "Amend". Also ⌘↩ (shortcut `.commit`, customizable) and Return in the Summary field when committing is allowed.
- **Preconditions:** Staged changes and a summary.
- **Reaction:**
  - The message is trimmed.
  - `perform("Committing")` runs `git commit -F -` with the message on stdin, plus `--amend` when amending.
  - On success the message is cleared and `amend` is set to false.
  - The undo is `.reset("Commit", soft: true, offerRevert: true)`. Undo runs `git reset --soft <before>`, puts the message back only if the field is still empty, and restores the amend flag. It appears in Edit › "Undo Commit". There is no toast.
- **Edge cases:**
  - On the first commit, undo runs `git update-ref -d HEAD`, back to unborn.
  - If the commit is already on a remote branch, undo is refused with "Commit is already on <remote/branch>. Undoing it would rewrite pushed history. Revert it instead: a new commit that takes it back." The alert offers "Revert Commit".
  - When git fails (a hook, unmerged files, nothing staged), the message is kept and the "Git" alert shows stderr.
- **Source:** ChangesView.swift:642-643, ChangesView.swift:683-692, Model.swift:465-479, Shortcuts.swift:99, Undo.swift:100-111, Undo.swift:151-157

### CM-04 When Commit is disabled
- **Trigger:** None (render only).
- **Render:** The button is disabled unless the summary has non-whitespace text and either something is staged or Amend is on. Return in Summary is ignored in the same cases.
- **Edge cases:** The button is not disabled while `busy` is set, so a fast double ⌘↩ makes a second `git commit` that fails with "nothing to commit". The box stays enabled during a merge or rebase.
- **Source:** ChangesView.swift:698, ChangesView.swift:691

### CM-05 Amend toggle
- **Trigger:** The "Amend" checkbox.
- **Preconditions:** At least one commit.
- **Reaction:** Turning it on with an empty message runs `git log -1 --format=%B` and fills the trimmed message into the fields. The button label becomes "Amend". Turning it off does not clear the message.
- **Edge cases:** On an unborn branch the log fails silently and committing then fails with "nothing to amend". With a message already typed, nothing is loaded.
- **Source:** ChangesView.swift:667-669, Model.swift:499-501

### CM-06 Amend commit
- **Trigger:** The "Amend" button or ⌘↩ with Amend on.
- **Preconditions:** A HEAD commit, with or without staged changes. A message-only reword is allowed.
- **Reaction:** `git commit -F - --amend`. The undo is `.reset("Amend", soft: true, offerRevert: false)`, so undo runs `reset --soft` to the commit before the amend. If that commit was pushed, undo is refused without a Revert offer.
- **Edge cases:** During a merge, git refuses with "cannot amend", shown in the alert.
- **Source:** Model.swift:465-479

### CM-07 Commit at an interactive-rebase "edit" stop
- **Trigger:** The banner text "Stopped to edit this commit. Change it (commit with Amend), then continue."
- **Preconditions:** A rebase stopped at an `edit` step (`rebase-merge/amend` exists).
- **Reaction:** The user ticks Amend (CM-05), amends (CM-06), then presses Continue (MG-03).
- **Source:** Conflicts.swift:95-106, Model.swift:348-349

### CM-08 When the ✨ button shows, and its states
- **Trigger:** Render only.
- **Preconditions:**
  - `CommitAI.selected != nil`: the `aiHarness` default is not "off", and the chosen tool (or, for "auto", the first one installed in `Harness.all` order) was found.
  - Tool detection: AIHarnesses scans the known bin folders, then runs `zsh -lc 'command -v …'`, once at launch and again from Settings "Look Again".
- **Render:**
  - The button is accent-colored and enabled when there are changes or Amend is on. Otherwise it is secondary and disabled.
  - Help: "Write the message with <Tool>, from the staged changes (or all of them when nothing is staged)". The first word is "Rewrite" when a message already exists.
  - While it runs, a small ProgressView 20 pt wide replaces the button.
- **Edge cases:** The button appears only after the asynchronous scan finishes. It re-renders when Settings changes `aiHarness`.
- **Source:** ChangesView.swift:637, ChangesView.swift:644-655, CommitAI.swift:103-141, CommitAI.swift:158-163

### CM-09 Generate a commit message
- **Trigger:** Click ✨.
- **Preconditions:** Changes exist and a tool is installed (a mock CLI works).
- **Reaction:**
  - `generatingMessage = true`.
  - The diff is chosen like this:
    - Amend on and `HEAD~1` exists: `git diff --no-color --no-ext-diff -M -U2 --cached HEAD~1`.
    - Otherwise, something staged: `... --cached`.
    - Otherwise, HEAD exists: `... HEAD`.
    - Otherwise: a plain diff.
  - In parallel: the same arguments without `-U2` plus `--stat`; `git log -n 15 --format=%s` (failure allowed); and, only when nothing is staged and Amend is off, `git ls-files --others --exclude-standard`.
  - The patch is capped at 60,000 bytes and ends with "[diff truncated]" when cut.
  - The prompt contains: the style instruction, the extra instructions, the reply rules, "Recent subjects:", "Summary:" (the stat), "New files:", and "Diff:".
  - The cleaned reply replaces the whole `commitMessage`.
- **Edge cases:**
  - Text typed while it runs is overwritten.
  - There is no cancel button.
  - Switching worktrees does not cancel it.
- **Source:** Model.swift:491-497, CommitAI.swift:167-221

### CM-10 How each tool is invoked
- **Trigger:** Part of CM-09 and MG-17.
- **Reaction:** The tool runs in the worktree directory. PATH gets the tool's own folder, `/opt/homebrew/bin`, `/usr/local/bin` and `~/.local/bin` in front, and `NO_COLOR=1` is set. Per tool:

| Tool | Arguments | Prompt goes in |
|---|---|---|
| claude | `-p --tools "" --setting-sources "" --strict-mcp-config --no-session-persistence --output-format text [--model m]` | stdin |
| codex | `exec --skip-git-repo-check --ephemeral -s read-only -o <tmpfile> [-m m] -` | stdin; the answer is read from the temp file |
| gemini | `--output-format text [-m m] -p <prompt>` | argv |
| copilot | `-p <prompt> -s --no-ask-user --no-color [--model m]` | argv |
| cursor-agent | `-p --output-format text --mode ask --trust [--model m] <prompt>` | argv |
| opencode | `run [-m m] <prompt>` | argv |
| qwen | `-p <prompt> --output-format text --approval-mode plan [-m m]` | argv |
| amp | `-x` | stdin |
| droid | `exec -o text [-m m]` | stdin |

- **Model:** Read from the `aiModel.<id>` default. For claude it defaults to `claude-haiku-4-5-20251001`, and the old aliases are mapped: haiku → claude-haiku-4-5-20251001, sonnet → claude-sonnet-5, opus → claude-opus-5-5.
- **Source:** CommitAI.swift:29-60, CommitAI.swift:149-155, CommitAI.swift:175-187, CommitAI.swift:241-277

### CM-11 AI errors and timeout
- **Trigger:** Failure cases of CM-09.
- **Render:** The "Git" alert with one of these messages:
  - "There are no changes to describe." (empty patch and no new files)
  - "<Tool> returned an empty message."
  - "<exe> took longer than 120 s and was stopped." (the process is terminated)
  - A non-zero exit: the cleaned stderr (or stdout), last 600 characters, or "<exe> exited with N"
  - No tool set up: "No AI tool is set up for commit messages. Pick one in Settings › General." This one cannot be reached from the button, which is hidden in that case.
  - Launch failure: the system error.
- **Reaction:** The spinner ends and the message is left unchanged.
- **Source:** CommitAI.swift:168-170, CommitAI.swift:185, CommitAI.swift:202, CommitAI.swift:256, CommitAI.swift:260-274

### CM-12 Cleaning the AI reply
- **Trigger:** Part of CM-09.
- **Reaction:** Removes ANSI escapes, trims whitespace, drops a leading ``` fence line and every line starting with ```, removes wrapping double quotes, and trims again.
- **Source:** CommitAI.swift:224-230

### CM-13 AI settings that commit and merge AI read
- **Trigger:** Settings › General › "Commit Messages" section (render owned by the settings agent).
- **Reaction:**
  - Defaults read: `aiHarness` ("auto", a tool id, or "off"), `aiStyle`, `aiInstructions`, `aiModel.<id>`.
  - Styles, with their prompt text:
    - "Match the repository": match recent subjects.
    - "Concise": one line, at most 60 characters.
    - "Standard": at most 72 characters, plus an optional short description.
    - "Detailed": a summary plus bullet points.
    - "Conventional Commits": type(scope) with the type list.
  - Model suggestions come from `cursor-agent models` and `opencode models` (20 s timeout), falling back to the built-in lists.
- **Source:** CommitAI.swift:64-99, CommitAI.swift:146-148, CommitAI.swift:233-239, Settings.swift:59-131

---

## Stash

### SS-01 "Stash…" popover
- **Trigger:** The "Stash…" button in the commit box (help "Set all changes aside without committing them").
- **Preconditions:** At least one changed file. Otherwise the button is disabled.
- **Render:** A popover above the button containing a TextField "Message (optional)" (260 pt), a checkbox "Include untracked files" (on by default), and a prominent "Stash" button (Return is its default action).
- **Reaction:** See SS-02.
- **Edge cases:** The untracked checkbox keeps its value between openings (view `@State`). The message is cleared after each stash.
- **Source:** ChangesView.swift:634-636, ChangesView.swift:671-682

### SS-02 Stash everything
- **Trigger:** "Stash" in the popover, or Return in the message field.
- **Preconditions:** Tracked changes and untracked files.
- **Reaction:**
  - The popover closes.
  - `git stash push [--include-untracked] [-m <trimmed msg>]` runs through `perform("Stashing")`.
  - Then `history.reload()` runs. The stash count goes up, so the "N stashes" link appears.
  - No undo is recorded and there is no toast.
- **Edge cases:** With conflicted files git refuses, and the alert shows it. After a stash, ⌘Z goes to whatever older entry is on the stack.
- **Source:** ChangesView.swift:713-717, Model.swift:482-489

### SS-03 Stash selected files
- **Trigger:** Context menu "Stash" / "Stash N Files".
- **Preconditions:** A selection, optionally with untracked files in it.
- **Reaction:** `git stash push [--include-untracked] -- <unique sorted paths>`. `--include-untracked` is added only if the selection contains an untracked file. There is no message.
- **Source:** ChangesView.swift:423-428, Model.swift:482-489

### SS-04 Stashes link
- **Trigger:** The row under the file list: a `tray.full` icon, "1 stash" or "N stashes", and a chevron (help "Show stashes in History").
- **Preconditions:** `stashCount > 0` (from `git stash list --format=%H` on every refresh).
- **Reaction:** `history.setScope(.stashes)` and the tab changes to "history".
- **Source:** ChangesView.swift:246, ChangesView.swift:609-630, Model.swift:305-306

### SS-05 Stash list (History, Stashes scope)
- **Trigger:** SS-04, or the History scope menu "Stashes".
- **Render:**
  - Search placeholder "Search stashes" filters by subject.
  - Rows come from `git stash list --format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%gd%x1f%s%x1e`. The selector is stash@{n}.
  - The detail shows `git stash show -p --include-untracked --no-color --no-ext-diff -M <sel>`.
  - Empty state: "No Stashes" (`tray`), "Stash changes from the Changes tab to set them aside."
  - There is no graph in this scope.
- **Source:** History.swift:52-55, History.swift:74-77, History.swift:190-195, History.swift:291, History.swift:315-316, History.swift:362

### SS-06 Apply a stash
- **Trigger:** The detail header "Apply" (help "Apply the changes and keep the stash"), or the row context menu "Apply".
- **Preconditions:** A stash and a compatible worktree.
- **Reaction:** `git stash apply stash@{n}`. The undo is `.stash("Apply Stash", applied: true, dropped: false)`: it snapshots the stash's files (`git stash show -z --name-only --no-renames --include-untracked <sha>`) and the index first. There is no toast. History reloads.
- **Edge cases:** On a conflict the error is suppressed and the tab switches to Changes (SS-10). On any other failure the History "Git" alert shows. Nothing is recorded when git fails.
- **Source:** History.swift:248-255, History.swift:263-265, History.swift:377, History.swift:389, Undo.swift:119-136

### SS-07 Pop a stash
- **Trigger:** The header "Pop" (prominent; help "Apply the changes and drop the stash"), or the context menu "Pop" (first item).
- **Reaction:** `git stash pop stash@{n}`. The undo is `.stash("Pop Stash", applied: true, dropped: true)`, with a toast. Undo restores the files, then runs `git stash store -m <subject> <sha>`.
- **Edge cases:** If restoring the files is refused, the stash is still stored again and the message gets " The stash is back in the list." appended.
- **Source:** History.swift:263-265, History.swift:378, History.swift:388, Undo.swift:128-135

### SS-08 Drop a stash
- **Trigger:** The header or context menu "Drop…" (destructive).
- **Render:** A confirmation dialog "Drop stash@{n}?" with "Drop Stash" (destructive). Message: "The stash is removed from the list. Edit › Undo brings it back."
- **Reaction:** `git stash drop stash@{n}`. The undo is `.stash("Drop Stash", applied: false, dropped: true)`, with a toast. Undo runs `git stash store -m <subject> <sha>`.
- **Source:** History.swift:266-268, History.swift:340-342, History.swift:376, History.swift:390

### SS-09 Copy a stash message
- **Trigger:** The stash row context menu "Copy Message".
- **Reaction:** Copies the subject to the pasteboard.
- **Source:** History.swift:391-392

### SS-10 Apply or pop stopped on conflicts
- **Trigger:** SS-06 or SS-07 when the stash conflicts.
- **Preconditions:** The worktree has edits that conflict with the stash.
- **Reaction:** `git diff --name-only --diff-filter=U` is non-empty, so `stoppedOnConflicts` is set and the tab switches to Changes. The OperationBanner shows "Conflicts" with no Abort or Continue (there is no operation). The conflicted files open in the merge editor with the labels "Ours" / "Theirs".
- **Edge cases:** git keeps the stash on a pop with conflicts. Nothing is recorded for undo.
- **Source:** History.swift:248-255, History.swift:339, Conflicts.swift:98-117, MergeEditor.swift:18

---

## File inspector (Blame / File History panel, FileInspector.swift)

### FI-01 Blame gutter toggle
- **Trigger:** The DiffHeader button `person.text.rectangle` (help "Blame: who last changed each line"), or the file context menu "Blame".
- **Preconditions:** A tracked file that is not untracked or added.
- **Render:** The button is accent-colored with a 15% accent background while on. The diff draws the blame gutter (rendered by the diff viewer).
- **Reaction:**
  - Toggles the `filePanel` default: a second click turns it off, and clicking History switches panels.
  - Runs `git blame --porcelain [<rev>] -- <path>` and parses the sha, author, author-time and summary for each line.
  - Blame runs again whenever the diff's `uid` changes, so it follows edits.
- **Edge cases:**
  - Blame failures are silent (no gutter). This includes a deleted file, which still shows the button.
  - The panel setting persists across files and app restarts.
  - Untracked and added files have no target, so the gutter is hidden for them.
- **Source:** FileInspector.swift:18-35, FileInspector.swift:65, FileInspector.swift:108-112, ChangesView.swift:579-587, ChangesView.swift:597-605

### FI-02 Open and close the History panel
- **Trigger:** The DiffHeader button `clock.arrow.circlepath` (help "File History"), the context menu "File History", or the panel's `xmark` (help "Close").
- **Render:** A list on the left: header "History", the file name (caption, middle-truncated) and the close button. A 1 pt separator, then the diff.
- **Reaction:** Toggles or sets `filePanel = .history`, or `.none` on close.
- **Source:** FileInspector.swift:83-100, FileInspector.swift:159-199

### FI-03 History list contents
- **Trigger:** The panel opens. The list is rebuilt when the file changes (`.id(file)`).
- **Preconditions:** A file with several commits, including one rename.
- **Render:**
  - A spinner while loading. "No History" (`clock`) when empty.
  - Rows use CommitRow. When a commit touched the file under another path, a caption with `arrow.turn.down.right` shows that path.
- **Reaction:** `git log --follow -M -n 1000 --format=%x1e<logFormat> --decorate=full --name-status -- <path>`.
- **Source:** FileInspector.swift:38-47, FileInspector.swift:177-198

### FI-04 Pick a commit to see its change to the file
- **Trigger:** Click a row in the History list.
- **Render:**
  - The diff area is replaced by a bar: `clock.arrow.circlepath`, the short sha (monospaced), the subject, "author · relative date", and a "Back to the Change" button. Accent background at 8%.
  - Then that commit's diff of the file, or "Binary File", or a spinner while it loads.
- **Reaction:**
  - `git show --format= --no-color --no-ext-diff -M --diff-merges=first-parent [-U1000000] <sha> -- <path>`. The large context is used in Full mode or with Before/After, and the file is shown whole.
  - The Markdown preview loads `git show <sha>:<path>`.
  - "Back to the Change" clears the pick.
- **Edge cases:**
  - Changing the file clears the pick.
  - A stale response for a pick that has since changed is ignored.
  - Switching to Blame and back to History brings the pick back.
- **Source:** FileInspector.swift:101-107, FileInspector.swift:113-152

### FI-05 Resize the History panel
- **Trigger:** Drag the separator (9 pt hit area; resize cursor on hover). Double-click it to reset.
- **Reaction:** The width is kept between 260 and 700 and persisted in the `filePanelWidth` default. Double-click sets it back to 340.
- **Source:** FileInspector.swift:74, FileInspector.swift:87-99

### FI-06 Where the panel is available
- **Trigger:** Render only.
- **Render:**
  - In the Compact DiffPane and the Full-mode file pane, for tracked files that are not untracked or added.
  - Untracked and added files: no buttons and no panel. The setting comes back on the next tracked file.
  - Submodules and conflicted files: no panel.
  - In History or PR views a FileTarget can carry a rev, and blame then runs at that rev.
- **Source:** ChangesView.swift:503, ChangesView.swift:511, ChangesView.swift:775, ChangesView.swift:787, FileInspector.swift:5-9

---

# 4. Diff viewer (modes, keys, intra-line, syntax, theme, large/binary/image, LFS, submodules, change overview)

**Key fact up front: there is no split (side-by-side) diff mode.** The vault says "Split view removed" (index.md:222), and the code has only Diff / Before / After. Vault line 56 still says "unified/split layouts", which is out of date. There is also no whitespace-ignore option, no context expansion except Full mode, no "show anyway" gate for large diffs, and no image diff for images that aren't in LFS.


I read all of DiffTableView.swift, Diff.swift, Syntax.swift, EditorTheme.swift, ChangeOverview.swift, LFS.swift and Submodules.swift line by line. I also read the parts that host and wire the diff: ChangesView.swift:1-805, Model.swift:370-479, App.swift:80-145, Shortcuts.swift:90-112, Git.swift:267-277, History.swift:535-617, PullRequestsView.swift:500-642, FileInspector.swift:50-152, PullModel.swift:381-425 and Conversation.swift:516-548. The vault note is Projects/loupe/index.md.

**Key fact up front: there is no split (side-by-side) diff mode.** The vault says "Split view removed" (index.md:222), and the code has only Diff / Before / After. Vault line 56 still says "unified/split layouts", which is out of date. There is also no whitespace-ignore option, no context expansion except Full mode, no "show anyway" gate for large diffs, and no image diff for images that aren't in LFS.

## Diff loading and states

### DV-01 Load a file's diff (Changes, Compact)
- **Trigger:** Select a row in the Changes file list (the plain-click / ⌘-click selection binding), or `model.select(id)`.
- **Preconditions:** A worktree with at least one changed file.
- **Render:** The pane keeps the previous diff on screen until the next one is parsed, so there is no spinner flash. Then it shows DiffHeader, a Divider, and the body inside WithFilePanel.
- **Reaction:**
  - Tracked file: `git diff --no-color --no-ext-diff -M -U3 [--cached] -- <path> [<origPath>]`.
  - Untracked file: `git diff --no-color --no-ext-diff --no-index -U3 -- /dev/null <path>`, where exit 1 is allowed.
  - When Full mode (`prFullMode`) is on or `diffSide` ≠ diff, the context is `-U1000000` and the result goes through `PullRequestModel.wholeFile`. This does not apply to untracked, added or deleted files.
  - If the reloaded diff has the same path, line texts and headerLines, `diff` is not replaced, so scroll and selection survive FS-watch refreshes.
- **Edge cases:**
  - `diffTask` is cancelled when the selection changes.
  - A file with both staged and unstaged entries diffs according to the selected entry's `staged` flag.
  - `apply` refuses to act if the shown diff's path doesn't match the selected file, which protects against a stale diff.
- **Source:** Model.swift:388-407, Git.swift:267-277, ChangesView.swift:492-535

### DV-02 Diff pane empty and loading states
- **Trigger:** The selection state of the Changes list.
- **Preconditions:** (a) a file is selected but its diff isn't loaded; (b) the worktree is clean; (c) files exist but none is selected.
- **Render:**
  - (a) ProgressView, centered.
  - (b) ContentUnavailableView "No Changes" (checkmark.seal), with "Edits in this worktree show up here as you make them."
  - (c) "No File Selected" (doc.text.magnifyingglass).
  - Elsewhere: in History, a commit with no file selected but still loading shows ProgressView, and with no files shows "No File Changes". In a PR, "Loading diff…" or "No File Selected".
- **Reaction:** None.
- **Edge cases:** The file list overlay separately shows "Working Tree Clean" (CH covers it).
- **Source:** ChangesView.swift:536-542, History.swift:565,588-592, PullRequestsView.swift:517-518

### DV-03 Diff header bar
- **Trigger:** Any diff shown (Changes, Full-mode tab, History, PR compact or full, submodule).
- **Preconditions:** Any diff.
- **Render:** An HStack on `.bar` background containing, in order:
  - FileIcon.
  - For a rename or copy: the old path (secondary), then an arrow.right icon.
  - The path (medium weight, head-truncated, selectable).
  - `+N` in green and `−N` in red, both monospaced digits. Hidden when `showCounts` is false (submodule, unchanged file in Full mode).
  - Spacer.
  - The Diff/Before/After segmented picker (DV-16). Shown only when additions > 0 and deletions > 0, and not while a Markdown file has Preview on.
  - Blame and File History icon buttons (DV-37/38), when there is a `file` target.
  - A divider before the actions.
  - The "Preview" checkbox for .md/.markdown/.mdx (DV-44).
  - The actions, at small control size.
- **Reaction:** See the individual controls.
- **Edge cases:**
  - The `file` target is nil (no blame/history) for untracked/added files in Changes, a deleted file or a stash in History, PR diffs, and submodules.
  - Counts come from the parsed diff, so in Full mode they still count only the changed lines.
- **Source:** ChangesView.swift:546-606, 502-509, 774-785; History.swift:571-572; PullRequestsView.swift:531-533, 555-563; Submodules.swift:185

### DV-04 File-level actions in the header
- **Trigger:**
  - Changes, unstaged file: "Discard…" button and "Stage File" (borderedProminent).
  - Changes, staged file: "Unstage File".
  - PR: "Viewed" checkbox.
  - PR Full mode, unchanged file: an "Unchanged" caption.
- **Preconditions:** A file selected.
- **Render:** Small buttons at the right of the header.
- **Reaction:** Action: see CH (stage `add -A`, unstage `restore --staged`, discard confirmation dialog). Viewed: see PR.
- **Edge cases:** History headers have no actions (EmptyView).
- **Source:** ChangesView.swift:504-508, 778-784; PullRequestsView.swift:532, 558-562; History.swift:572

## Line rendering

### DV-05 Line row layout
- **Trigger:** Any diff with hunks.
- **Preconditions:** Added, removed and context lines.
- **Render:**
  - **Gutter.** Diff mode has two number columns, old then new. Each column is `max(3, digits(maxLineNo)) × charWidth + 12` wide, numbers are right-aligned 6 px from the column edge, in smallFont (fontSize − 1.5) with the lineNumber color, on `gutterBG` (foreground at 3%).
  - **Missing numbers.** An added line has no old number; a removed line has no new number.
  - **Marker.** "+" in addMark or "−" (U+2212) in delMark is drawn at the start of the text area, in Diff mode only.
  - **Row background.** Full width after the blame area: addBG or delBG (DV-43). Context rows have none.
  - **Text.** Starts at gutter + 4 + 12.
  - **Row height.** `round(fontSize × lineHeightFactor)`, 18 by default.
  - **Table.** No grid, no row selection highlight, no header, iBeam cursor, both scrollers auto-hiding, theme background.
- **Reaction:** None.
- **Edge cases:**
  - The column is never narrower than the viewport, so backgrounds span it (fitColumn).
  - Line numbers are parsed from `@@ -a,b +c,d @@`. The counts after the commas are ignored; a missing or garbled number becomes 0.
- **Source:** DiffTableView.swift:41-82, 295-298, 394-400, 960-1034; Diff.swift:113-139, 163-171

### DV-06 Hunk header rows and hunk buttons
- **Trigger:** A diff with `@@` hunks, in Diff mode.
- **Preconditions:** Multiple hunks, some with function context after the second `@@`.
- **Render:**
  - Row height is lineHeight + 8, background `hunkBG` (the theme's *function* token color at 14% dark / 9% light).
  - The label is the full raw header text ("@@ -12,7 +12,9 @@ func x") in smallFont, secondary color, tail-truncated, 12 px from the left.
  - To its right are small push buttons, depending on the host:
    - Changes, unstaged: "Discard" (systemRed tint), then "Stage Hunk".
    - Changes, staged: "Unstage Hunk".
    - Changes, untracked: none.
    - History, PR, File History and Settings preview: no buttons (`hunkActions` empty).
- **Reaction:**
  - The button calls `onHunkAction(action, all line indices of the hunk)`.
  - Discard first shows an NSAlert "Discard these changes?" / "The change is removed from the working tree. Edit › Undo brings it back." with buttons [Discard (destructive), Cancel].
  - Then `git apply [--cached] [--reverse] --whitespace=nowarn -` runs. Action: see CH.
- **Edge cases:**
  - No hunk rows in Before/After, or in Full mode, because wholeFile strips them.
  - A reused row keeps its buttons if the action set is unchanged.
- **Source:** DiffTableView.swift:285, 396, 410-416, 1099-1146; ChangesView.swift:515-527; Model.swift:445-463; EditorTheme.swift:83

### DV-07 "\ No newline at end of file" (meta lines)
- **Trigger:** A diff where either side lacks a trailing newline.
- **Preconditions:** A file whose last line has no `\n`, on the old side, the new side, or both.
- **Render:** A row with the literal text `\ No newline at end of file` in metaForeground (the foreground at 60% alpha). It has no line numbers, no marker, no background and no syntax colors.
- **Reaction:** In line or hunk patches, the meta line is kept only if the line before it was kept.
- **Edge cases:**
  - Counted in rows; it is not a change for n/p, the ruler or the context-menu line actions.
  - It can be selected and copied.
- **Source:** Diff.swift:113, 125-127, 54; DiffTableView.swift:205; Syntax.swift:106

### DV-08 Tabs, very long lines, horizontal scroll
- **Trigger:** Display.
- **Preconditions:** (a) lines with tab characters; (b) a line over 1500 characters, such as minified JS.
- **Render:**
  - Tabs are drawn as 4 spaces.
  - Lines are cut at 1500 UTF-16 characters, `Theme.maxDrawnChars`.
  - The column width is `min(maxLen, 1500) × charWidth + 24` plus the gutter, and you scroll horizontally.
- **Reaction:** None.
- **Edge cases:**
  - Intra-line highlight is skipped for any line containing a tab.
  - Find, copy and syntax all work on the displayed text, so they see truncated, tab-expanded text (see BROKEN).
- **Source:** Syntax.swift:91-95; DiffTableView.swift:212, 297, 344-348, 935

### DV-09 CRLF lines
- **Trigger:** Display.
- **Preconditions:** A file with `\r\n` endings, or a change from LF to CRLF.
- **Render:** The parser splits on `\n` only, so `\r` stays at the end of the line text and draws as nothing.
- **Reaction:** None.
- **Edge cases:**
  - A line that changes only from LF to CRLF shows as a removed/added pair. Intra-line marks a 1-character span at the end (the invisible `\r`), so the pair shows no visible highlight.
  - Patches keep the `\r`.
  - The tokenizer skips `\r`.
- **Source:** Diff.swift:176-188; DiffTableView.swift:771-795

### DV-10 Intra-line (word) highlight
- **Trigger:** Display in Diff mode.
- **Preconditions:** A run of N removed lines immediately followed by exactly N added lines.
- **Render:** Each removed/added pair gets the span between their common prefix and common suffix painted with delHL or addHL as a text background.
- **Reaction:** None; computed on the main thread at rebuild.
- **Edge cases:**
  - Only 1:1 runs (dels.count == adds.count) of 50 pairs or fewer.
  - Both lines must be under 1000 UTF-16 units.
  - No highlight when the prefix and suffix are both 0 (the whole line changed).
  - No highlight on a side whose differing span is empty (pure insertion shows only on the added line).
  - Lines containing a tab are skipped.
  - Not shown in Before/After.
  - Cached per file uid for the last 3 files (switching Changes/History/PRs doesn't recompute).
  - The Settings preview has a 1-removed / 2-added run, so it shows no intra-line highlight.
- **Source:** DiffTableView.swift:151, 154-165, 212-214, 250-257, 771-795

### DV-11 Syntax highlighting
- **Trigger:** Display.
- **Preconditions:** File extensions from the table below; also a file with no recognized language.
- **Render:**
  - Colors arrive shortly after the text: a detached task tokenizes, then visible rows redraw.
  - Token kinds: keyword, string, comment, number, type (capitalized identifier, or `$var` in shell/PHP), function (identifier followed by `(`), macro (Rust `name!`), attribute (`@x`, `#[...]`, `#include`, Rust lifetimes `'a`), property (keys in YAML/TOML/JSON/CSS/HCL).
  - An unknown extension gets plain foreground text with no spans.
- **Reaction:** None.
- **Edge cases:**
  - Languages by extension:

    | Language | Extensions / names |
    |---|---|
    | Rust | rs |
    | Swift | swift |
    | Go | go |
    | TypeScript/JavaScript | ts, tsx, js, jsx, mjs, cjs, mts, cts |
    | Python | py, pyi |
    | C family | c, h, cc, cpp, cxx, hpp, m, mm |
    | JVM | java, kt, kts, scala, cs, dart |
    | Nix | nix |
    | SQL (case-insensitive keywords) | sql |
    | Shell | sh, bash, zsh, fish, envrc; also `Dockerfile`, `Dockerfile.*`, `Makefile`, `justfile` |
    | YAML | yml, yaml |
    | TOML | toml, ini, cfg, conf |
    | JSON | json, jsonc, json5 |
    | CSS | css, scss, sass, less |
    | Ruby | rb |
    | Lua | lua |
    | Solidity | sol |
    | Proto/GraphQL | proto, graphql, gql |
    | PHP | php |
    | HCL | tf, tfvars, hcl |

  - Markdown has no highlighting.
  - Block comments and multi-line strings carry state across lines. Old and new sides keep separate state: a context line tokenizes with the old state and copies it to both sides. A hunk header resets both.
  - An unterminated string ends at end of line.
  - Highlighting is cached per uid for the last 3 files. Results for a file no longer shown are dropped.
- **Source:** Syntax.swift:1-225; DiffTableView.swift:152-153, 207-211, 263, 322-342

### DV-12 Binary file
- **Trigger:** Select a binary file.
- **Preconditions:** A diff containing "Binary files … differ" or "GIT binary patch". For a PR full file or a whole file in Full mode: a NUL byte in the first 8000 bytes.
- **Render:** ContentUnavailableView "Binary File" (doc.zipper). The header shows +0 −0.
- **Reaction:** None.
- **Edge cases:**
  - Untracked binary files are detected the same way, through `--no-index`.
  - The History tree row shows a "binary" tag (FileTree.swift:173).
- **Source:** Diff.swift:149; ChangesView.swift:512, 789; History.swift:575; PullRequestsView.swift:535, 566; FileInspector.swift:131; PullModel.swift:393

### DV-13 Image files (not in LFS)
- **Trigger:** Select a changed PNG or JPG that is not an LFS pointer.
- **Preconditions:** A binary image committed directly.
- **Render:** Same as DV-12, "Binary File". There is no image preview, side-by-side, onion-skin or swipe view.
- **Reaction:** None.
- **Edge cases:** Image previews exist only for LFS objects (LF-04).
- **Source:** ChangesView.swift:512; LFS.swift:236-237

### DV-14 Renamed, copied, mode-only and empty files
- **Trigger:** Select such a file.
- **Preconditions:** (a) a rename with edits; (b) a 100% rename; (c) a chmod only; (d) a new empty file; (e) a copy (`-C`).
- **Render:**
  - **Rename:** the header shows "old → new". Similarity is parsed but never shown in the diff.
  - **History or PR compact, rename or mode change with no lines:** "No Content Changes" (doc) with "Mode change or rename only."
  - **Changes compact, same cases:** a blank, empty table under the header, with no empty state (see BROKEN).
  - **Full-mode tab or PR full file with no lines:** "Empty File" (doc).
- **Reaction:** Rename detection is `-M`. `origPath` is passed to git diff.
- **Edge cases:**
  - Paths come from `diff --git … b/x`, overridden by `---`/`+++`, `rename from/to` and `copy from/to`.
  - `/dev/null` means no side.
  - A trailing `\t<timestamp>` (from --no-index) is stripped.
  - The `a/` and `b/` prefixes are stripped.
- **Source:** Diff.swift:100-111, 140-161; ChangesView.swift:502, 562, 790; History.swift:577; PullRequestsView.swift:536, 567

### DV-15 Large diffs
- **Trigger:** Open a huge file or diff.
- **Preconditions:** A 150k-line diff or file; a file with 5000+ changed lines; a PR with 5000 files.
- **Render:**
  - It opens at once with no warning or "show anyway". The NSTableView is virtualized and rows draw themselves.
  - Syntax colors arrive later.
  - The vault says a 150k-line file opens in under 1 s, and the parser does 200k lines in about 40 ms.
- **Reaction:** None.
- **Edge cases and thresholds:**
  - Drawn-line cache: 1500 lines or 1M UTF-16 units.
  - Intra-line: runs of 50 pairs or fewer, lines under 1000 characters.
  - Display truncation at 1500 characters per line.
  - Comment heights are cached.
  - For PR diffs too large for the forge (GitHub 406 over 300 files or 20k lines), see PR (vault).
- **Source:** DiffTableView.swift:15-16, 172-176, 780-783, 935; vault index.md:50

## Diff modes

### DV-16 Diff / Before / After switch
- **Trigger:** The segmented picker "Diff | Before | After" in the diff header (help: "The changes, the file before them, or the file after them").
- **Preconditions:** The file has additions > 0 **and** deletions > 0. Otherwise the picker is hidden and the view is forced to Diff.
- **Render:**
  - **Before:** only context and removed lines, one gutter column of old numbers, no hunk rows, no red/green backgrounds, no markers, no intra-line highlight. Syntax colors stay.
  - **After:** the same with added lines and new numbers.
  - Inline comments for the other side are hidden, and outdated comments are not appended.
  - The change ruler shows no marks.
- **Reaction:**
  - Persisted in the `diffSide` UserDefault, shared by every host.
  - In Changes, `onChange(side)` calls `loadDiff()`, which reloads with `-U1000000` + wholeFile.
  - In History it loads `git show --format= --no-color --no-ext-diff -M --diff-merges=first-parent -U1000000 <sha> -- <old> <new>`, or for a stash `git stash show -p --no-color --no-ext-diff -M -U1000000 <stash> --`.
  - In a PR, Before/After forces the full-file view.
- **Edge cases:**
  - A file with one side only always shows as Diff, even if the default is Before or After.
  - The selection is not cleared on the same path.
  - The context-menu Stage/Discard Line or Hunk items still appear in Before/After in Changes, and "Hunk" is then the whole file (see BROKEN).
- **Source:** ChangesView.swift:569-578, 300; DiffTableView.swift:23, 27, 91-94, 256-257, 285-286, 292, 376, 568, 979, 1003, 1028; Model.swift:394-399; History.swift:579-583, 607-617; PullRequestsView.swift:514-515

### DV-17 Scroll anchoring on rebuild
- **Trigger:** Switch Diff, Before and After on the same file, or the same file reloads (Full mode, an edit on disk).
- **Preconditions:** A file scrolled to the middle.
- **Render:**
  - The first visible diff line, by new line number (or old when there is no new one), stays at the same offset from the top.
  - A different file starts at the top, its selection cleared.
  - Scroll is clamped after layout, so the view is never past the end (DV-45).
- **Reaction:** None.
- **Edge cases:** Same path but a different uid also anchors.
- **Source:** DiffTableView.swift:94-120, 429-436

### DV-18 Compact / Full mode (whole file with changes inline)
- **Trigger:**
  - Toolbar segmented control with rectangle.compress.vertical ("Compact: the changes with a few lines around them") and rectangle.expand.vertical ("Full: whole files with the changes inline (⌥⌘F)"). Shown only while a diff is visible (`DiffControls.full`).
  - Menu Review › Compact / Full Mode, ⌥⌘F (rebindable).
- **Preconditions:** A file selected.
- **Render:**
  - Full mode shows the whole file: hunk rows removed, every line context except the changes, and the change ruler (DV-19) on.
  - In Changes, Full replaces the file list with the worktree tree and the pane with tabs (see Code browser).
  - In History and PR, the same pane shows the whole file.
- **Reaction:**
  - `prFullMode` default.
  - Changes reloads with `-U1000000`. History and PR load as in DV-16.
- **Edge cases:**
  - Untracked, added and deleted files are already whole and keep their hunk.
  - wholeFile sets every `hunkIndex` to 0 and makes one hunk.
  - The toolbar control is hidden in Overview and when no file is selected.
- **Source:** ChangesView.swift:72-81, 279-281, 289-294; App.swift:98; Shortcuts.swift:71, 104; Model.swift:391-399; PullModel.swift:381-387; History.swift:581-583, 598-599

### DV-19 Change ruler (marks beside the scroller)
- **Trigger:** Shown when `changeRuler` is true: Full mode in Changes (WorktreeFilePane, always), History, PR full file (always), and the File-History picked diff when Full mode is on.
- **Preconditions:** A whole file with several separate change runs (added only, removed only, mixed).
- **Render:**
  - An always-visible track (not an overlay) on the theme background with a 1 px separator on the left.
  - One rounded mark per run of changed rows at its proportional position: green for adds only, red for removes only, orange for mixed. Minimum height 3 px, inset 3 px.
  - The knob is a translucent rounded rectangle.
  - Hovering a mark shows the pointing-hand cursor and a tooltip "Line N: +a −r" (zero parts left out). Elsewhere the cursor is an arrow.
- **Reaction:** Clicking a mark (hit area padded 3 px) scrolls that run's first row to a third of the way down and selects that whole row. Clicking the knob still drags; the rest of the track works as usual.
- **Edge cases:**
  - Comment rows inside a run don't split it.
  - No marks in Before/After.
  - The ruler swaps in and out without rebuilding the table.
  - Mark positions are recomputed when the slot or table height changes.
  - Marks redraw on a theme change.
- **Source:** DiffTableView.swift:37, 64, 88-90, 121-125, 374-392, 800-870

### DV-20 Unified vs split (side-by-side): not present
- **Trigger:** None. Testers should confirm the GPUI build doesn't add one, or that it matches deliberately.
- **Preconditions:** None.
- **Render:** Unified only. Before/After replaces the split view.
- **Reaction:** None.
- **Edge cases:** None.
- **Source:** DiffTableView.swift:22-23; vault index.md:219, 222

## Keyboard, mouse and selection

### DV-21 n / p: next and previous change
- **Trigger:** Keys `n` / `p` while the diff table is first responder (click into the diff first).
- **Preconditions:** A diff with several hunks, and several change runs inside one hunk.
- **Render:**
  - The stop is either a hunk header (scrolled to the very top) or the first row of a run of added/removed lines.
  - A run is scrolled so that 3 lines of context show above it.
- **Reaction:** Scroll only, no selection.
- **Edge cases:**
  - The search starts 3 rows below the top visible row, forward from there or backward.
  - It does not wrap.
  - A change directly under its hunk header is not a second stop, but a later run inside the same hunk is a stop in every view, not only whole files.
  - Comment rows are not changes.
  - The key is matched on `charactersIgnoringModifiers` with no modifier check, so ⌃n and ⌥n also jump.
  - Shift+N is not handled.
- **Source:** DiffTableView.swift:744-767, 875-878

### DV-22 v: Mark Viewed & Next (from the diff)
- **Trigger:** Key `v` in a focused diff table. Also menu Review › Mark Viewed & Next, ⇧⌘V.
- **Preconditions:** A PR's Files tab.
- **Render:** The file's Viewed box gets checked and the next file opens (see PR).
- **Reaction:** Posts `.markViewed`. Only PullRequestsView listens, calling `markViewedAndAdvance()`.
- **Edge cases:** In Changes and History, `v` does nothing.
- **Source:** DiffTableView.swift:879; App.swift:97, 143; Shortcuts.swift:102; PullRequestsView.swift:305

### DV-23 Next and previous file
- **Trigger:** Menu Review › Next File (⌥↓) and Previous File (⌥↑).
- **Preconditions:** Several changed files.
- **Render:** The adjacent file's diff.
- **Reaction:** Posts `.moveFile ±1`. Changes clamps within `model.listed`; History and PR call `move(d)`.
- **Edge cases:** It stops at the ends, with no wrap.
- **Source:** App.swift:95-96; Shortcuts.swift:100-101; ChangesView.swift:306-310; History.swift:336; PullRequestsView.swift:304

### DV-24 Click and drag text selection
- **Trigger:** Mouse down and drag in the diff.
- **Preconditions:** Several lines.
- **Render:**
  - Character-level selection drawn by the rows: selectedTextBackgroundColor when the table is first responder in the key window, unemphasizedSelectedTextBackgroundColor otherwise.
  - A fully selected line shows one extra column (its newline).
  - Hunk and comment rows show no selection.
- **Reaction:**
  - A click makes the table first responder and places a caret (empty selection).
  - Dragging extends the selection and autoscrolls.
  - The column is `round((x − textX) / charWidth)`, clamped to the line length.
  - A point above the table maps to row 0; below it, to the last row.
- **Edge cases:**
  - The selection is cleared when another path loads.
  - It is stored by row index, so a same-path rebuild can leave it on different lines.
- **Source:** DiffTableView.swift:444-458, 577-587, 616-644, 653-662, 888-896, 1014-1019

### DV-25 Double-click (word) and triple-click (line)
- **Trigger:** Double-click or triple-click.
- **Preconditions:** Identifier text and punctuation.
- **Render:**
  - Double-click selects the identifier: characters in [0-9A-Za-z_] or any non-ASCII character.
  - On a non-word character it selects that one character.
  - Triple-click (or more) selects the whole line.
- **Reaction:** Selection only.
- **Edge cases:** A click left of the text (over the gutter) gives a negative column, so no word is found.
- **Source:** DiffTableView.swift:589-603, 624-633

### DV-26 Select all
- **Trigger:** ⌘A or Edit › Select All, with the diff focused.
- **Preconditions:** Any diff.
- **Render:** Everything is selected, from row 0 to the end of the last row.
- **Reaction:** Selection only.
- **Edge cases:** Nothing happens with zero rows.
- **Source:** DiffTableView.swift:646-651, 886

### DV-27 Copy
- **Trigger:** ⌘C (handled in keyDown and the `copy(_:)` responder), Edit › Copy, or the context menu "Copy".
- **Preconditions:** A selection spanning lines, including hunk and comment rows.
- **Render:** None.
- **Reaction:** The pasteboard gets the selected text of line rows only, joined with `\n`. Markers, line numbers and hunk headers are not included.
- **Edge cases:**
  - Tabs are copied as 4 spaces, and lines are truncated at 1500 characters (see BROKEN).
  - With no selection it does nothing.
  - The context menu always shows "Copy", even with no selection.
- **Source:** DiffTableView.swift:717, 720-742, 880, 885

### DV-28 Focus redraw
- **Trigger:** Focus moves into or out of the diff, or the window becomes key or resigns.
- **Preconditions:** An active selection.
- **Render:** The selection color switches between emphasized and unemphasized.
- **Reaction:** Visible rows redraw.
- **Edge cases:** None.
- **Source:** DiffTableView.swift:922-923, 1015

### DV-29 ⌘-hover link and ⌘-click Go to Definition
- **Trigger:** Hold ⌘ over an identifier, then ⌘-click once.
- **Preconditions:** A host with `onNavigate`: Changes compact, Changes Full, PR full file. Not History, PR compact, File History or the preview.
- **Render:**
  - While ⌘ is held, the identifier under the mouse gets a 1.2 px underline in linkColor and the pointing-hand cursor. It updates on mouse move and on pressing or releasing ⌘.
  - Words starting with a digit are not links.
- **Reaction:** The identifier is selected, then `onNavigate(word, .definition)`. Action: see Code browser (navigate, NavResults panel, Full-mode jump).
- **Edge cases:**
  - Only a single click navigates; ⌘ double-click selects normally.
  - Leaving the table clears the hover.
- **Source:** DiffTableView.swift:35, 169, 219-230, 605-622, 898-920, 983-987; ChangesView.swift:528, 793; PullRequestsView.swift:573

### DV-30 Diff context menu (right-click)
- **Trigger:** Right-click a row.
- **Preconditions:** Combinations of the following: an identifier under the cursor, a commentable host (PR), Changes hunk actions, a selection covering several changed lines.
- **Render:** Items in this order:
  1. When `onNavigate` exists and a word is under the cursor (cursor x, the clicked row's midline): "Go to Definition of “w”", "Find References to “w”", then a separator.
  2. On a line row:
     - "Add Comment…" when `onAddComment` is set and `canComment(line)` allows it.
     - Line actions. The lines are the changed lines of the selection if the clicked line is inside the selection; otherwise the clicked line if it is a change. For each host action, in host order: "Stage Line" / "Stage N Lines", "Unstage Line" / "Unstage N Lines", "Discard Line…" / "Discard N Lines…". Changes unstaged order is Discard first, then Stage.
     - Hunk actions: "Stage Hunk" / "Unstage Hunk" / "Discard Hunk…".
     - A separator if anything was added.
  3. "Copy" (always).
- **Reaction:**
  - Line and hunk actions call `onHunkAction(a, set)`. Discard shows the same NSAlert as DV-06, then `git apply` with a patch built per line (`patch(selecting:reverse:)`). Action: see CH.
  - Go to Definition and Find References: see Code browser.
  - Add Comment: DV-35.
- **Edge cases:**
  - On a hunk header row only "Copy" appears.
  - A context or meta line outside the selection gets no line actions but still gets the hunk actions.
  - Untracked files have no actions.
  - In Before/After or a whole file the hunk is the entire file.
- **Source:** DiffTableView.swift:58-59, 688-724; Diff.swift:35-70; ChangesView.swift:516-527

### DV-31 Reveal a line (a jump into a diff)
- **Trigger:**
  - Overview fact click in History or PR (CO-20).
  - Navigation results, back and forward (Code browser).
  - `DiffReveal` with a new id.
- **Preconditions:** A target new-side line number inside the diff.
- **Render:** That line (new side, not a removed row) is scrolled to a third of the way down and selected whole.
- **Reaction:** Deferred one main-loop turn so the rows exist.
- **Edge cases:**
  - A line not present (outside the hunks) does nothing.
  - The same reveal id isn't repeated.
  - In Changes, the reveal applies only when the browser's selected path equals the diff's path.
- **Source:** DiffTableView.swift:19-20, 126-130, 232-248; ChangesView.swift:530; History.swift:540, 554, 582

### DV-32 Find in file (⌘F)
- **Trigger:** ⌘F anywhere in the window, even while the file list has focus. It is a local key monitor. There is no menu item.
- **Preconditions:** A visible (not hidden) diff table in that window.
- **Render:**
  - A find bar above the content: a search field (placeholder "Find in file", 260 wide), a count label, ‹ › chevrons, and "Done". It uses windowBackground with a bottom hairline.
  - Matches get rounded highlights: the current one systemOrange at 60%, the others systemYellow at 30%.
  - The count reads "i of n", "No results" for 0, or empty for an empty query.
- **Reaction:**
  - The field gets focus with its text selected.
  - The search is incremental on every keystroke, case- and diacritic-insensitive, over displayed text of line rows only (not hunk headers or comments).
  - The current match starts at the first match at or below the top visible row.
  - It scrolls to the match: centered vertically if off screen, and horizontally so it sits about a third in if it is past the edge.
  - Reopening with an old query searches again.
- **Edge cases:**
  - When the file changes under an open bar, the search re-runs without scrolling.
  - Finds work on truncated, tab-expanded text.
  - With several diff tables in one window, the first monitor wins.
- **Source:** DiffTableView.swift:460-565, 1020-1027, 1037-1097

### DV-33 Find next, previous and close
- **Trigger:** ⌘G (next) or ⇧⌘G (previous), only while the find bar is visible. Also Return (next) and ⇧Return (previous) in the field; the chevrons (tooltips "Previous (⇧⌘G)" / "Next (⌘G)"); Esc or "Done" to close.
- **Preconditions:** Several matches.
- **Render:** The current highlight moves, wrapping around, and the count updates.
- **Reaction:** Close hides the bar, clears the matches and highlights, and gives focus back to the table. The query text is kept.
- **Edge cases:**
  - ⌘G with the bar closed passes through.
  - With zero matches, stepping does nothing.
- **Source:** DiffTableView.swift:472-489, 507-512, 547-555, 1053-1058, 1087-1096

## Inline comments (rendering in the diff)

### DV-34 Inline comment rows
- **Trigger:** Pass comments in (PR compact or full file).
- **Preconditions:** Comments on a new line, an old line, a context line, a draft, and an outdated comment whose line is no longer in the diff.
- **Render:**
  - A box 620 wide at x = 56 with corner radius 8, a tinted background (7%) and border (35%).
  - The tint is the accent color for posted comments and orange for drafts.
  - The author is 11 pt semibold; drafts read "<author> · Pending" and show a trash button (tooltip "Delete draft").
  - The body is wrapping and selectable, 12.5 pt.
  - Row height is the body height + 46.
- **Reaction:** None.
- **Edge cases:**
  - Placement: newLine > 0 goes on the new line; otherwise the old line.
  - A context line collects comments for both its new and old numbers.
  - Comments not matched to a line go at the end, Diff mode only.
  - Before/After drop the other side's comments.
- **Source:** DiffTableView.swift:4-11, 136, 261-292, 397, 406-409, 1148-1217

### DV-35 Add Comment popover
- **Trigger:** Context menu "Add Comment…" (DV-30).
- **Preconditions:** A PR diff. `canComment` may restrict it in Full mode, and unchanged files in a PR full file get none.
- **Render:**
  - A transient popover near the row with the caption "<path> · Line N", or "Line N (old)" for a removed line.
  - A TextEditor 380×110 that is focused after 0.05 s.
  - "Cancel" (Esc) and "Add Comment" (⌘Return, borderedProminent, disabled while the text is only spaces).
- **Reaction:** The trimmed body goes to `onAdd(newLine, oldLine, body)`, which appends a draft (see PR). Cancel closes the popover.
- **Edge cases:**
  - Not offered on hunk or meta lines.
  - A removed line sends oldLine only; other lines send newLine only.
  - Text made only of newlines enables the button but adds nothing (see BROKEN).
- **Source:** DiffTableView.swift:671-686, 1230-1259; PullRequestsView.swift:540, 570-572

### DV-36 Delete a draft comment
- **Trigger:** The trash icon on a Pending comment row.
- **Preconditions:** A draft exists.
- **Render:** The row disappears.
- **Reaction:** `onDeleteDraft(id)` removes it from `pull.drafts`.
- **Edge cases:** Posted comments have no button.
- **Source:** DiffTableView.swift:408, 1175-1178, 1200, 1216; PullRequestsView.swift:541

## Blame and File History in the diff

### DV-37 Blame gutter
- **Trigger:** Header button with the person.text.rectangle icon (tooltip "Blame: who last changed each line"). A second click turns it off. Persisted in the `filePanel` default.
- **Preconditions:** A tracked file with some commits and uncommitted lines; a `file` target is present.
- **Render:**
  - A 210 px column left of the line numbers.
  - On the first line of each run from one commit: "<sha7>  <author>  · <abbreviated relative age>", or "Not committed" for the all-zero sha.
  - The shading alternates per run (labelColor at 7% / 3.5%).
  - The active button is accent-tinted with a 15% background.
- **Reaction:**
  - `git blame --porcelain [<rev>] -- <path>`; History passes rev = commit sha.
  - Blame reloads when the diff uid changes (an edit).
- **Edge cases:**
  - Only new-side lines get blame. Removed rows and Before rows have none.
  - Lines beyond the blame length get nothing.
  - Blame and History are exclusive.
- **Source:** DiffTableView.swift:28, 147-149, 299, 302-318, 992-1001; ChangesView.swift:579-586, 597-605; FileInspector.swift:18-19, 53-65, 103, 108-112

### DV-38 File History panel (entry from the diff header)
- **Trigger:** Header button with the clock.arrow.circlepath icon ("File History").
- **Preconditions:** A tracked file with history.
- **Render:**
  - A commit list on the left, resizable from 260 to 700 by a drag handle; double-click the handle to reset to 340. The width is persisted in `filePanelWidth`.
  - Picking a commit replaces the diff with a bar showing the clock icon, short sha, subject, "author · relative date" and a "Back to the Change" button.
  - Below the bar is that commit's diff, or "Binary File", or a ProgressView.
- **Reaction:** `git show --format= --no-color --no-ext-diff -M --diff-merges=first-parent [-U1000000 if Full or Before/After] <sha> -- <path>`. The list internals are covered elsewhere (FileInspector).
- **Edge cases:**
  - The pick resets when the file changes.
  - There is no LFS handling here (see BROKEN).
- **Source:** FileInspector.swift:65-152; ChangesView.swift:583

## Theme, fonts and settings (EditorTheme.swift)

### DV-39 Editor preset
- **Trigger:** Settings › editor pane › "Preset" picker: Xcode, VS Code, JetBrains, GitHub. A divider and "Custom" appear only when the current settings match no preset.
- **Preconditions:** Default settings, which match Xcode.
- **Render:** The pickers below update.
- **Reaction:** `apply` sets light, dark, font, size and line height in one batch, then posts one `editorStyleChanged`.
- **Edge cases:** Presets:
  - Xcode: xcode-light / xcode-dark, SF Mono, 12, 1.5.
  - VS Code: Menlo, 12, 1.5.
  - JetBrains: intellij-light / jetbrains-dark, JetBrains Mono, 13, 1.45.
  - GitHub: SF Mono, 12, 1.65.
- **Source:** EditorTheme.swift:88-102, 150-159, 199-203

### DV-40 Light and dark code themes
- **Trigger:** The "Light theme" picker (7 themes) and "Dark theme" picker (12 themes). Each entry shows the name and a strip of 7 color swatches.
- **Preconditions:** Switch system appearance between Light and Dark.
- **Render:**
  - Footer: "Code follows the app's appearance: the light theme in Light mode, the dark one in Dark mode."
  - Every open diff restyles; colors are dynamic per appearance, so an appearance change needs no rebuild.
- **Reaction:** Persisted in `editorLightTheme` and `editorDarkTheme`. The `.editorStyleChanged` notification makes every DiffTableView rebuild, refit and reload.
- **Edge cases:**
  - An unknown id falls back to xcode-light or xcode-dark.
  - Dark themes: Xcode Default (Dark), VS Code Dark Modern, JetBrains Dark, JetBrains Darcula, GitHub Dark, One Dark, Dracula, Monokai, Tokyo Night, Catppuccin Mocha, Nord, Solarized Dark.
  - Light themes: Xcode Default (Light), VS Code Light Modern, IntelliJ Light, GitHub Light, One Light, Catppuccin Latte, Solarized Light.
- **Source:** EditorTheme.swift:23-68, 117-118, 135-136, 161-168, 204-213, 244-257; DiffTableView.swift:78-79, 364-372, 937-952

### DV-41 Font, size and line height
- **Trigger:**
  - "Font" picker: "SF Mono (System)", "<family> (not installed)" if the saved family is missing, a divider, then the installed fixed-pitch families, each shown in its own face.
  - "Size" stepper, 9–24 pt in steps of 1.
  - "Line height" slider, 1.2–2.0 in steps of 0.05, with an "N.NN×" label.
- **Preconditions:** JetBrains Mono not installed, for the warning footer.
- **Render:**
  - Footer when the font is missing: "<family> isn't installed, so code uses SF Mono. Install it from its site (JetBrains Mono: jetbrains.com/lp/mono) and reopen this pane."
  - Diff rows re-measure: char width from "M", line height = round(size × factor), small font = size − 1.5.
- **Reaction:** Persisted in `editorFont`, `editorFontSize` and `editorLineHeight`.
- **Edge cases:**
  - Families starting with "." are excluded.
  - Font cache holds 32 sizes.
- **Source:** EditorTheme.swift:119-121, 138-148, 170-184, 214-235

### DV-42 Settings preview diff
- **Trigger:** Settings editor pane, "Preview" section.
- **Preconditions:** None.
- **Render:**
  - A real DiffTableView of a built-in Swift diff of `Sources/Cache.swift`: a hunk header with function context, 1 removed and 2 added lines.
  - Height is 9 × lineHeight + 2, with rounded corners.
  - No hunk buttons, no ruler.
- **Reaction:** Live updates.
- **Edge cases:** ⌘F in the Settings window opens a find bar on the preview (see BROKEN).
- **Source:** EditorTheme.swift:236-238, 259-280

### DV-43 Diff color constants (same in every theme)
- **Trigger:** Display.
- **Preconditions:** Light and dark appearances.
- **Render:**

  | Element | Dark | Light |
  |---|---|---|
  | Added | #2EA043 | #1A7F37 |
  | Removed | #F85149 | #CF222E |
  | addBG | 17% | 12% |
  | delBG | 17% | 11% |
  | addHL | 40% | 30% |
  | delHL | 40% | 28% |

  Also: gutter = foreground at 3%; hunkBG = the function token color at 14% dark / 9% light; meta text = foreground at 60%.
- **Reaction:** None.
- **Edge cases:** None.
- **Source:** EditorTheme.swift:19-21, 71-86

### DV-44 Markdown "Preview" toggle (entry point)
- **Trigger:** The "Preview" checkbox in the diff header, for .md, .markdown and .mdx only. Persisted in the `markdownPreview` default.
- **Preconditions:** A changed Markdown file.
- **Render:** The rendered document (max width 860) replaces the diff. The Before/After picker hides while it is on.
- **Reaction:** Changes reads the file from disk. History uses `git show <sha>:<path>`; PR compact fetches the head, then `git show`. Otherwise it renders the diff's new side.
- **Edge cases:** A deleted Markdown file (no newPath) shows the diff.
- **Source:** ChangesView.swift:556-559, 570, 588-589, 532; Conversation.swift:516-548

### DV-45 Viewport resize and scroll clamp
- **Trigger:** Resize the window or sidebar, the header height changes, or a shorter file loads.
- **Preconditions:** Scrolled to the bottom of a long file, then switch to a short one.
- **Render:** Rows never hang past the end. The column refits to the viewport width.
- **Reaction:** `constrainBoundsRect`.
- **Edge cases:** None.
- **Source:** DiffTableView.swift:71-77, 118, 344-362; vault index.md:237

## LFS

### LF-01 LFS detection
- **Trigger:** Every status refresh.
- **Preconditions:** (a) `.gitattributes` with `*.psd filter=lfs diff=lfs merge=lfs -text`; (b) no attributes but `<common-dir>/lfs/objects` exists; (c) a commented `#… filter=lfs` line, which does not count.
- **Render:** Turns on LF-02, LF-05 and LF-08.
- **Reaction:**
  - Once per model: `git rev-parse --path-format=absolute --git-common-dir`.
  - Reads `.gitattributes` from the worktree root.
- **Edge cases:** Locks load only when the repo uses LFS and git-lfs is on PATH (`Git.searchPath`).
- **Source:** LFS.swift:63-76, 92-114

### LF-02 "git-lfs isn't installed" banner
- **Trigger:** Automatic in Changes.
- **Preconditions:** The repo uses LFS, `git-lfs` is not on PATH, and the banner hasn't been dismissed.
- **Render:**
  - An orange-tinted bar with the externaldrive.badge.exclamationmark icon.
  - Title "This repository uses Git LFS, but git-lfs isn't installed".
  - Caption "Large files stay as pointers until it is. In Terminal: brew install git-lfs && git lfs install" (selectable).
  - Buttons "Copy Command" and "Dismiss".
- **Reaction:**
  - Copy Command puts `brew install git-lfs && git lfs install` on the pasteboard.
  - Dismiss sets `lfsMissingBannerDismissed`, global for every repo and permanent.
- **Edge cases:** Changes only.
- **Source:** LFS.swift:198-225; ChangesView.swift:240

### LF-03 LFS pointer diff becomes the LFS object view
- **Trigger:** Select a file whose diff sides are LFS pointers (Changes compact, Changes Full tab, History, PR compact).
- **Preconditions:**
  - A diff with fewer than 24 lines, not binary.
  - Each side's text (context + removed for old, context + added for new) is either empty or a valid pointer. At least one side is a pointer.
  - A valid pointer starts with `version https://git-lfs.github.com/spec/` (or the hawser spec), has `oid sha256:<64 hex>` and `size <int>`.
  - Fixtures: a modified pointer, an added one (old empty), a deleted one, and an unchanged file in Full mode.
- **Render:**
  - Headline "LFS object" with the externaldrive icon.
  - If old == new: one column "Object".
  - Otherwise "Before" (shown if old exists or new is nil), an arrow when both exist, and "After".
  - Each column shows the size (ByteCountFormatter, file style, title3), the first 12 characters of the oid (monospaced, selectable, tooltip "sha256:<full>"), and "No file" for a missing side.
- **Reaction:** None.
- **Edge cases:** An oid of the wrong length, or extra non-pointer text, falls back to the text diff.
- **Source:** LFS.swift:7-54, 227-276; ChangesView.swift:513, 791; History.swift:576; PullRequestsView.swift:537

### LF-04 LFS image previews
- **Trigger:** Same as LF-03, for image extensions: png, jpg, jpeg, gif, tif, tiff, bmp, heic, webp, psd, ico, icns.
- **Preconditions:** (a) the object is present at `<common>/lfs/objects/ab/cd/<oid>`; (b) not downloaded; (c) an unstaged change whose working-tree file is the real binary (its size equals the pointer size); (d) an object of 64 MiB or more.
- **Render:**
  - A thumbnail of at most 720 px on the long side, decoded at that size, shown fit within 360×360, tooltip "W × H" (the original pixel size).
  - Otherwise "Not downloaded" (tertiary) with the tooltip "Pull, or run git lfs fetch, to get this version". This shows only after the load has finished.
- **Reaction:** `git rev-parse --path-format=absolute --git-common-dir`, then file reads off the main thread. It reloads whenever the dir, path or either oid changes.
- **Edge cases:**
  - The working-tree file is used only in Changes for unstaged changes (`newInWorktree`).
  - Images of 64 MiB or more are never previewed; they show "Not downloaded".
  - A non-image shows no image row.
  - There is no side-by-side, swipe or onion skin; it is just two thumbnails.
- **Source:** LFS.swift:28-31, 232-237, 256, 278-312

### LF-05 Lock list loading
- **Trigger:** Status refresh, at most once a minute per repository (git common dir), shared across its worktrees.
- **Preconditions:** (a) the server lists locks; (b) a remote has `lfs.<url>.locksverify false`.
- **Render:** Turns on lock badges and the Lock/Unlock menu items.
- **Reaction:**
  - `git config --get-regexp ^lfs\..*\.locksverify$`. If any value is "false", locking is unsupported.
  - Otherwise `git lfs locks --json`, decoding `[{id, path, owner{name}}]`.
  - Supported means the JSON parsed.
  - One request at a time.
- **Edge cases:**
  - A missing owner shows as "someone".
  - Duplicate paths: the first one wins.
  - The vault says this used to be per worktree (about 16 requests a minute) and was fixed in 5f97dac.
- **Source:** LFS.swift:56-61, 78-82, 98-145; vault index.md:317

### LF-06 Lock badge
- **Trigger:** Display.
- **Preconditions:** A file locked by someone, in the Changes list or the Full-mode worktree tree.
- **Render:** A lock.fill icon (caption, secondary) before the status icon, tooltip "Locked by <owner> (Git LFS)".
- **Reaction:** None.
- **Edge cases:** Not shown in the History or PR trees.
- **Source:** LFS.swift:190-196; ChangesView.swift:406, 472, 738

### LF-07 Track with LFS (file-list context menu)
- **Trigger:** Right-click a file in Changes, submenu "Git LFS":
  - "Track *.<ext> with LFS" (a single file with an extension).
  - "Track This File with LFS" (a single file).
- **Preconditions:** git-lfs installed. The repo does not need to use LFS. Submodule entries are excluded.
- **Render:** The busy label "Tracking with LFS". `.gitattributes` then appears as a change.
- **Reaction:** `git lfs track -- *.ext`, or `git lfs track --filename -- <path>`. Errors show in the "Git" alert.
- **Edge cases:**
  - Several files selected: no Track items.
  - A file without an extension: only "Track This File".
  - The whole Git LFS submenu is hidden if git-lfs is missing or the selection is only submodules.
- **Source:** LFS.swift:84-88, 149-153, 165-188; ChangesView.swift:430

### LF-08 Lock and unlock (file-list context menu)
- **Trigger:** The "Git LFS" submenu: "Lock" or "Lock N Files" for unlocked paths, "Unlock" or "Unlock N Files" for locked paths. A divider comes first for a single file.
- **Preconditions:** Locking is supported (LF-05). The selection mixes locked and unlocked files.
- **Render:** The busy label "Locking" or "Unlocking", then the badges update.
- **Reaction:** For each path, `git lfs lock -- <p>` or `git lfs unlock -- <p>` with auth env, sequentially. Then the locks reload at once, bypassing the one-minute cap.
- **Edge cases:**
  - The first failure aborts the remaining paths and shows the error alert.
  - Unlocking someone else's lock fails; there is no --force option.
- **Source:** LFS.swift:155-161, 178-184

### LF-09 Hosts that don't recognize LFS pointers
- **Trigger:** A PR Full-mode file, or a File-History picked commit.
- **Preconditions:** An LFS pointer file.
- **Render:** The raw pointer text appears as a normal text diff.
- **Reaction:** None.
- **Edge cases:** This is inconsistent with the other hosts (see BROKEN).
- **Source:** PullRequestsView.swift:565-576; FileInspector.swift:130-135

## Submodules

### SM-01 Submodule change pane
- **Trigger:** Select a submodule entry in the Changes list. The row shows a shippingbox icon instead of a file icon.
- **Preconditions:**
  - (a) the checked-out commit is ahead of the recorded one;
  - (b) it is behind (a rewind);
  - (c) only modified or untracked content inside;
  - (d) a new submodule;
  - (e) a staged pointer change;
  - (f) a fork more than 300 commits off.
- **Render:**
  - DiffHeader with the path, no counts, no Before/After, no blame.
  - While loading: ProgressView.
  - Then a "Submodule" headline (shippingbox), and `<old|none> → <new|none>` in monospace (selectable) plus a note such as "new submodule" or "rewind".
  - Caption: "The recorded commit, before and after this change" (staged) or "The recorded commit, and the one checked out in the submodule" (unstaged).
  - A summary such as "N new commit(s), M commit(s) left behind".
  - Up to 300 commit subjects with plus.circle (green) or arrow.uturn.backward.circle (orange), then "and N more".
  - For content changes: "Contains modified files", "Contains untracked files" or "Contains modified and untracked files", plus "Open the submodule to commit or discard them."
- **Reaction:** `git diff --no-color --no-ext-diff --submodule=log [--cached] -- <path>`. It re-runs when `statusVersion` changes.
- **Edge cases:**
  - An all-zero sha shows as "none".
  - A path containing spaces is handled.
  - Both `..` and `...` ranges are parsed.
  - Dirty-only output has no range row.
- **Source:** Submodules.swift:6-55, 176-253; ChangesView.swift:406, 499

### SM-02 Open Submodule
- **Trigger:** The "Open Submodule" button in the submodule header, or the file-list context menu "Open Submodule" (single selection).
- **Preconditions:** The submodule is checked out (`<sub>/.git` exists). Otherwise the item is disabled.
- **Render:** The submodule appears as its own repository in the sidebar.
- **Reaction:** `app.addRepo(URL)`, the same as ⌘O.
- **Edge cases:** Disabled for an uninitialized submodule.
- **Source:** Submodules.swift:120-127, 141, 186

### SM-03 Update to Recorded Commit
- **Trigger:**
  - The header button (shown only when unstaged and the commit changed; borderedProminent when behind; help "Check the submodule out at the commit this repository records (git submodule update)").
  - Context menu "Update to Recorded Commit" or "Update to Recorded Commits".
- **Preconditions:** (a) a clean submodule; (b) commits made on a detached HEAD that no branch, tag or remote reaches.
- **Render:**
  - With stranded commits, an NSAlert "Leave commits that are on no branch?" that lists "<path>: N commit(s)" followed by "They were made on a detached HEAD. After the update they're only in the submodule's reflog. Open the submodule and create a branch to keep them."
  - Buttons: [Update Anyway (destructive), Cancel].
  - Busy label "Updating submodules".
- **Reaction:**
  - Per submodule: `git rev-list --count HEAD --not --branches --tags --remotes`.
  - Then `git submodule update --init --recursive -- <paths>` with auth env.
- **Edge cases:**
  - Cancel aborts all of them.
  - A submodule that isn't checked out counts 0 stranded commits.
- **Source:** Submodules.swift:63-69, 100-118, 143, 191-195

### SM-04 Stage Pointer and Unstage (submodule)
- **Trigger:**
  - Unstaged: "Stage Pointer". It is prominent when not behind and disabled unless the commit changed. Help text: "Record the submodule's checked-out commit in this repository", or when behind "Record the older commit checked out in the submodule (a rewind)".
  - Staged: "Unstage".
- **Preconditions:** A changed submodule commit.
- **Render:** The entry moves between the sections.
- **Reaction:** Stage: `git add -A -- <path>`. Unstage: `git restore --staged -- <path>`. See CH.
- **Edge cases:** "Behind" means there are commits and none of them was added.
- **Source:** Submodules.swift:187-199; Model.swift:419-432

### SM-05 Submodules in the file list (menu and keys)
- **Trigger:** Right-click a submodule entry; press Delete with it selected.
- **Preconditions:** A selection mixing a submodule and files.
- **Render:**
  - The menu adds a divider, then "Open Submodule" (single only) and "Update to Recorded Commit(s)".
  - "Discard Changes" excludes submodules.
  - Blame and File History are not offered for a submodule.
- **Reaction:** Delete ignores submodules.
- **Edge cases:** None.
- **Source:** Submodules.swift:132-146; ChangesView.swift:369-372, 421-422, 429-431

### SM-06 Uninitialized-submodules banner
- **Trigger:** Automatic in Changes.
- **Preconditions:** A `.gitmodules` listing paths without a `.git` (1, 3 and 5 or more uninitialized).
- **Render:**
  - A blue-tinted bar with the shippingbox icon.
  - Title "A submodule isn't initialized" or "N submodules aren't initialized".
  - Caption with the first 3 paths plus " and N more".
  - Buttons "Not Now" and "Initialize" (borderedProminent, disabled while busy, help "git submodule update --init --recursive").
- **Reaction:**
  - Detection: `git config -f .gitmodules --get-regexp ^submodule\..*\.path$` plus a stat per path.
  - Initialize runs `git submodule update --init --recursive`, labeled "Initializing submodules".
  - Not Now hides the banner for the life of the model, not persisted.
- **Edge cases:** No `.gitmodules` means no git call.
- **Source:** Submodules.swift:57-61, 72-98, 148-174; ChangesView.swift:241

### SM-07 Submodules outside Changes
- **Trigger:** A commit in History or a PR file that changes a submodule pointer.
- **Preconditions:** A commit that changes a gitlink.
- **Render:** A plain text diff of "-Subproject commit <a>" / "+Subproject commit <b>", with no pane.
- **Reaction:** None.
- **Edge cases:** The vault confirms: "Submodules get special handling only in Changes."
- **Source:** History.swift:575-583; vault index.md:288

## Change overview

### CO-01 Entry points
- **Trigger:** The toolbar "View" segmented control with square.grid.2x2 ("Overview: what the change does"), doc.text ("Files: the diff") and, in a PR, bubble ("Conversation").
- **Preconditions:**
  - Changes with at least one changed file.
  - A History commit that is not a stash (stashes have no switch).
  - A PR.
- **Render:** The Overview replaces the pane to the right of the list. In Changes the file list stays.
- **Reaction:**
  - Changes: `@State overview`, not persisted, and reset when a file is picked. It compares HEAD (or empty for an unborn HEAD, i.e. headOID empty or starting with "(") against the worktree.
  - History: persisted in `commitOverview`. It compares the first parent (or empty) against the sha.
  - PR: tab = "overview"; it compares the merge base against the head, with the target branch.
- **Edge cases:**
  - Compact/Full is hidden while the Overview shows.
  - Changes builds ChangedPath values preferring the unstaged entry; deleted means no new side, added or untracked means no old side.
- **Source:** ChangesView.swift:82-95, 228, 253-265, 279-288; History.swift:537, 548-555, 598-601; PullRequestsView.swift:287, 587-595

### CO-02 Loading and error states
- **Trigger:** Open the Overview, or the key changes.
- **Preconditions:** A slow analysis; a PR whose head can't be fetched.
- **Render:**
  - A small ProgressView with "Reading the change…".
  - PR before the base is known: "Fetching the pull request…".
  - PR error: "Can't Read the Pull Request" (exclamationmark.triangle) with the error message.
- **Reaction:**
  - `ChangeLens.analyze(changes, a, b, upstream)`.
  - In a PR, also `BranchLens.status` and `git rev-parse --verify -q refs/remotes/<remote>/<base>^{commit}`, then `git merge-base <base.sha> <head.sha>`.
- **Edge cases:**
  - Recompute keys: Changes "headOID statusVersion"; History "sha fileCount"; PR "base head fileCount targetRef".
  - On a key change the old report is replaced by the spinner.
  - A cancelled task doesn't store its result.
- **Source:** ChangeOverview.swift:17-47; PullRequestsView.swift:602-632

### CO-03 Summary tiles
- **Trigger:** Display, and a click on a tile.
- **Preconditions:** (a) only a files lens; (b) a files lens and one other lens; (c) a files lens and two or more others (for example DB and Deps).
- **Render:**
  - The files tile always shows. Tiles for other lenses show only when there are at least 2 other lenses.
  - Each tile has an icon on a gradient square in the lens tint (files blue, database orange, dependencies brown, configuration gray, follow-ups purple, code indigo), the title, and a chevron that appears on hover (non-files tiles).
  - A 24 pt rounded number with its unit.
  - Then either deltas "+a −r ~c" (zero parts left out), or the summary text when only one delta kind is non-zero (non-files tiles).
  - "N to read" when there are Generated files.
  - The hover background goes from 3.5% to 6%. The tooltip is the summary.
- **Reaction:** Clicking a non-files tile scrolls to its section with a 0.25 s ease-out. The files tile does nothing.
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:52-56, 104-115, 160-171, 398-466

### CO-04 File kinds bar and kind chips
- **Trigger:** In the files tile, click a chip such as "● 2 SQL".
- **Preconditions:** Files of several kinds (Code, Tests, SQL, Dependencies, Config, Build & CI, Docs, Assets, Generated); a kind with more than 16 files.
- **Render:**
  - A 4 px capsule bar split by kind counts, with a 2 px minimum segment.
  - Wrapping chips: a colored dot and "N kind", using the singular forms test/dependency/doc/asset, and "SQL" unchanged. Text turns primary on hover or while open; the cursor is a link pointer.
  - The chips sit beside the number or under it, whichever fits.
  - The popover (400 wide, scrolling at 460 high when over 16 files) has the kind as a headline and rows with file icon, name, dimmed directory and status symbol (added, removed, renamed, modified).
- **Reaction:** Clicking a file row closes the popover and calls `onOpen(path, nil)`.
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:448-456, 504-612

### CO-05 Nothing structural
- **Trigger:** Display.
- **Preconditions:** A change with only files and follow-ups lenses.
- **Render:** "Nothing structural beyond the files: no schema, dependency or configuration changes."
- **Reaction:** None.
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:57-60

### CO-06 Collapse a lens section
- **Trigger:** Click a section header (title3 title, chevron, one-line summary).
- **Preconditions:** Any non-files lens.
- **Render:** The body hides; the chevron rotates −90°.
- **Reaction:** Local state, 0.15 s animation.
- **Edge cases:** Not persisted.
- **Source:** ChangeOverview.swift:470-500

### CO-07 Database: groups and warnings
- **Trigger:** Display; Show/Hide; Open.
- **Preconditions:**
  - SQL migrations under several roots.
  - More than 6 warnings.
  - A warning whose path is `.../<migration>/up.sql`.
  - A warning with `after` SQL.
- **Render:**
  - A folder label per root (head-truncated).
  - Warnings in a grouped box, capped at 6 with "Show N More Warnings" / "Show Fewer".
  - Each warning row: a multicolor triangle, the title, notes, and the file name. up.sql/down.sql show their directory name instead, and the name is omitted if a note already contains it.
  - A small "Show"/"Hide" button when there is SQL, and "Open" when there is a path.
- **Reaction:** Show expands a CodeBlock (14 lines). Open calls `onOpen(path, line)`.
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:379-396, 614-650, 905-934

### CO-08 Database: table cards
- **Trigger:** Display; click the card header.
- **Preconditions:** New, dropped, changed and renamed tables, in a wide and a narrow window.
- **Render:**
  - Masonry columns: as many as fit at 300 minimum, at most 640 wide each.
  - Card header: tablecells icon, the name (middle-truncated), "Renamed from X", and the status symbol with its label ("Dropped" for removed).
- **Reaction:** The header is openable (CO-20).
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:342-377, 628-633, 652-699

### CO-09 Database: columns and folding
- **Trigger:** Click "N unchanged columns" or "Show N More Columns".
- **Preconditions:**
  - A changed table with 3 or more consecutive untouched columns.
  - A new table with more than 12 columns.
  - A changed table where only an index changed.
- **Render:**
  - Column row: an own-status symbol (in changed tables only), a key icon when it is the primary key (tooltip "Primary key"), the name (secondary in dropped or untouched cases), a link icon (tooltip "References X"), and the type or "old → new" (tooltip "NOT NULL" or "Nullable").
  - Notes: for added columns "not null, default X, references Y"; otherwise the notes that don't start with "type ".
  - In a changed table, runs of 3 or more untouched columns fold into "N unchanged columns".
  - Non-changed tables show 12 rows, then "Show N More Columns".
  - When only extras changed, the column list is hidden.
- **Reaction:** Either control expands everything (showAll).
- **Edge cases:** Columns are openable.
- **Source:** ChangeOverview.swift:656-724, 744-778

### CO-10 Database: keys, indexes, triggers and constraints
- **Trigger:** Display; click.
- **Preconditions:** A changed CHECK longer than 90 characters, a multi-line definition, a trigger, a short index change.
- **Render:**
  - Icon per tag: index list.bullet, trigger bolt, key key, other checkmark.shield. Then the title, the tag, and the definition:
    - LineDiff when the text is long or multi-line and has both before and after.
    - A red CodeBlock for removed, a green one for added (14 lines).
    - Otherwise "before → after" in monospace, 2 lines.
  - Triggers show no definition, only notes (3 lines each).
- **Reaction:** Openable.
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:725-731, 780-816

### CO-11 Table "Definition" disclosure
- **Trigger:** "▸ Definition", or "▸ Definition before this change" for a dropped table.
- **Preconditions:** A table with SQL.
- **Render:** The chevron rotates and a CodeBlock (30 lines) appears.
- **Reaction:** 0.15 s animation.
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:193-208, 732-739

### CO-12 Database: functions, views and types
- **Trigger:** Display; the Definition disclosure.
- **Preconditions:** A function of 25 lines or fewer, one longer than 25, a changed view, an enum type.
- **Render:**
  - Row: status symbol, the name, the type, and "Dropped", "Redefined" (a changed function, view or trigger) or the label.
  - Notes.
  - Non-code objects show "before → after" inline.
  - Code objects have a Definition disclosure, open by default when 25 lines or fewer: LineDiff for changed, CodeBlock (12 lines) otherwise.
- **Reaction:** The header row is openable.
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:634-639, 818-862

### CO-13 Database: data statements
- **Trigger:** The "Statement" / "N statements" disclosure.
- **Preconditions:** INSERT, DELETE and UPDATE seed data.
- **Render:**
  - Row: "Insert into <T>", "Delete from <T>" or "Update <T>" (the table name in medium weight), "N statement(s) added/removed/changed", and the file name.
  - Open: CodeBlocks (10 lines). Changed statements show removed and added pairs.
- **Reaction:** The header is openable.
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:640-645, 864-903

### CO-14 Dependencies
- **Trigger:** Display; "Show N More Packages".
- **Preconditions:** More than 10 package changes, including a major bump and a downgrade.
- **Render:** Per section, a subheading and grouped rows: status, the name, "before → after" (or the version) in monospace, and a tag (orange for "major" and "downgrade").
- **Reaction:** Rows are openable. Capped at 10.
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:938-965

### CO-15 Configuration
- **Trigger:** Display; "Show N More Keys".
- **Preconditions:** Nested key changes, including a "breaking" one.
- **Render:** Key path with the prefix dimmed and the last component in full, head-truncated monospace; the tag (orange for "breaking"); "before → after", or the detail when there is no tag and no values.
- **Reaction:** Openable. Capped at 10.
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:967-1008

### CO-16 Code lens
- **Trigger:** Display; "Show N More <section title>".
- **Preconditions:** Added or removed symbols, a warning, an info item (a copy).
- **Render:** A yellow triangle for warnings, doc.on.doc for info, otherwise the status symbol. The title is monospaced except for warnings. Then the detail (2 lines) and the tag.
- **Reaction:** Openable. Capped at 6.
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:1010-1043

### CO-17 Follow-ups
- **Trigger:** Display; "Show N More Lines".
- **Preconditions:** Added lines containing TODO, FIXME or @ts-ignore.
- **Render:** The line in monospace (secondary) with the matched tag in orange, and "file:line" at the right.
- **Reaction:** Openable. Capped at 8.
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:1045-1079

### CO-18 CodeBlock snippets
- **Trigger:** Anywhere a snippet shows; the "Show All N Lines" / "Show Less" link.
- **Preconditions:** A snippet longer than maxLines, indented, with blank edge lines.
- **Render:**
  - Leading and trailing blank lines are trimmed and common indentation removed.
  - Editor font at 11 pt, horizontal scroll, selectable.
  - For added or removed snippets, a + or − mark and a 7% green or red tint.
  - Syntax colors arrive asynchronously.
- **Reaction:** The link toggles between showing everything and the cap.
- **Edge cases:** The highlight cache holds 400 entries, about 4 MB.
- **Source:** ChangeOverview.swift:210-286

### CO-19 LineDiff (short before/after)
- **Trigger:** Display.
- **Preconditions:** A re-indented SQL definition.
- **Render:**
  - An LCS over the trimmed lines, so re-indentation alone doesn't mark lines.
  - Rows marked "+" (green, addBG) or "−" (red, delBG); unchanged rows show the new text.
  - SQL highlighting.
- **Reaction:** None.
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:288-340

### CO-20 Open from the overview (any openable row)
- **Trigger:** Hover (5% background highlight, tooltip "Open <path>[:<line>]"), then click any fact that has a path.
- **Preconditions:** Facts with and without lines, a file outside the change (PR), a file not in the Changes list.
- **Render:** Rows without a path show no hover and don't respond to clicks.
- **Reaction:**
  - **Changes:** overview turns off and the file is selected (unstaged entry preferred). **The line is ignored.**
  - **History:** overview turns off, `selectedPath` is set, and `DiffReveal(line)` is set.
  - **PR:** tab = "files" and `pull.reveal(path, line)`. A file the PR doesn't change switches to Full mode.
- **Edge cases:** In Changes, a path not in the list just closes the Overview.
- **Source:** ChangeOverview.swift:173-191; ChangesView.swift:261-264; History.swift:551-555; PullRequestsView.swift:287; PullModel.swift:421-425

### CO-21 Branch status row (PR only)
- **Trigger:** Display; disclosure rows.
- **Preconditions:**
  - A PR whose base branch moved on.
  - (a) merges cleanly; (b) conflicts.
  - Commits on the target that touch the PR's files.
- **Render:**
  - Branch icon, "N commit(s) behind <target>".
  - "None of them change these files", "All of them change these files" or "N of them change these files".
  - A capsule "Merges cleanly" (green, checkmark) or "N conflicting file(s)" (red, xmark.octagon).
- **Reaction:** None.
- **Edge cases:** The conflicts capsule is hidden when conflicts is nil (unknown).
- **Source:** ChangeOverview.swift:10-11, 37-45, 55, 1081-1115

### CO-22 Conflicting files and touching commits (disclosures)
- **Trigger:** "▸ Conflicting files  N" and "▸ Commits on <target> that change these files  N" (link pointer, count capsule).
- **Preconditions:** More than 8 of each.
- **Render:**
  - Conflict rows: file icon, name, directory, and "changed on <target> by A, B" (authors sorted).
  - Commit rows: sha, subject, "author · date", and the file names.
  - Each list is capped at 8 with "Show N More Files" / "Show N More Commits".
- **Reaction:** Conflict rows are openable. Commit rows are not clickable.
- **Edge cases:** None.
- **Source:** ChangeOverview.swift:1116-1181

---

# 5. Overview lenses (SQL, YAML/config, dependencies, code), code browser, go-to-definition, outline, quick switcher, file tree

Three things testers will expect that the Swift code does not have:
- **No per-file "SQL view" or "YAML view" with a toggle back to the raw diff.** Every lens (Database/SQL, Configuration/YAML, Dependencies, Code, Follow-ups, Files) shows only in the **Overview**. You get to it with the toolbar **View** picker (Overview / Files / Conversation). "Back to raw diff" means picking **Files** in that picker or clicking a fact.
- **QuickSwitcher (⌘K) switches worktrees, not files.** There is no file quick-open.
- **There is no "go to symbol" across the repo.** Code navigation is: ⌘-click or context menu → Go to Definition / Find References, the Outline menu, and Back/Forward.

The vault note is stale in two places, and the code wins:
- It says "a directory has a viewed checkbox". The code has a `done/total` count plus a context-menu item instead.
- It says Changes has "an Overview row". The code uses the toolbar View picker.
- The vault also says the old code-symbol lens was removed on purpose (74440a7). The code agrees.


Three things testers will expect that the Swift code does not have:
- **No per-file "SQL view" or "YAML view" with a toggle back to the raw diff.** Every lens (Database/SQL, Configuration/YAML, Dependencies, Code, Follow-ups, Files) shows only in the **Overview**. You get to it with the toolbar **View** picker (Overview / Files / Conversation). "Back to raw diff" means picking **Files** in that picker or clicking a fact.
- **QuickSwitcher (⌘K) switches worktrees, not files.** There is no file quick-open.
- **There is no "go to symbol" across the repo.** Code navigation is: ⌘-click or context menu → Go to Definition / Find References, the Outline menu, and Back/Forward.

The vault note is stale in two places, and the code wins:
- It says "a directory has a viewed checkbox". The code has a `done/total` count plus a context-menu item instead.
- It says Changes has "an Overview row". The code uses the toolbar View picker.
- The vault also says the old code-symbol lens was removed on purpose (74440a7). The code agrees.

## Overview host (where lenses render)

### LN-01 Open Overview for uncommitted work (Changes)
- **Trigger:** toolbar View picker, "Overview" segment (icon square.grid.2x2, help "Overview: what the change does"). There is no keyboard shortcut.
- **Preconditions:** the worktree has at least 1 changed file. With no changes the picker is not shown, because `DiffControls()` is empty.
- **Render:**
  - While computing: a centred small spinner and "Reading the change…".
  - Then a scroll view padded 24/20: summary tiles, then one section per non-Files lens.
  - When there is no Database, Code, Dependencies or Configuration report, it shows the line "Nothing structural beyond the files: no schema, dependency or configuration changes."
  - The bottom nav-results panel is hidden while the Overview shows.
  - The Compact/Full picker is hidden too (`full: !overview && …`).
- **Reaction:**
  - A = `.rev("HEAD")`. A becomes `.empty` when `headOID` is empty or starts with "(" (unborn branch). B = `.worktree`.
  - Changes come from `model.files` grouped by path: an added or untracked file has old = nil, a deleted file has new = nil, and a rename has old = `origPath`.
  - Lens git calls:
    - `git ls-files -z --cached --others --exclude-standard` (worktree list)
    - `git ls-tree -r -z --name-only HEAD`
    - `git cat-file --batch` (stdin `HEAD:<path>` per file)
    - `git diff -U0 -M -C -C --no-color --no-ext-diff HEAD`
  - Worktree contents are read from disk.
- **Edge cases:**
  - The key is `"<headOID> <statusVersion>"`. `statusVersion` goes up on every status refresh, so every file-system refresh recomputes and shows the spinner again (see BROKEN).
  - The `overview` flag is `@State` and is not persisted.
  - Selecting a file in the list sets `overview = false`.
  - A cancelled task does not publish its result.
- **Source:** ChangesView.swift:228, 254-265, 279-281, 285-288; ChangeOverview.swift:17-47, 57-60.

### LN-02 Open Overview for a commit (History)
- **Trigger:** toolbar View picker "Overview" on a selected commit.
- **Preconditions:** a non-stash commit. Stash entries (`c.selector != nil`) get no View picker and always show the files.
- **Render:** the same as LN-01, under the commit header.
- **Reaction:**
  - A = first parent, or `.empty` for a root commit. B = `.rev(sha)`.
  - Changes come from the commit's files (old/new paths).
  - Key = `"<sha> <fileCount>"`.
- **Edge cases:**
  - The flag is persisted (`@AppStorage("commitOverview")`), so the Overview stays on when you pick another commit.
  - Clicking a fact sets `overview = false`, sets `model.selectedPath = path`, and reveals the line when there is one.
- **Source:** History.swift:537, 548-555, 601.

### LN-03 Open Overview for a pull request
- **Trigger:** toolbar View picker "Overview" (PR modes: Overview / Files / Conversation).
- **Preconditions:** an open PR whose local checkout can fetch it.
- **Render:**
  - "Fetching the pull request…" spinner, then the Overview.
  - On error: "Can't Read the Pull Request" with the error message.
  - The Branch row (LN-10) shows when the base branch is ahead.
- **Reaction:**
  1. `fetchHead`: `git cat-file -e <head>^{commit}` and `<base>^{commit}`. If either is missing: `git fetch --no-tags <remote> +refs/pull/N/head:refs/remotes/<remote>/pr/N +refs/heads/<base>:refs/remotes/<remote>/<base>`.
  2. `git rev-parse --verify -q refs/remotes/<remote>/<base.ref>^{commit}` gives the target tip (falls back to `pr.base.sha`).
  3. `git merge-base <base.sha> <head.sha>` gives A. B = head sha.
  4. Key = `"<base> <head> <fileCount> <targetRef>"`.
- **Edge cases:**
  - Clicking a fact switches to the Files tab and calls `pull.reveal`. A file the PR doesn't change forces Full mode. A fact with a line records Back history; one without a line only selects the file.
- **Source:** PullRequestsView.swift:287, 601-631; PullModel.swift:288-295, 421-424.

### LN-04 Summary tiles strip
- **Trigger:** shown automatically at the top of the Overview.
- **Preconditions:** any reports.
- **Render:**
  - The Files tile is always there. Other lenses get a tile only when there are 2 or more non-Files reports. Alone, a lens has only its section.
  - Each tile has: a coloured icon square (Files blue, Database orange, Dependencies brown, Configuration gray, Follow-ups purple, Code indigo), the title, a big number and its unit (e.g. "3 tables").
  - Under the number it shows Deltas "+a −r ~c" (zeroes left out). When a non-Files tile has at most 1 non-zero delta, it shows the summary text instead.
  - The Files tile adds "N to read" when there are generated files.
  - Hovering a non-Files tile darkens it and shows a trailing chevron. Help text = the summary.
- **Reaction:** clicking a non-Files tile scrolls to its section (anchor top, animated 0.25 s). Clicking the Files tile does nothing.
- **Edge cases:** the Files tile lays the breakdown beside the number when it fits (`ViewThatFits`), otherwise under it.
- **Source:** ChangeOverview.swift:401-466.

### LN-05 Files tile: kind bar, kind chips and file popover
- **Trigger:** click a kind chip ("● 2 SQL", "● 1 test", "● 3 code", …).
- **Preconditions:** changed files in several categories.
- **Render:**
  - A 4 pt capsule bar split by category, in this order and colour: Code blue, Tests teal, SQL orange, Dependencies brown, Config gray, Build & CI indigo, Docs mint, Assets pink, Generated gray 45%.
  - Chips wrap (Flow layout). The text brightens on hover and pointer shows the link cursor.
  - Chip text: SQL is always "SQL". With count 1 the singular is used (test, dependency, doc, asset, otherwise the lowercased title); otherwise the lowercased title.
- **Reaction:**
  - Clicking a chip opens a popover (400 wide) listing the category's files: icon, name, directory (head-truncated), and a status symbol (New / Removed / Renamed / Modified).
  - Over 16 files the list scrolls at 460 high.
  - Clicking a file closes the popover and runs `onOpen(path, nil)`.
- **Edge cases:** category rules (FilesLens.category):
  - Generated: lock files, `.min.`, `.generated.`, pb, `generated/` folders, or a file whose first 5 lines say "@generated / do not edit / code generated / auto-generated".
  - Tests: test dirs, `*_test`, `.test.`, `FooTests`.
  - Docs: md/mdx/rst/txt/adoc or a `docs/` folder.
  - SQL: `.sql`.
  - Dependencies: manifests.
  - Build & CI: `.github/`, `.forgejo/`, `.gitea/`, Dockerfile, `.nix`.
  - Assets: JSON/XML under assets/public/static/res/resources, and media files.
  - Config: json/yaml/toml/ini/…
  - Code: everything else.
- **Source:** ChangeOverview.swift:505-612; ChangeLens.swift:138-206.

### LN-06 Collapse / expand a lens section
- **Trigger:** click the section heading (title, chevron.down, summary).
- **Preconditions:** a non-Files report.
- **Render:** the chevron rotates −90° when collapsed and the body is hidden.
- **Reaction:** local `@State`, animated 0.15 s.
- **Edge cases:** the state is not persisted. It resets when the view is rebuilt.
- **Source:** ChangeOverview.swift:470-500.

### LN-07 Open a fact's file and line
- **Trigger:** click any row that has a path: table header, column, extra, object, data, config, dependency, code or follow-up row. Also a Warning's "Open" button, and conflicting-file rows.
- **Preconditions:** the fact has a path.
- **Render:** hover background `primary.opacity(0.05)`. Help "Open <path>:<line>". A row without a path has no hover and no action.
- **Reaction:** what happens depends on the host:
  - **Changes:** closes the Overview and selects the file in the change list. **The line is ignored.** A path not in the changed files does nothing except close the Overview.
  - **History:** closes the Overview, sets the selected path, and reveals the line.
  - **PR:** switches to the Files tab and calls `pull.reveal` (see LN-03).
- **Source:** ChangeOverview.swift:174-191, 928; ChangesView.swift:261-264; History.swift:551-554; PullRequestsView.swift:287.

### LN-08 Code excerpts: "Show All N Lines" and LineDiff
- **Trigger:** "Show All N Lines" / "Show Less" link under a CodeBlock longer than its `maxLines` (12 by default; 30 for table definitions, 14 for warnings and extras, 10 for data).
- **Render:**
  - Syntax-highlighted monospace text with common indentation removed and blank lines at both ends trimmed. Selectable, and scrolls horizontally.
  - Added lines get a "+" mark and green tint; removed lines get "−" and red.
  - LineDiff matches lines while ignoring indentation (LCS) and shows the new lines with + rows on the add background and − rows on the delete background.
- **Edge cases:** highlighting is async and cached (400 entries / 4 MB). Plain text shows until it is ready.
- **Source:** ChangeOverview.swift:213-340.

### LN-09 CappedRows "Show N More …" / "Show Fewer"
- **Trigger:** a link under a group with more items than its limit.
- **Limits and nouns:** Warnings 6 "Warnings", Packages 10, Keys 10, Code 6 (the noun is the section title), Follow-ups 8 "Lines", conflicts 8 "Files", commits 8 "Commits".
- **Reaction:** toggles showing everything, animated.
- **Source:** ChangeOverview.swift:380-396.

### LN-10 Branch row (PR Overview only)
- **Trigger:** shown automatically when the PR's base branch tip has commits the head lacks.
- **Preconditions:** a base branch that moved on after the PR branched, ideally with one commit touching the same file and one conflicting change.
- **Render:**
  - "N commits behind <base>", then "None of them change these files", "All of them change these files" or "K of them change these files".
  - A pill: "Merges cleanly" (green check) or "N conflicting files" (red octagon). There is no pill when git is older than 2.38 (`merge-tree` prints usage).
  - Disclosure rows "Conflicting files <count>" and "Commits on <base> that change these files <count>", both closed by default.
  - Conflict rows: icon, name, directory, and "changed on <base> by <authors>". Rows open the file.
  - Commit rows: short sha, subject, "author · relative date", file names. These rows are not clickable.
- **Reaction:**
  - `git rev-list --count <head>..<upstream>`. The row is hidden when this is 0.
  - `git --literal-pathspecs log --format=%x1e%h%x1f%an%x1f%ar%x1f%s --name-only <head>..<upstream> -- <paths ≤500>`
  - `git merge-tree --write-tree --name-only --no-messages <upstream> <head>` (failure allowed)
- **Source:** Lenses.swift:713-737; ChangeOverview.swift:38-42, 1085-1181.

## Database (SQL) lens

### LN-11 SQL lens trigger and grouping
- **Trigger:** any changed path ending in `.sql` (case-insensitive).
- **Preconditions (fixtures):**
  - A `migrations/` directory of numbered `.sql` files.
  - A Diesel layout `<root>/<ts>_x/up.sql` plus `down.sql`.
  - A seed directory.
- **Render:** the section is titled "Database". It has one group per root, labelled with a folder icon and the root path ("/" for the repo root).
- **Reaction:**
  - Roots: a changed `up.sql` or `down.sql` makes its grandparent a Diesel root, which replays every `<root>/*/up.sql`. Any other `.sql` makes its parent directory a root, which replays that directory's `.sql` files sorted, leaving out down files.
  - Down files are `down.sql`, `*.down.sql`, `*_down.sql`, or `u<digit>…` (Flyway undo).
  - Root order: Diesel first, then roots whose path contains "migrat", then the rest. Renames found in migrations are carried over to seed directories.
  - Replay at A and at B: listed with `ls-tree` / `ls-files`, read with `cat-file --batch`. The PR upstream is also listed.
  - A group with nothing in it is dropped. The lens is absent when no group has anything.
- **Edge cases:**
  - Summary: "N new tables · N tables dropped · N tables renamed · N tables changed · N <type> added/dropped/changed · N data statements". When it's empty: "SQL changed, schema the same".
  - Metric: with no tables it counts "schema changes" (objects + data); otherwise it counts tables.
  - Binary files and files over 2 MB are not read.
- **Source:** ChangeLens.swift:308-431.

### LN-12 New table
- **Preconditions:** a migration with `CREATE TABLE`.
- **Render:**
  - A card in a masonry grid of 300-640 pt wide columns: tablecells icon, name, green plus "New".
  - The columns, capped at 12, then "Show N More Columns".
  - Each column row: a key icon for primary keys, a link icon (help "References X"), the type in monospace (help "NOT NULL" / "Nullable").
  - A "Definition" disclosure (closed) holds the CREATE TABLE as written, up to 30 lines.
- **Reaction:**
  - Clicking the header opens the CREATE line. Clicking a column opens its line.
  - Column constraints parsed: NOT NULL, PRIMARY (implies NOT NULL), DEFAULT, REFERENCES t(c), CHECK (stored as `<table>_<col>_check`).
  - Table constraints: CONSTRAINT/PRIMARY/UNIQUE/FOREIGN/CHECK/EXCLUDE/LIKE. Unnamed ones get Postgres names (`_pkey`, `_<cols>_key`, `_<cols>_fkey`, `_check`, `_excl`).
- **Edge cases:**
  - `CREATE TABLE IF NOT EXISTS` on a table that already exists is ignored.
  - The `public.` prefix is stripped. Other schema prefixes stay part of the name.
  - TEMP / UNLOGGED / OR REPLACE are skipped.
- **Source:** SQLSchema.swift:337-375, 295-335, 164-185; ChangeLens.swift:477-480; ChangeOverview.swift:653-742.

### LN-13 Dropped table
- **Preconditions:** `DROP TABLE x` on a table that existed at A.
- **Render:** red minus "Dropped"; columns muted with no per-column marks; disclosure "Definition before this change".
- **Reaction:** dropping a table also drops its indexes and triggers. Comma-separated DROP lists are handled.
- **Source:** SQLSchema.swift:461-486; ChangeLens.swift:481-484; ChangeOverview.swift:735.

### LN-14 Renamed table
- **Preconditions:** `ALTER TABLE a RENAME TO b`, or a drop plus a create with identical non-empty columns.
- **Render:** purple arrow "Renamed", with "Renamed from a" under the name. Columns are shown unmarked.
- **Edge cases:**
  - Column equality ignores location.
  - Empty tables are never paired as a rename.
  - Indexes and triggers follow the renamed table.
- **Source:** SQLSchema.swift:546-555; ChangeLens.swift:470-476.

### LN-15 Changed table: columns
- **Preconditions:** migrations that run ALTER TABLE ADD COLUMN / DROP COLUMN / ALTER COLUMN TYPE / SET NOT NULL / DROP NOT NULL / SET DEFAULT / DROP DEFAULT / RENAME COLUMN.
- **Render:**
  - Orange "Modified".
  - Columns with their own status symbol (added, removed, changed).
  - A type change reads "old → new".
  - Notes: "now NOT NULL", "now nullable", "default x → y", "reference a → b". Added columns note "not null, default …, references …".
  - Runs of 3 or more unchanged columns fold into a "N unchanged columns" row. Clicking it shows all columns.
  - Dropped columns go back to their old position.
  - When only keys, indexes or triggers changed, the column block is hidden entirely.
- **Edge cases:**
  - `ADD COLUMN IF NOT EXISTS` on an existing column is ignored. Without IF NOT EXISTS it replaces the column.
  - `SET DATA TYPE` and `TYPE … USING` are both read.
  - Several comma-separated actions in one ALTER are all applied.
- **Source:** SQLSchema.swift:496-569; ChangeLens.swift:486-529; ChangeOverview.swift:665-778.

### LN-16 Changed table: keys, constraints, indexes, triggers
- **Render:** extra rows under the columns, each with an icon: index = list.bullet, trigger = bolt, key = key, constraint = checkmark.shield. Each row has a status, a title and a tag.
  - A definition over 90 characters, or one with several lines, shows as a LineDiff, or as a CodeBlock (red when removed, green when added).
  - Short definitions show on one line as "before → after".
  - Trigger rows show notes instead of SQL: timing such as "BEFORE UPDATE OF col · each row → fn()", "was …", and "Raises “msg” · ERRCODE".
- **Reaction:**
  - The primary-key change fact shows "(a) → (a, b)".
  - Constraints are compared by name and definition. DROP + ADD under the same name with a new definition counts as "constraint changed", shown as it was written (the ALTER TABLE … ADD CONSTRAINT prefix is stripped).
  - Index definitions are compared, including UNIQUE.
  - Unnamed indexes are named `<table>_<letters of def, first 30>_idx`.
  - `ALTER INDEX … RENAME TO` is followed.
- **Edge cases:** an index or trigger on a table this group's schema doesn't define becomes an object row "… · ON <table>" (LN-17).
- **Source:** ChangeLens.swift:509-563; SQLSchema.swift:376-390, 411-422, 455-460, 574-579; ChangeOverview.swift:781-816.

### LN-17 Functions, views, types (objects)
- **Preconditions:**
  - CREATE TYPE … AS ENUM, ALTER TYPE ADD VALUE, RENAME VALUE, RENAME TO.
  - CREATE [OR REPLACE] FUNCTION/PROCEDURE with a `$$` body containing RAISE EXCEPTION '…' USING ERRCODE='…'.
  - CREATE [MATERIALIZED] VIEW.
- **Render:** under the subheading "Functions, Views and Types". Each row has a status symbol, a name, a type tag, and a state on the right: "New", "Dropped", "Redefined" (code objects) or "Modified".
  - Enums show "before → after" on one line, with detail "enum values + a, b  − c".
  - Functions, views and triggers have a "Definition" disclosure. It is open by default when the definition is 25 lines or fewer. A changed object shows a LineDiff.
  - Notes "Raises “…” · code".
- **Edge cases:**
  - A non-enum CREATE TYPE counts as "type added".
  - A function redefinition is found by its whitespace-normalised text changing.
- **Source:** SQLSchema.swift:391-410, 436-454; ChangeLens.swift:565-598; ChangeOverview.swift:818-862.

### LN-18 Data statements (seed files)
- **Preconditions:** a changed `.sql` file whose INSERT, UPDATE or DELETE statements were added, removed or edited.
- **Render:**
  - Subheading "Data". Each row reads "Insert into <table>" / "Update <table>" / "Delete from <table>", then "N statements added/removed/changed" and the file name.
  - A "Statement" / "N statements" disclosure (closed) holds them. A changed statement shows as a red/green pair.
- **Reaction:**
  - Statements are compared by normalised text and paired per verb and table.
  - INSERT on an old table name is paired across a rename found in migrations.
  - `public.t` is shortened to `t`.
- **Source:** ChangeLens.swift:605-648; ChangeOverview.swift:864-903.

### LN-19 Warnings: risky statements
- **Preconditions:** statements new in a changed migration. A statement whose text already existed at A is not reported.
- **Render:**
  - A grouped list at the top of the root (capped at 6).
  - Each warning has a triangle icon, a title, a note, and a file label. For up.sql/down.sql the label is the migration directory, and it is left out when the note already names it.
  - Buttons "Show"/"Hide" (the statement source, up to 14 lines) and "Open".
- **The rules:**
  - "Drops table X": X existed at A.
  - "Drops column X.c".
  - "NOT NULL column without a default: X.c": an ADD with NOT NULL and no DEFAULT or GENERATED.
  - "Changes the type of X.c".
  - "Makes X.c NOT NULL".
  - "Index on X built without CONCURRENTLY": X existed at A.
  - "Updates rows in X" / "Deletes rows from X": only in a Diesel root or one whose path contains "migrat". The note reads "Every row: there's no WHERE clause." or "Runs with the migration, on every database."
- **Edge cases:** tables created in the same change don't trigger the existing-table rules.
- **Source:** SQLSchema.swift:584-644; ChangeLens.swift:382-394; ChangeOverview.swift:905-934.

### LN-20 Warnings: migration ordering
- **Preconditions:** a new migration file whose leading version:
  - equals an existing one's: "Migration version already used", or
  - sorts before existing ones: "Migration older than N existing ones", with the note "… sorts before <last> and N-1 more".
- **Reaction:**
  - Version parse: an optional leading V/U/R, then the leading run of digits and "-_." separators split into numbers. "2026-11-03-100000_x" → [2026, 11, 3, 100000]. "V2_1__x" → [2, 1].
  - Comparison is against A's migrations plus the PR upstream's, not counting the new file itself.
  - For Diesel the id is the directory name.
- **Source:** ChangeLens.swift:314-321, 364-381.

### LN-21 Warnings: statements not replayed
- **Preconditions:** a changed file with a DO block, CREATE POLICY, CREATE RULE, or any unknown top-level verb (e.g. VACUUM).
- **Render:**
  - Title "<KIND> not replayed", e.g. "DO block not replayed" or "CREATE POLICY not replayed".
  - Detail "N statements not replayed".
  - Note "Loupe doesn't run procedural SQL, so the schema shown doesn't include its/their effect."
  - "Show" reveals the first such statement.
- **Edge cases:**
  - CREATE EXTENSION, SCHEMA and SEQUENCE are dropped silently.
  - ALTER of anything other than TABLE, TYPE or INDEX is dropped silently.
  - INSERT, UPDATE, DELETE, SELECT, COMMENT, GRANT, REVOKE, SET, BEGIN, COMMIT, WITH, TRUNCATE and COPY are never reported here.
- **Source:** SQLSchema.swift:423-425, 487-493; ChangeLens.swift:395-402.

### LN-22 SQL tokenizer fidelity (edge cases)
- **Preconditions:** SQL containing:
  - `--` and nested `/* */` comments
  - `'str''ing'` and `E'\''`
  - `"Quoted ""Ident"""`
  - `$tag$ … ; … $tag$` bodies
  - `::casts`
  - `<>`, `<=`, `!=`
- **Reaction:**
  - Statements split only on top-level `;`.
  - Words are lowercased; quoted identifiers are kept as written.
  - A statement's source keeps its original lines and comments.
- **Source:** SQLSchema.swift:37-128, 276-289.

## Dependencies lens

### LN-23 Dependency changes
- **Trigger:** a changed file named Cargo.lock, Cargo.toml, pyproject.toml, package.json, package-lock.json, go.mod, Package.resolved, flake.lock, or `requirements*.txt`.
- **Render:**
  - Section "Dependencies". Subheading = the file path.
  - Rows (10, then "Show N More Packages"): status symbol, package, value ("a → b" when changed), and a tag: major, minor, patch or downgrade. major and downgrade are orange.
  - Summary: "N added · N removed · N updated (N major)". Metric "N packages".
- **Reaction:**
  - Files are read with `cat-file --batch` / from disk, maxSize 20 MB.
  - Labels: package.json sections get " (dev)", " (peer)", " (optional)". Cargo target-specific tables get " (<section>)". Cargo `workspace = true` reads "workspace". Path dependencies read "path X".
  - package-lock nesting shows as "a › b". A Cargo.lock duplicate reads "name (version)".
  - flake.lock reads "rev8 (date)". Package.resolved reads the version or the first 8 characters of the revision.
  - Bump classification: numbers padded to 3 parts. Lexicographically lower = downgrade.
- **Edge cases:**
  - Invalid JSON produces no facts.
  - Versions that aren't numeric get no tag.
  - The lens is absent when nothing changed.
- **Source:** Lenses.swift:5-165; ChangeOverview.swift:938-965.

## Configuration lens (JSON / TOML / YAML / .env / .properties)

### LN-24 Config key changes
- **Trigger:** a changed file with extension json, toml, yaml, yml or properties, or named `.env*` / `*.env`.
- **Excluded:**
  - Files handled elsewhere: package.json, package-lock.json, Cargo.toml/lock, pyproject.toml, flake.lock, Package.resolved, composer.lock, yarn.lock, pnpm-lock.yaml, tsconfig.json.
  - Files in the Assets, Tests or Generated categories.
- **Render:**
  - Section "Configuration". Summary "N changes in M files". Subheading per file.
  - Rows (10, then "Show N More Keys"): status symbol and the key path in monospace, parent parts muted and head-truncated.
  - Tags: "N keys", "new file", "deleted", "breaking" (orange), "columns".
  - Value line "before → after", or the detail when there is no tag.
- **Reaction:**
  - A whole new file is one fact "new file · N keys". A deleted file is one fact "deleted · had N keys".
  - Keys under a parent that exists on one side only collapse into the parent. A group of 4 or fewer leaf keys is listed inline as "k: v · k: v".
  - List values are diffed as "+x  −y" ("reordered" when nothing joined or left).
  - Multi-line values read "changed (N lines)".
  - A key whose last part contains secret, token, password, passwd, apikey, api_key, private, credential or auth shows "(value hidden)".
- **Edge cases:**
  - Files over 1 MB are not read.
  - TOML is flattened per `[section]`. `.env` handles `export ` and `#`/`!` comments.
  - JSON fragments are allowed.
  - Values are cut to 80 characters.
- **Source:** Lenses.swift:169-347, 422-455; ChangeOverview.swift:967-1008.

### LN-25 YAML parsing specifics
- **Preconditions:**
  - A multi-document k8s file (`---`).
  - Lists of mappings with name, id, key or type.
  - RBAC rules.
  - Expo plugins as `[name, {opts}]`.
  - Helm `{{ }}` lines.
  - `|` and `>` block scalars, flow `[a, b]` and `{k: v}`, quoted keys, inline `# comments`.
- **Reaction:**
  - Documents split on `---`, `--- …` and `...`. Each document is prefixed "[Kind name]", or "[i]" when unnamed, but only when the file has more than one document.
  - List items are keyed by the first of name, id, key, type, or "apiGroups: resources" that is unique and non-empty for every item. Otherwise by index.
  - Lists of plain values stay one value "[a, b]".
  - Expo-style pairs are keyed by name; a bare name is "on".
  - Helm and comment lines are skipped.
- **Edge cases:** anchors, aliases, tags and multi-line flow collections are not supported (ponytail note).
- **Source:** YAML.swift:6-146; Lenses.swift:225-278.

### LN-26 CustomResourceDefinition lens
- **Preconditions:** a single-document YAML with `kind: CustomResourceDefinition` on both sides, e.g. adding an enum field to `status.phase`, making a field required, or adding a printer column.
- **Render:**
  - The section is titled "<Kind> · <file>", before the other config sections.
  - Field facts show the type, e.g. "Pending | Bound · required", "list of string", "int or string", "string (date-time)".
  - Removed fields and type changes are tagged "breaking", as is a field becoming required. A field becoming optional is not.
  - "kubectl get" fact: "NAME · AGE → NAME · …" (tag "columns").
  - With several CRDs changed, facts shared by all of them go into a first section "A, B · the same in each".
- **Edge cases:**
  - A multi-version CRD prefixes fields with "<version> ".
  - Schema and printer-column keys are left out of the key facts.
- **Source:** Lenses.swift:353-420, 437-471.

## Code and Follow-ups lenses

### LN-27 Code lens
- **Trigger:** any change containing one of:
  - copied files (the diff with `-C -C` reports a copy)
  - an import swapped or added across modified files
  - an Expo native-code change
- **Render:** section "Code", with subsections "App build", "Copied", "Imports across files" (6 rows each, then "Show N More <title>").
  - Copied: "src/ → src/classic/" with "26 files · 90–100% the same" (doc icon).
  - Imports: "Button → PillButton" with "N files · module", or "X · now imported in N files" (tag "new"), or "no longer imported in N files".
  - Native warning: "Native code changed; runtimeVersion is still X".
- **Reaction:**
  - Imports are read from added and removed lines (`git diff -U0 -M -C -C --no-color --no-ext-diff A B`) in JS/TS/Vue/Svelte, Python and Rust.
  - A swap needs 2 or more shared files. A new module needs 3 importers, or 2 when the module is a file this change adds.
  - New files don't count.
  - The Expo warning needs:
    - a native file: swift/m/mm/h/kt/java/cpp/cc/c or Podfile.lock
    - B's package.json containing "expo-updates"
    - the same `expo.runtimeVersion` string in app.json at A and at B
- **Edge cases:**
  - Generated files are excluded from the line facts.
  - When A is empty, or for untracked files, the whole file counts as added (300 files at most).
  - Only the root app.json is read.
- **Source:** Lenses.swift:476-512, 567-709; ChangeOverview.swift:1012-1043.

### LN-28 Follow-ups lens
- **Trigger:** added lines matching one of:
  - To Do: `TODO|FIXME|XXX|HACK`
  - Skipped and Focused Tests: `it.skip`, `.only`, `xit`, `#[ignore]`, `t.Skip`, `XCTSkip`, `@Disabled`
  - Silenced Checks: `@ts-ignore`, `eslint-disable`, `#[allow(`, `noqa`, `nolint`, `swiftlint:disable`, …
- **Render:**
  - Subsections with up to 8 rows, then "Show N More Lines".
  - Each row: the line in monospace with the match in orange, and "file:line" on the right.
  - Long lines start "…" 20 characters before the match.
  - Summary e.g. "2 to-dos · 1 skipped test".
- **Edge cases:**
  - A match inside a string literal is ignored. An apostrophe between letters doesn't open a string.
  - Generated and Docs files are skipped.
  - At most 200 facts per kind.
  - The Overview's "nothing structural" line still shows when only Follow-ups are present.
- **Source:** Lenses.swift:516-560; ChangeLens.swift:131; ChangeOverview.swift:1047-1079.

### LN-29 Dev command line `--lens`
- **Trigger:** `Loupe --lens <repo> <revA> <revB|worktree> [<upstream>]`
- **Reaction:**
  - Runs `git diff --name-status -z -M revA [revB]`.
  - Prints the Branch status and each report: "■ Title: summary", then sections, then up to 40 facts each with + − ~ → · ! markers, "@path:line", and notes.
  - Prints the time taken in ms and exits.
- **Source:** App.swift:12-50.

## Code browser (Full mode, tabs, navigation, outline)

### CB-01 Enter or leave Full mode
- **Trigger:**
  - Toolbar Mode picker. Icons: rectangle.compress.vertical "Compact: …", rectangle.expand.vertical "Full: whole files with the changes inline (⌥⌘F)".
  - Review › Compact / Full Mode (⌥⌘F, can be changed in Settings › Shortcuts › Review).
- **Preconditions:** a diff is showing: Changes with a selected file or already in Full mode, or a PR on its Files tab.
- **Reaction:**
  - Toggles the global `prFullMode` default, shared by Changes, History and PRs.
  - Changes entering Full: the selected file becomes the open tab. Changes leaving Full: the change list selects the browser's file if it is a changed one.
  - PR entering Full: the selected path is appended as a tab. PR leaving Full on an unchanged file selects the first changed file and clears nav results.
- **Source:** ChangesView.swift:73-81, 289-294; App.swift:98; PullModel.swift:299-307; PullRequestsView.swift:455-458.

### CB-02 Worktree file tree (Changes, Full)
- **Trigger:** Full mode in Changes. The left pane replaces the change list.
- **Render:**
  - A spinner while the tree is empty.
  - FileTreeList rows: icon, name (secondary when unchanged, primary when changed), LFS lock badge, StatusIcon. Help = path.
- **Reaction:**
  - `git ls-files -co --exclude-standard -z`, plus the changed paths so deleted files stay reachable.
  - Sorted with `treeOrder`. Reloads when `"<statusVersion> <changeCount>"` changes.
  - Default fold: every directory collapsed except ancestors of changed files. This is applied once, after changes are known.
- **Edge cases:** see BROKEN for the clean-worktree re-collapse and the empty-repo spinner.
- **Source:** ChangesView.swift:723-748; CodeBrowser.swift:121-141.

### CB-03 Open a file from the tree (Changes, Full)
- **Trigger:** click a file row.
- **Render:**
  - A tab is inserted after the current tab (or at the end).
  - Header: path, "+/−" counts only when the file is changed, the nav controls (CB-12, CB-13), and Stage File / Discard… or Unstage File.
  - Body: a spinner while loading, then "Binary File", "Empty File", an LFS object view, or the DiffTableView with a change ruler.
  - No tab open: "No File Open" with "Open one from the tree, or ⌘-click a name to go to its definition."
- **Reaction:**
  - An untracked, added or deleted file uses the normal file diff.
  - A modified file: `git diff --no-color --no-ext-diff -M -U1000000 HEAD -- <path>`, with the hunk header removed.
  - Any other file is read from disk as plain context lines. A NUL byte in the first 8000 bytes means binary.
- **Edge cases:**
  - Open files are cleared when `statusVersion` changes, so each refresh reloads the file and briefly shows the spinner.
  - Entering Full with no tabs opens the list's selected file.
- **Source:** ChangesView.swift:751-804; CodeBrowser.swift:77-84, 144-160; PullModel.swift:381-402.

### CB-04 PR Full-mode tree and files
- **Trigger:** Full mode on the PR Files tab.
- **Render:**
  - "Fetching the pull request…" while the tree is empty, or "Can't Load the Repository" with the error.
  - Tree: changed files as TreeFileRow (dimmed when viewed, comment count bubble). Unchanged files show icon and name in secondary.
  - Header of an unchanged file: "Unchanged". Header of a changed file: a "Viewed" checkbox.
  - Body: "Binary File", "Empty File", or the table. Comments can be added only on lines inside the diff hunks.
- **Reaction:**
  - `fetchHead`, then `git ls-tree -r -z --name-only <head>`.
  - The default fold (directories the PR doesn't touch) is applied once both the tree and the files are known.
  - A changed file: `git diff --no-color --no-ext-diff -M -U1000000 <base>...<head> -- <old> <new>`. Binary and deleted files use the PR diff as it is.
  - An unchanged file: `git show <head>:<path>`.
  - Reloads when the head sha changes.
- **Edge cases:** the "Filter files" field filters tree paths case-insensitively; while it is non-empty every folder is expanded (FT-07).
- **Source:** PullRequestsView.swift:429-501, 553-587; PullModel.swift:308-378.

### CB-05 File tabs
- **Trigger:** the tab bar (30 pt) above the file in Full mode. It shows only when tabs exist.
- **Render:**
  - Each tab: icon and name. When two open tabs share a name (mod.rs, index.ts), the parent directory is added in caption.
  - A dot in the change colour (help "Changed").
  - An xmark (help "Close Tab"), visible only on the active or hovered tab.
  - The active tab has the text background and a 2 pt accent line on top. Help = path.
- **Reaction:**
  - Clicking a tab selects it. The bar scrolls to the selected tab (animated).
  - Clicking xmark: `closeTab`. When the closed tab was the selected one, the tab now at that index (or the last tab) becomes selected; when none are left, nothing is selected.
- **Source:** CodeBrowser.swift:49-55, 178-221.

### CB-06 Tab context menu
- **Trigger:** right-click a tab.
- **Items:**
  - "Close Tab"
  - "Close Other Tabs" (disabled when there are fewer than 2 tabs; it also selects that tab)
  - "Close All Tabs" (no selection afterwards)
  - divider
  - "Copy Path" (puts the repo-relative path on the general pasteboard)
- **Source:** CodeBrowser.swift:222-228.

### CB-07 ⌘W closes the tab
- **Trigger:** ⌘W (a local key monitor, only for this window).
- **Preconditions:** Changes in Full mode, or the PR Files tab in Full mode.
- **Reaction:** closes the selected tab. With no tab selected it does nothing, and never closes the window. Anywhere else ⌘W passes through.
- **Source:** ChangesView.swift:766; PullRequestsView.swift:597, 684-710.

### CB-08 ⌘-hover link
- **Trigger:** hold ⌘ and move the mouse over an identifier in a navigable DiffTableView. Pressing or releasing ⌘ updates it at once (`flagsChanged`).
- **Render:** a 1.2 pt underline in the link colour under the word, and the pointing-hand cursor. Without ⌘ the cursor is the I-beam.
- **Edge cases:**
  - Word characters are [0-9A-Za-z_] and anything above 127.
  - A word starting with a digit gets no link.
  - Over the line numbers there is no word.
  - Shown in **any** file type when navigation is on, including YAML, SQL and Markdown.
  - Leaving the view clears it.
- **Source:** DiffTableView.swift:220-230, 592-612, 898-920, 983-987.

### CB-09 Go to Definition: ⌘-click
- **Trigger:** ⌘-click (single click) on an identifier.
- **Preconditions:** navigation is available in the Changes diff (Compact and Full) and PR Full mode. It is **not** available in PR Compact, History, or the Overview.
- **Render:** the word gets selected. The header shows a mini spinner and the word while looking it up (CB-14).
- **Reaction:**
  - `git grep -l -I -w --fixed-strings -e <word>` with scope `--untracked --` for the worktree, or `<head> --` for a PR.
  - Files with a grammar are ranked: the current file, then the same directory, then the same extension, then the rest. The first 80 are parsed in parallel with tree-sitter tags, keeping definitions whose name matches exactly.
  - The worktree is read from disk; a PR uses `git show <head>:<f>`.
  - A PR first runs `fetchHead`. If that fails, the PR error is set and the lookup stops.
  - **Exactly 1 result:** results are cleared and `open(path, line)` runs, recording Back history. The file opens as a tab, scrolls the line a third of the way down, and selects it.
  - **2 or more:** CB-10. **None:** CB-11.
- **Edge cases:**
  - Results are sorted by (rank, path, line).
  - Too-large files are skipped (CB-17).
  - A word with no hits: grep fails and the result is empty.
- **Source:** DiffTableView.swift:616-622, 887-890; CodeBrowser.swift:163-172; PullModel.swift:427-437; CodeIndex.swift:204-255.

### CB-10 Several definitions panel
- **Render:** a bottom panel 220 high. Header "N definitions of <word>" (word in monospace) and an xmark. Rows: icon, "path:line" (260 wide, head-truncated), "<kind> <name>".
- **Reaction:**
  - Clicking a row opens it and records Back. The panel stays open.
  - xmark or **Esc** (`.cancelAction`) closes the panel.
- **Edge cases:**
  - In Changes the panel is hidden while the Overview shows.
  - In a PR the panel is cleared when leaving Full mode.
  - `release()` clears it when you leave the worktree.
- **Source:** CodeBrowser.swift:275-323; ChangesView.swift:273-277; PullRequestsView.swift:521-524.

### CB-11 No definition: references fallback
- **Trigger:** ⌘-click or Go to Definition when no definition is found.
- **Render:** header "No definition found in this repository · N references to <word>". Rows: "path:line" and the trimmed line text.
- **Reaction:** `git grep -n -I -w --fixed-strings -e <word> <scope>`, capped at 2000 matches.
- **Edge cases:**
  - Count text: "1 reference", "N references", or "2000+ references" when the cap is reached.
  - Zero results gives an empty list.
- **Source:** CodeBrowser.swift:171, 284-288; CodeIndex.swift:223-237.

### CB-12 Context menu: Go to Definition / Find References
- **Trigger:** right-click on an identifier in a navigable diff.
- **Items:**
  - "Go to Definition of “<w>”"
  - "Find References to “<w>”"
  - separator, then the usual diff items (Add Comment…, Stage/Unstage/Discard Line(s)…)
- **Reaction:**
  - Definition: as CB-09.
  - References: skips the definition search. The header has no "No definition…" prefix.
- **Edge cases:** the word is taken at the mouse x on the clicked row's middle. With no word under the mouse there are no nav items.
- **Source:** DiffTableView.swift:688-696.

### CB-13 Back / Forward
- **Trigger:**
  - chevron.left button (help "Back", ⌃-)
  - chevron.right button (help "Forward", ⌃⇧-)
  - both can be changed in Settings › Shortcuts › Review ("Navigate Back" / "Navigate Forward")
- **Preconditions:** Full-mode file header only, in Changes or PR Full mode. The buttons are disabled when their stack is empty.
- **Reaction:**
  - Every `open()` to a different path or line pushes the current (path, last revealed line or 1) onto Back and clears Forward. Opens from a result, the outline, ⌘-click or the Overview all count.
  - Back pops and pushes the current location onto Forward; Forward does the opposite. Both reveal the line.
- **Edge cases:**
  - Opening from the tree or a tab sets the path without recording history (see BROKEN).
  - In Changes Compact the buttons and shortcuts don't exist.
- **Source:** CodeBrowser.swift:29-47, 245-248; Shortcuts.swift:53, 72-73, 86, 105-106.

### CB-14 Lookup busy indicator
- **Render:** a mini ProgressView and the word in caption monospace, left of Back/Forward, while a lookup runs.
- **Edge cases:** cleared on every exit path (`defer`).
- **Source:** CodeBrowser.swift:241-244; PullModel.swift:428.

### CB-15 A jump while in Compact (Changes)
- **Trigger:** ⌘-click, a result row, or the context menu, with Changes in Compact mode.
- **Reaction:**
  - If the target is a changed file, the change list selects it and the diff reveals the line (`reveal` is passed only when the browser's path equals the shown file).
  - Otherwise it **switches to Full mode automatically** (the global `prFullMode`).
  - In Compact, the browser follows the list's selection without opening tabs.
- **Source:** ChangesView.swift:285-299, 528-530; CodeBrowser.swift:108-111.

### CB-16 Outline menu
- **Trigger:** the list.bullet.indent menu button in the Full-mode file header (help "Outline: jump to a definition in this file").
- **Preconditions:** a loaded, non-binary file in a supported language that has definitions.
- **Render:**
  - One item per definition line, sorted by line: "<name>  <kind> · <line>".
  - Kinds: function, method, class, interface, module, type, constant, macro, field, other.
  - The button is hidden when there are no symbols, the language is unsupported, or the file is binary or not loaded.
- **Reaction:**
  - Picking an item calls `open(path, line)`, which records Back and reveals the line.
  - Recomputed when the path or loaded state changes, off the main thread.
- **Source:** CodeBrowser.swift:233-271; CodeIndex.swift:119-132.

### CB-17 Outline too large, caching, cancellation
- **Preconditions:** a file over 2 MB; or over 64 KB with more than 50,000 lines; or over 64 KB with an average line over 1000 bytes (minified).
- **Render:** a dimmed (tertiary) outline icon with no menu, help "No outline: this file is too large or minified to parse".
- **Reaction:**
  - These files also give no definitions. The parse has a 5 s timeout.
  - Outlines are cached by path, length and hash (64 entries). One parse runs per content, shared by every view that asks. The parse is cancelled when every caller leaves (the file changed or the view closed).
  - A cancelled result is not cached.
- **Edge cases:** names of 200 characters or more are dropped. One symbol per line.
- **Source:** CodeIndex.swift:76-91, 95-117, 150-191; vault (the 2026-11-11 memory pass: a 30 MB minified JS used to leak about 1.25 GB per visit).

### CB-18 Languages and what counts as a definition
- **Supported extensions:**

  | Extensions | Grammar |
  |---|---|
  | rs | Rust |
  | swift | Swift |
  | go | Go |
  | ts, mts, cts | TypeScript |
  | tsx | TSX (uses the TypeScript tags) |
  | js, jsx, mjs, cjs | JavaScript |
  | py, pyi | Python |
  | c, h | C |
  | cc, cpp, cxx, hpp, hh, hxx, mm | C++ |
  | java | Java |

  Every other extension gets no outline and no definitions, but references still work.
- **Definitions per language:**
  - Rust: struct, enum, union and type as class; fn as function, or method inside an impl/trait body; trait as interface; mod as module; macro_rules as macro.
  - Swift: class, struct, enum and extension declarations as class; protocol as interface; func; init/deinit and subscript as method; properties as field.
  - Go: func, method, type spec.
  - JS/TS: class; function, generator and arrow or function-expression assignments and pairs as function; methods except `constructor`; exported constant assignments as constant. TS adds function/method signatures, abstract class, module, interface.
  - Python: class, def, module-level assignment as constant.
  - C/C++: struct, union, function declarators, typedef, enum, class; C++ adds `ns::f` as method.
  - Java: class, method, interface.
- **Edge cases:** Go's package, import, var and const captures have no definition tag, so they are ignored.
- **Source:** CodeIndex.swift:33-74, 134-147; TagQueries.swift:3-466.

## Quick Switcher (worktrees)

### QS-01 Open the switcher
- **Trigger:** ⌘K, or View menu › "Go to Worktree…" (in the sidebar command group; can be changed under Settings › Shortcuts › General "Go to Worktree").
- **Render:** a 600×380 sheet. A plain title3 text field with placeholder "Go to worktree…", a divider, then a plain list.
- **Edge cases:** the field gets no explicit focus; it relies on the sheet's default first responder.
- **Source:** App.swift:92, 175; Model.swift:44; Shortcuts.swift:93; QuickSwitcher.swift:29-56.

### QS-02 Rows
- **Render:**
  - The repo folder name (secondary, 130 wide), then the sidebar WorktreeRow.
  - WorktreeRow: icon folder (main), arrow.triangle.branch, or purple arrow.triangle.merge (merged). Title: "Root" for the main worktree, otherwise the folder name minus the "<repo>-/_/." prefix. Second line: the branch or "detached @ <7 chars>". A "Merged" badge.
  - The highlighted row has an accent 25% background.
- **Edge cases:**
  - Worktrees of collapsed repos are listed too.
  - Every row creates or looks up a WorktreeModel (`app.model(for:)`).
- **Source:** QuickSwitcher.swift:38-53; App.swift:569-600.

### QS-03 Matching
- **Trigger:** type in the field.
- **Rules:**
  - The haystack is `"<repo folder> <branch> <worktree folder name>"`, lowercased.
  - The query is lowercased and split on spaces. **Every** token must be a plain substring. This is not fuzzy and not ordered.
  - An empty query lists everything.
- **Reaction:** any query change resets the highlight to 0.
- **Edge cases:** no match gives an empty list with no "no results" text.
- **Source:** QuickSwitcher.swift:5-16, 59.

### QS-04 Ranking and preselection
- **Rules:**
  - Ordered by position in `AppModel.recent` (most recent first, 30 kept, persisted in UserDefaults "recent"). Ties go to sidebar order (repos in order, main worktree first).
  - On open: when there are 2 or more items and the first is the current worktree, row 1 is highlighted, so ⌘K ↩ toggles back to the previous worktree. Otherwise row 0.
- **Edge cases:** the list covers **only the current space's repos** (`app.repoPaths`). See BROKEN.
- **Source:** QuickSwitcher.swift:7, 24-27, 57-58; Model.swift:20-23, 25-40; vault ("⌘K ↩ toggles back").

### QS-05 Keyboard navigation
- **Trigger:** ↓ / ↑ while the text field has focus.
- **Reaction:** the highlight moves by 1, clamped to [0, count−1], and the list scrolls to the row. The key press is handled, so the caret doesn't move.
- **Edge cases:** with no items the index stays 0.
- **Source:** QuickSwitcher.swift:34-35, 52, 62-65.

### QS-06 Open the worktree
- **Trigger:** Return (`onSubmit`) on the highlighted row, or a click on any row.
- **Reaction:**
  - Removes the repo from the collapsed set, so its sidebar section expands.
  - Sets `app.selection`. That releases the old worktree's heavy state, restores the new one's, switches the sidebar to the repo's space if needed, and moves the worktree to the top of the recent list.
  - Dismisses the sheet.
- **Edge cases:** Return with no items does nothing.
- **Source:** QuickSwitcher.swift:36, 48, 67-71; Model.swift:25-37.

### QS-07 Dismiss without choosing
- **Trigger:** Esc (`onExitCommand`), or the sheet's own dismiss.
- **Reaction:** closes, with no selection change.
- **Source:** QuickSwitcher.swift:56.

## File tree component

### FT-01 Ordering and compaction
- **Rules:**
  - At every level, directories come before files, then names compare with `localizedStandardCompare` (natural number order: file2 before file10).
  - A chain of single-child directories is compressed into one row "a/b/c". Its fold key is the full path "a/b/c".
- **Source:** FileTree.swift:15-60.

### FT-02 Row look
- **Directory row:**
  - A chevron (0° collapsed, 90° open), then the folder or folder.fill icon.
  - The name in secondary, head-truncated, then the trailing accessory.
- **File row:** a 12 pt spacer, then the host's content.
- **Indentation:** 10 pt per depth, with a 1 pt indent guide per level and no separators.
- **TreeFileRow:**
  - The file icon, then the name, middle-truncated.
  - Then "+N" in green and "−N" in red (monospaced digits), or "binary" for a binary file.
  - Then the StatusIcon: plus.square green for added/untracked, minus.square red for deleted, arrow.right.square blue for renamed/copied, exclamationmark.square orange for conflicted, otherwise dot.square yellow for modified or type change.
  - A viewed file is drawn at 50% opacity. Help = path.
- **Source:** FileTree.swift:88-104, 143-186; FileIcons.swift:69-107.

### FT-03 Fold or unfold a directory
- **Trigger:** single click a directory row.
- **Reaction:** toggles it in the collapsed set. Directories can't be selected, so the file selection stays.
- **Edge cases:**
  - Keyboard arrows move the native List selection between files only. There is no keyboard way to fold.
  - While the PR filter is on, the binding is constant, so clicks do nothing and everything stays expanded.
- **Source:** FileTree.swift:107-110, 135-137; PullRequestsView.swift:472.

### FT-04 Tree context menu
- **Trigger:** right-click a row.
- **Items on a directory row:**
  - "Expand" or "Collapse"
  - the host item; the PR tree adds "Mark Folder as Viewed" or "Mark Folder as Not Viewed" (Not Viewed when every file under it is viewed)
  - divider
- **Items on every row:**
  - "Expand All" (empties the set)
  - "Collapse All" (every ancestor directory of every path)
- **Reaction:** the PR folder item marks every filtered file under the folder viewed, or clears them all if they all already were. One write.
- **Source:** FileTree.swift:111-119, 128-133; PullRequestsView.swift:473-476; PullModel.swift:178-185.

### FT-05 Select a file
- **Trigger:** click a file row, or use the arrow keys.
- **Reaction:** single selection, bound weakly to the host's `selectedPath`:
  - Changes Full: the browser.
  - PR: the pull.
  - History: the commit model.

  ⌘A is swallowed and does nothing (it used to hang on big trees, vault 0.6.1).
- **Source:** FileTree.swift:87, 123-125, 229-231.

### FT-06 Where it is used
- **History commit detail:**
  - The tree sits in a resizable pane (180-300 pt, `commitTreeWidth`), listing only the changed files.
  - With no files and nothing loading, it shows "No File Changes".
  - It has no directory accessory.
- **PR Files tab:**
  - 250-460 pt (`prTreeWidth`).
  - The "Filter files" field, then a progress bar and "N of M files viewed".
  - Directory accessory "done/total": accent colour when complete, secondary otherwise; hidden when the folder holds no files.
  - A "bubble.left.fill N" label for comment counts.
- **Changes Full:** CB-02.
- **Changes Compact list:**
  - It reuses `FileTree.rows` and `.treeRow` per section with its own fold set, keyed "section/dir".
  - Clicking a folder toggles it.
  - It has no Expand All / Collapse All menu.
- **Source:** History.swift:558-565; PullRequestsView.swift:429-501; ChangesView.swift:385-412.

### FT-07 Filtering (PR)
- **Trigger:** type in "Filter files".
- **Reaction:** a case-insensitive substring match on the full path, over the changed files (Compact) or the whole tree (Full). Results are cached until the tree or the query changes. All folders are expanded while filtering.
- **Source:** PullRequestsView.swift:431, 466-472; PullModel.swift:149, 199-206, 216-227.

### FT-08 File icons
- **Rules, in order:**
  1. The exact lowercased file name: cargo.toml → rust, flake.nix → nix, dockerfile → docker, package.json → nodejs, yarn.lock → yarn, tsconfig.json → tsconfig, readme.md → readme, chart.yaml → helm, go.mod → go, …
  2. A name starting with `.env` → tune; starting with `dockerfile` → docker.
  3. `.test.` or `.spec.` with ts → test-ts, tsx or jsx → test-jsx, js → test-js.
  4. yaml or yml under `/k8s/` or `/kubernetes/` → kubernetes.
  5. By extension (about 90 entries, e.g. sql → database, yml → yaml).
  6. Otherwise "file". When the SVG is missing, the SF Symbol "doc".
- **Render:** 16 pt by default (14 pt in popovers). Images are cached per icon name.
- **Source:** FileIcons.swift:5-67.

### FT-09 Resizable split pane
- **Trigger:** drag the 1 pt divider (9 pt hit area, resize cursor on hover). Double-click it to reset.
- **Reaction:**
  - The width is clamped between min and max and persisted under its `@AppStorage` key: `changesListWidth`, `historyListWidth`, `commitTreeWidth`, `prTreeWidth`.
  - Double-click restores `max × defaultFraction`.
- **Source:** FileTree.swift:190-223.

---

# 6. History (log, scopes, refs, commit detail, commit actions, interactive rebase, undo)

**Things testers should know first:**
- History has no "file" or "folder" scope. Its scopes are Current Branch, All Branches, any local branch, any remote branch, and Stashes. File history lives in the diff header's File History panel.
- Loupe has no reset (soft/mixed/hard), no checkout of a commit, and no create branch/tag from a commit. Reset only appears as the undo mechanism (`reset --keep` or `reset --soft`).
- There is no git redo. Redo (⌘⇧Z) only works inside text fields.
- The vault is stale: it says History pages 300 commits. The code pages 100 (`History.swift:154`, commit 5f97dac).


I read every line of History.swift (698 lines), InteractiveRebase.swift (573) and Undo.swift (407). I also read the parts of Model.swift, App.swift, Shortcuts.swift, ChangesView.swift, Conflicts.swift and FileInspector.swift that wire these features, plus the vault note. I did not change any files.

**Things testers should know first:**
- History has no "file" or "folder" scope. Its scopes are Current Branch, All Branches, any local branch, any remote branch, and Stashes. File history lives in the diff header's File History panel.
- Loupe has no reset (soft/mixed/hard), no checkout of a commit, and no create branch/tag from a commit. Reset only appears as the undo mechanism (`reset --keep` or `reset --soft`).
- There is no git redo. Redo (⌘⇧Z) only works inside text fields.
- The vault is stale: it says History pages 300 commits. The code pages 100 (`History.swift:154`, commit 5f97dac).

## History log

### HI-01 Open History tab
- **Trigger:** Toolbar segmented picker "History" (principal placement); menu View › History; ⌘2 (rebindable `historyTab`). All three write `UserDefaults["tab"]="history"`.
- **Preconditions:** A worktree is selected in the sidebar.
- **Render:**
  - Left/right `ResizableSplit` (key `historyListWidth`, min 260, max 420, default fraction 340/420).
  - Left side: the search field and the scope menu in an 8pt-padded HStack, a Divider, then the commit List (`.inset` style).
  - Right side: the commit detail (min width 420).
  - While the first page loads, the list is blank (no spinner) and the detail shows "Select a Commit" (icon `clock.arrow.circlepath`).
- **Reaction:**
  - `.task(id: "<headOID> <refsVersion>")` builds that key. If it matches `model.loadedKey` and commits are non-empty, nothing happens (no git calls).
  - Otherwise it runs `reload()` and then `rebase.refreshLocal()`.
  - `reload()` runs:
    - `git log --format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1e --decorate=full --topo-order --skip=0 -n 100 HEAD --`
    - `git for-each-ref --format=%(refname) --sort=-committerdate refs/heads refs/remotes`
    - `git rev-list --first-parent --parents -n 1000 HEAD --` (from `refreshLocal`)
  - The first commit is auto-selected.
- **Edge cases:**
  - Unborn branch: `git log` fails with `allowFailure`, giving an empty list and "No Commits".
  - The selection survives a reload if the SHA is still in the list.
  - If the selection is gone, the top commit is selected.
  - If the selection stayed but its files were released (another worktree was shown), the detail is re-selected. See BROKEN #1: the task guard usually prevents this.
- **Source:** ChangesView.swift:16-17,63-68; App.swift:89; Shortcuts.swift:96; History.swift:163-173,288-333; InteractiveRebase.swift:235-244

### HI-02 Reload only when HEAD or remote refs moved
- **Trigger:** Return to the History tab, or any change to `headOID` (status refresh after commit, checkout or reset, often driven by the FS watcher) or to `refsVersion` (bumped after Fetch, Pull or Push).
- **Preconditions:** History was already loaded once.
- **Render:** Unchanged if the key is unchanged. Otherwise the list reloads in place and the selection is kept.
- **Reaction:**
  - Same key and commits loaded: zero git processes.
  - `remoteOp` (fetch, pull, push) runs `refsVersion += 1`, so History reloads the next time it is shown.
- **Edge cases:**
  - The key ignores scope, search and stash changes. A stash pushed from Changes calls `history.reload()` directly (Model.swift:488).
  - A scope change reloads through `setScope`.
  - (vault) Commit 1d10383: switching tabs costs ~8 git processes before, 0 after.
- **Source:** History.swift:137,327-333; Model.swift:227,293,503-509

### HI-03 Commit row content
- **Trigger:** None (rendering).
- **Preconditions:** Commits whose authors, refs and dates vary.
- **Render:** An HStack of graph cell, then avatar, then details.
  - **Avatar:** 26pt, 8pt top padding. The forge avatar URL comes from `AuthorAvatars`; without one it shows initials.
  - **Line 1:** a 6pt accent dot when the commit is HEAD (tooltip "HEAD"), then the subject on one line. The subject is semibold for HEAD.
  - **Line 2:** the 8-character short SHA (caption, monospaced, secondary), the author (caption, secondary), a spacer, then the relative date ("named" presentation, tertiary).
  - **Line 3:** only when the commit has refs (see HI-05).
  - **Row tooltip:** `subject\nauthor <email>\n<abbreviated date, short time>`.
  - Row insets: leading 6, trailing 8, top and bottom 0, so graph lines run row to row.
- **Reaction:**
  - Each row's `.task` requests its avatar. The request runs only if the worktree has a forge client and a remote, and the email has not been resolved yet (cache `UserDefaults["authorAvatars"]`, keyed by lowercased email).
  - Requests go out one at a time through `client.commitAuthor(repo, sha)`. A failure caches `""`, which means initials forever.
  - The cache is saved once per drained batch.
- **Edge cases:**
  - `isHead` compares against `worktree.headOID`. In non-current scopes HEAD may be absent, so no row gets the dot.
  - A forge with no account shows initials only.
- **Source:** History.swift:413-457,664-698

### HI-04 Commit graph lanes
- **Trigger:** None (rendering).
- **Preconditions:** Non-linear history, such as a merge commit and branches diverging in the All scope. A linear history shows no graph.
- **Render:** A `Canvas` per row.
  - Lanes are 11pt wide. The node sits at y=21 and is an 8pt dot in the lane color with a 1.5pt ring in the window background color. Strokes are 1.6pt.
  - Lane 0 (the main line) uses the accent color. Other lanes cycle through green, purple, pink, teal, yellow, red, indigo, mint, cyan, brown, orange.
  - "Through" lanes are straight vertical lines. "Into" and "out" lanes are quad curves to and from the node.
- **Reaction:**
  - Layout runs off the main thread with `Graph.layout(commits, main: tip)`.
  - The tip is HEAD (current scope: the first commit). In other scopes it is `git rev-parse HEAD` if HEAD is among the loaded commits, otherwise the first commit.
  - The main line is the first-parent chain from the tip, pinned to lane 0.
- **Edge cases:**
  - The width is capped at 12 lanes. Rows whose node column is past the cap draw lines only, with no dot.
  - If the maximum width is 1 or less, there is no graph at all (`graphWidth` 0).
  - No graph while a search is active or in the Stashes scope.
  - The graph is recomputed after every page. If a newer fetch replaced the list meanwhile, the stale layout is discarded.
- **Source:** History.swift:80-131,200-211,459-491

### HI-05 Ref badges
- **Trigger:** None (rendering). Hover a badge to see its name as a tooltip.
- **Preconditions:** A commit carrying HEAD→branch, a local branch, a remote branch, a tag and detached HEAD. Many refs on one commit test the "+N" overflow.
- **Render:** Capsule badges. Each one is font caption2 medium, middle-truncated, with a 12% tint background. A selected row turns badges white on 20% white.
  - `HEAD -> refs/heads/x`: name x, `arrow.triangle.branch` icon, accent tint.
  - `tag: refs/tags/v1`: `tag` icon, orange.
  - `refs/remotes/o/x`: `cloud` icon, secondary.
  - `refs/heads/x`: branch icon, green.
  - Anything else (detached `HEAD`, refs/pull/*, `origin/HEAD` handled as a remote): `smallcircle.filled.circle`, accent.
  - `ViewThatFits` shows as many badges as fit, then a "+N" capsule whose tooltip lists the remaining names one per line. If nothing fits, one truncated badge.
- **Reaction:** None.
- **Edge cases:** `--decorate=full` output is split on ", ".
- **Source:** History.swift:444-454,493-530

### HI-06 Paging: load more on scroll
- **Trigger:** Scroll until the last row appears (`onAppear` of the last commit).
- **Preconditions:** More than 100 commits in scope (to exercise several pages, more than 200).
- **Render:** While the next page loads, a small `ProgressView` row sits at the bottom of the list.
- **Reaction:**
  - Runs `git log … --skip=<count> -n 100 <revs> --` and appends the result.
  - The graph is laid out again over all loaded commits.
  - `exhausted` is set when a page returns fewer than 100 commits; after that no more fetches happen.
- **Edge cases:**
  - Guarded against a fetch already in flight or an exhausted list.
  - Applies to non-current scopes and to a search.
  - In the Stashes scope everything comes in one call (exhausted at once).
  - See BROKEN #2: an unsubmitted search leaks into paging.
- **Source:** History.swift:154,182-199,306,308

### HI-07 Arrow-key and keyboard commit navigation
- **Trigger:**
  - ↑/↓ in the focused list (native List selection).
  - Review › Next File (⌥↓) or Review › Previous File (⌥↑). In History these move to the next or previous **commit**.
- **Preconditions:** History is showing at least 2 commits.
- **Render:** The selection moves and the detail loads.
- **Reaction:** `NotificationCenter .moveFile(±1)` runs `model.move(d)`, which clamps to [0, count-1]. With no selection it selects the first commit.
- **Edge cases:**
  - ⌥↓ on the last loaded row does not trigger paging; it stays put.
  - Single selection only.
- **Source:** App.swift:95-96; Shortcuts.swift:100-101; History.swift:270-273,298,336

### HI-08 Refresh
- **Trigger:** Review › Refresh, ⌘R.
- **Preconditions:** None.
- **Render:** The list reloads.
- **Reaction:**
  - The `.refresh` notification runs `history.reload()` (`log`, `for-each-ref`, plus `rev-parse HEAD` and `rev-list` when the scope is not current).
  - WorktreeView also runs `refreshStatus()`.
  - Not keyed: it always reloads.
  - `refreshLocal` is not re-run.
- **Edge cases:** The currently typed search text is applied even if it was never submitted.
- **Source:** App.swift:100; History.swift:337; ChangesView.swift:100

## Scopes

### HI-09 Scope menu
- **Trigger:** Borderless menu button to the right of the search field.
  - Label: the scope title, truncated to 17 characters plus "…" when longer than 18.
  - Icon: `tray` for Stashes, `point.3.filled.connected.trianglepath.dotted` for All Branches, `arrow.triangle.branch` otherwise.
  - Tooltip: "Which history to show: <title>".
- **Preconditions:** Local and remote branches exist, plus stashes.
- **Render:** The menu items are, in order:
  1. "Current Branch"
  2. "All Branches"
  3. "Local Branches" ▸ (submenu, only when local branches exist; sorted by committer date, newest first)
  4. "Remote Branches" ▸ (only when remote branches exist; `*/HEAD` excluded)
  5. Divider
  6. "Stashes"
  
  There is no checkmark on the active scope.
- **Reaction:**
  - `setScope` ignores the scope already active. Otherwise it clears commits, graph and selection, then reloads.
  - Git revs per scope:
    - current: `HEAD`
    - all: `--branches --remotes --tags HEAD`
    - branch: `<b>` (for example `origin/main`)
  - For non-current scopes it also runs `git rev-parse HEAD` and `git rev-list -n 20000 <revs minus HEAD> ^HEAD --` to compute `notInHead`.
- **Edge cases:**
  - If the scoped branch disappears (deleted or pruned), the next reload falls back to Current Branch and reloads again.
  - The scope lives in memory per worktree. It survives tab switches but not a relaunch.
- **Source:** History.swift:17-36,175-180,190-215,351-369

### HI-10 Stashes scope
- **Trigger:** Scope menu › Stashes; or in Changes, the "N stashes →" row under the file list (`StashesLink`). That row sets the scope to Stashes and the tab to History.
- **Preconditions:** At least one stash, ideally one that includes untracked files.
- **Render:**
  - The search placeholder becomes "Search stashes".
  - Rows show the stash subject ("On main: msg" or "WIP on …"), with no ref badges and no graph.
  - Empty: "No Stashes" (icon `tray`), described as "Stash changes from the Changes tab to set them aside."
- **Reaction:**
  - `git stash list --format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%gd%x1f%s%x1e`. The selector (`stash@{n}`) is taken from the refs field.
  - All stashes load at once (exhausted).
  - Search filters on the subject only, client side and case-insensitive.
- **Edge cases:** No drag, no rebase menu (scope ≠ current).
- **Source:** History.swift:51-55,190-195,291,315-316; ChangesView.swift:608-616

### HI-11 Search / filter
- **Trigger:** Rounded text field with placeholder "Search" and tooltip "Commit message or author". Press Return (`onSubmit`) to apply. Clearing the field to empty reloads automatically.
- **Preconditions:** Commits with distinctive messages and authors.
- **Render:**
  - The filtered list has no graph.
  - Empty result: "No Matches" (icon `magnifyingglass`).
  - Rebase drag-and-drop and the rebase context-menu section are disabled while the field is non-empty.
- **Reaction:**
  - Runs `git log … -i --grep=<s> --author=<s> <revs> --`. Git ORs the two conditions; matching is case-insensitive.
  - Paging still applies.
  - In the Stashes scope, the filter matches the subject.
- **Edge cases:**
  - An invalid regex (for example `foo(`) makes git fail, which with `allowFailure` gives "No Matches" and no error.
  - Typing without Return does nothing until the next reload (scope change, ⌘R, paging).
  - The selection is kept if the selected commit still matches; otherwise the first match is selected.
- **Source:** History.swift:41-49,193,203-208,291-293,304,313-314,335,397

## Commit detail

### HI-12 Commit selection and detail load
- **Trigger:** Click a row (List selection), arrow keys, or ⌥↓/⌥↑.
- **Preconditions:** Commits of each kind: normal, merge, root, rename-only, binary, LFS, markdown file.
- **Render:**
  - A spinner (small, in the header's stats line) shows while the detail loads.
  - With no files yet and still loading, the right pane shows a ProgressView.
  - With nothing selected: "Select a Commit".
- **Reaction:**
  - Cancels the previous detail task.
  - Runs `git show -s --format=%B <sha>` and, concurrently, `git show --format= --no-color --no-ext-diff -M --diff-merges=first-parent <sha>`.
  - For a stash it runs `git stash show -p --include-untracked --no-color --no-ext-diff -M stash@{n}` instead.
  - Files are sorted in tree order.
  - The previously selected file path is kept if the new commit has it; otherwise the first file is selected.
- **Edge cases:**
  - A merge is diffed against its first parent.
  - A root commit is diffed against the empty tree.
  - Errors show the "Git" alert. Cancellation is silent.
- **Source:** History.swift:67-77,225-243,588-590,603

### HI-13 Commit detail header
- **Trigger:** Select a commit.
- **Preconditions:**
  - A commit with a multi-paragraph Markdown body.
  - A merge commit.
  - A commit whose parent is not loaded yet (beyond page 1).
- **Render:** Bar background, 14pt horizontal and 10pt vertical padding.
  - **Row 1:** the subject (title3 semibold, selectable, up to 3 lines), a spacer, then the action buttons (HI-15), small and fixed size.
  - **Body:** everything after the first line of `%B`, rendered as Markdown (callout) in a ScrollView capped at 120pt. Hidden when empty.
  - **Row 3 (callout font):**
    - 18pt avatar, author (medium), date (abbreviated, short time, secondary).
    - Copy button labeled with the short SHA and a `doc.on.doc` icon (borderless, tooltip "Copy full SHA").
    - "parent" or "merge of" (tertiary), followed by each parent's 8-character SHA as a link button.
    - Spacer, "N files" / "1 file", "+adds" in green, "−dels" in red (U+2212 minus, monospaced digits), and a spinner while loading.
- **Reaction:**
  - The copy button copies the full SHA to the pasteboard.
  - A parent link selects that parent if it is loaded. Otherwise it does nothing; its tooltip reads "Parent not loaded yet — scroll the list to load more" ("Go to parent" when loaded).
- **Edge cases:**
  - A root commit shows no parent section.
  - Stash entries are merge commits, so they show "merge of" with 2-3 parent links that never resolve.
- **Source:** History.swift:619-659

### HI-14 Files / Overview switch and the hosted diff
- **Trigger:**
  - Toolbar view switch: Overview (`square.grid.2x2`) or Files (`doc.text`). Persisted as `AppStorage "commitOverview"`. Not offered for a stash.
  - Toolbar Compact/Full picker (⌥⌘F, `prFullMode`). Shown only when the Files view is active and a file is selected.
  - The Diff / Before / After side picker in the DiffHeader (`diffSide`).
- **Preconditions:** A commit with several files across directories, a rename, a binary file, an LFS pointer, a mode-only change and a `.md` file.
- **Render (Files):**
  - `ResizableSplit` (key `commitTreeWidth`, 180-300): a `FileTreeList` with `TreeFileRow` entries (badge, +/−, binary mark) and collapsible directories. The collapsed state persists across commits.
  - The tree shows "No File Changes" when it is empty and not loading.
  - The right side (min width 300) shows a `DiffHeader` with the path, the old path if renamed, and +/−. The header offers Blame and File History buttons, but not for deleted files or stashes.
  - Below the header, depending on the file:
    - "Binary File" (icon `doc.zipper`)
    - the LFS object view
    - "No Content Changes" with "Mode change or rename only."
    - a `DiffTableView`
  - `markdownPreview` loads `git show <sha>:<path>`.
- **Render (Overview):** `ChangeOverview` from the first parent (or the empty tree) to the commit. Clicking an item there switches to Files, selects the path and, if a line was given, reveals it.
- **Reaction:**
  - Full mode or Before/After loads the whole file: `git show --format= --no-color --no-ext-diff -M --diff-merges=first-parent -U1000000 <sha> -- <old> <new>`.
  - For a stash: `git stash show -p --no-color --no-ext-diff -M -U1000000 stash@{n} --`.
  - Whole files are cached per "sha path" for the commit showing only.
- **Edge cases:**
  - Added, deleted and binary files never load a whole file.
  - The File History panel lists commits from HEAD (`git log --follow`), not from the commit being viewed. Blame does use the commit's revision.
- **Source:** History.swift:532-617; ChangesView.swift:73-95,551-583; FileInspector.swift:38-47,69-199

## Commit actions

### HI-15 Header one-click actions
- **Trigger:** Buttons in the detail header's right corner.
- **Preconditions:**
  - For Revert: a commit contained in HEAD.
  - For Cherry-pick: a commit not in HEAD, which requires the All or a branch scope.
  - For the stash buttons: a stash.
- **Render:**
  - Stash: "Drop…" (destructive), "Apply" (tooltip "Apply the changes and keep the stash"), "Pop" (prominent, tooltip "Apply the changes and drop the stash").
  - Commit not in HEAD: "Cherry-pick onto <branch>" (tooltip "Apply this commit on top of <branch>").
  - Otherwise: "Revert" (tooltip "Make a new commit on <branch> that undoes this one").
  - `<branch>` is the worktree's branch, or "HEAD" when detached.
- **Reaction:** Same as HI-17, HI-18 and HI-19.
- **Edge cases:**
  - In the Current scope, `notInHead` is always empty, so every commit offers Revert.
  - When a scope has more than 20000 commits not in HEAD, the ones beyond the cap show Revert.
- **Source:** History.swift:142,212-215,371-384,627

### HI-16 Commit context menu
- **Trigger:** Right-click a commit row.
- **Preconditions:** A remote with a forge account (for Open in Browser).
- **Render:**
  - Normal commit, in order:
    1. "Cherry-pick onto <branch>" (if not in HEAD) or "Revert Commit"
    2. Divider
    3. The rebase section (RB-02) and a Divider, only when the scope is Current and the search is empty
    4. "Copy SHA"
    5. "Copy Subject"
    6. "New Worktree from Here…"
    7. "Open in Browser" (only when the worktree has a remote and an account)
  - Stash: "Pop", "Apply", "Drop…" (destructive), Divider, "Copy Message".
- **Reaction:**
  - Copy SHA copies the full SHA. Copy Subject and Copy Message copy the subject line only, not the body.
  - Open in Browser opens `<account.baseURL>/<owner>/<name>/commit/<sha>` with NSWorkspace.
- **Edge cases:** The labels differ from the header: the header says "Revert", the menu "Revert Commit".
- **Source:** History.swift:305,386-410

### HI-17 Cherry-pick a commit
- **Trigger:** Header or context menu: "Cherry-pick onto <branch>".
- **Preconditions:** All or branch scope, and a commit on another branch not in HEAD. Also test a merge commit not in HEAD, and a commit that conflicts with HEAD.
- **Render:** No confirmation dialog.
- **Reaction:**
  - Runs `git cherry-pick [-m 1 if a merge] <sha>`.
  - Undo recipe: `.reset("Cherry-pick of <short8>", offerRevert: true)`, recorded only if HEAD moved and no operation was left in progress. No toast.
  - History then reloads.
  - If the command fails:
    - With unmerged files (`git diff --name-only --diff-filter=U`): `stoppedOnConflicts`, so the tab switches to Changes and the banner reads "Cherry-picking <subject>".
    - Without unmerged files: the "Git" alert with git's stderr.
- **Edge cases:**
  - An empty cherry-pick (already applied) fails with no unmerged files, giving an alert while CHERRY_PICK_HEAD stays behind. The Changes banner then offers Abort and Continue.
  - When it stops on conflicts, Continue records the undo as "Cherry-pick" (UN-06).
- **Source:** History.swift:247-259,339

### HI-18 Revert a commit
- **Trigger:** Header "Revert" or context menu "Revert Commit".
- **Preconditions:** A commit in HEAD. Also test a merge commit and one whose revert conflicts.
- **Render:** No confirmation.
- **Reaction:**
  - Runs `git revert --no-edit [-m 1] <sha>`.
  - Undo: `.reset("Revert of <short8>", offerRevert: true)`.
  - Conflicts are handled as in HI-17: switch to Changes, banner "Reverting …".
- **Edge cases:** Reverting a commit not in HEAD is never offered.
- **Source:** History.swift:260-262

### HI-19 Stash Apply / Pop / Drop
- **Trigger:** In the Stashes scope: header buttons Apply, Pop or Drop…, or the same items in the context menu.
- **Preconditions:**
  - Stashes, including one with untracked files.
  - A dirty worktree that conflicts with a stash.
- **Render:** Drop… opens a confirmation dialog:
  - Title: "Drop stash@{n}?"
  - Button: "Drop Stash" (destructive), plus Cancel.
  - Message: "The stash is removed from the list. Edit › Undo brings it back."
  
  Apply and Pop have no confirmation.
- **Reaction:**
  - Commands: `git stash apply stash@{n}`, `git stash pop stash@{n}`, `git stash drop stash@{n}`.
  - Undo recipes:
    - Apply: `.stash("Apply Stash", applied: true, dropped: false)`, no toast.
    - Pop: `"Pop Stash"`, applied and dropped, toast.
    - Drop: `"Drop Stash"`, dropped, toast.
  - History reloads afterwards.
- **Edge cases:**
  - A conflict on apply sends you to Changes. Because no operation is in progress, the banner reads "Conflicts" with no Continue or Abort buttons.
  - "Local changes would be overwritten" gives the "Git" alert.
  - A pop that conflicts keeps the stash (git behaviour) and records no undo.
- **Source:** History.swift:263-268,340-342,375-378,387-392

### HI-20 New Worktree from Here
- **Trigger:** Context menu "New Worktree from Here…".
- **Preconditions:** Any commit; the repo is registered in Loupe.
- **Render:** Sheet "New Worktree" (grouped form, 480 wide):
  - Segmented picker "New branch" / "Existing branch" (New is the default).
  - "Name" field (placeholder "feature/my-change").
  - "Based on" picker: "HEAD", then an extra item showing the commit's SHA truncated to 10 characters (preselected), then all branches.
  - "Location" field with a "Choose…" button (NSSavePanel).
  - An error, when there is one, in red.
  - Toolbar: Cancel, and Create (disabled when the name or path is empty, or while working).
- **Reaction:**
  - The suggested path is `<parent of repo>/<repo>-<branch with / → - and "origin-" removed>`. It stops updating once edited.
  - Create runs `git worktree add -b <name> <path> <sha>`, refreshes the worktree list and selects the new worktree.
- **Edge cases:**
  - Switching to Existing branch ignores the base.
  - An existing `origin/x` without a local branch creates local `x` tracking it.
  - git errors show inline.
- **Source:** History.swift:346-348,403; App.swift:617-706

### HI-21 Merge PR → History with the merge on top
- **Trigger:** Merging a PR in the PR detail (other agent's flow) calls `showMerged(into: base)`.
- **Preconditions:** An open PR with a mock forge that really merges and serves git (vault: the memtest mock forge supports this).
- **Render:**
  - The fetch runs with busy "Fetching <base>" (the toolbar Fetch icon spins).
  - Then the History tab opens with scope "<remote>/<base>" and the top commit (the merge) selected.
- **Reaction:**
  - Runs `git fetch <remoteName> <base>`, bumps `refsVersion` and refreshes the worktree list.
  - Sets `tab=history`.
  - If the scope is already `.branch(remote/base)`: clears `selectedSHA` and reloads. Otherwise `setScope`, which clears and reloads.
  - The reload selects `commits.first`.
- **Edge cases:**
  - If the fetch fails, the scope is set anyway. If the remote branch does not exist, the scope falls back to Current Branch.
  - The `refsVersion` bump can cause a second reload through the task key (harmless).
  - (vault) This is the 0.9.0 change "merge → History".
- **Source:** Model.swift:511-519; History.swift:166,172

### HI-22 Loading, empty and error states
- **Trigger:** Various.
- **Preconditions:** An empty repo, an empty search result, no stashes, a git failure.
- **Render:**
  - "No Commits" (icon `clock.arrow.circlepath`).
  - "No Matches".
  - "No Stashes".
  - These show only when the list is empty and not loading.
  - A page-load spinner row.
  - A detail ProgressView or a small header spinner.
  - Alert titled "Git" with an OK button for `model.error`. It is shown for log, refs, cherry-pick, revert, stash and rebase errors.
- **Reaction:** OK clears the error.
- **Edge cases:** Cancellation errors are never shown (`userMessage` returns nil).
- **Source:** History.swift:216,241,308,312-319,343-345; Git.swift:5-8

## Interactive rebase

### RB-01 Which commits are rewritable
- **Trigger:** History load (`refreshLocal`), and after every rewrite.
- **Preconditions:** HEAD with N linear commits above a merge, and a root commit.
- **Render:** None directly. This set enables drag-and-drop (RB-03).
- **Reaction:**
  - Runs `git rev-list --first-parent --parents -n 1000 HEAD --`.
  - Commits are added until the first line that does not have exactly 2 IDs, which means a merge or the root.
- **Edge cases:**
  - The root commit and everything at or below the first merge cannot be dragged.
  - Pushed commits can be dragged; a confirmation asks first (RB-08).
  - The context-menu items are not limited to this set (see RB-02).
- **Source:** InteractiveRebase.swift:223-244

### RB-02 Rebase context-menu section
- **Trigger:** Right-click a row. The section exists only when the scope is Current Branch and the search is empty.
- **Preconditions:**
  - A linear branch.
  - The HEAD commit (for Move Up disabled).
  - A merge (for disabled items).
  - The root commit (no parent).
- **Render:** Items, in order:
  1. "Reword…" (disabled for a merge)
  2. "Edit (Stop Here)" (disabled for a merge)
  3. "Squash into Parent" (disabled unless exactly 1 parent)
  4. "Fixup into Parent" (same)
  5. "Move Up" (disabled for HEAD or a merge)
  6. "Move Down" (disabled unless 1 parent)
  7. "Drop…" (destructive, disabled for a merge)
  8. Divider
  9. "Interactive Rebase from Here…"
  
  The whole group is disabled while a rebase runs.
- **Reaction:** See RB-04 to RB-07, RB-09 and RB-10.
- **Edge cases:**
  - The items stay enabled for a commit below a merge. Running one fails with the merge error alert (RB-12).
  - Squash or Fixup into a parent that is a merge also errors.
- **Source:** History.swift:397-400; InteractiveRebase.swift:417-437

### RB-03 Drag-and-drop reorder, squash and fixup in the History list
- **Trigger:** Drag a row onto another row:
  - Top 30% of the target row: drop above.
  - Bottom 30%: drop below.
  - Middle: drop onto, which squashes. Holding ⌥ during the hover or at drop time makes it a fixup.
- **Preconditions:** Scope Current, empty search, and both the source and target rows rewritable (in `local`, not a merge). Use 3 or more local commits.
- **Render:**
  - Above or below: a 2pt accent line at the top or bottom of the target row.
  - Onto: a rounded accent border with a 10% fill and a "Squash" or "Fixup" capsule on the right.
  - Invalid targets show the forbidden cursor (the target is the source itself, or a rebase is running).
  - Non-rewritable rows have no drag or drop at all.
- **Reaction:**
  - On drop, the payload SHA is checked against `dragging`.
  - The oldest of source and target defines the range: `oldest^..HEAD`. The rewrite then uses `RebaseTodo.move`.
  - Onto inserts the moved commit directly above the target with action squash or fixup, so it melds into the target.
  - Labels: "Reorder Commits", "Squash" or "Fixup".
  - Then the confirmation checks and execution of RB-08 and RB-11 apply.
- **Edge cases:**
  - A drop that ends up as the same order does nothing (no change detected).
  - A stale `dragging` value left by a drag dropped elsewhere is ignored because the payload must match.
  - Squash combines messages as the target message, a blank line, then the moved message.
- **Source:** InteractiveRebase.swift:103-114,287-291,338-414; History.swift:304

### RB-04 Reword
- **Trigger:** Context menu "Reword…".
- **Preconditions:** A non-merge commit with a multi-line message.
- **Render:** Sheet:
  - Headline "Reword <sha8>".
  - Monospaced TextEditor, minimum 480×160, prefilled with the full message (`git show -s --format=%B`).
  - Buttons: "Cancel" (Esc) and "Reword" (default action, disabled when the message is blank).
- **Reaction:**
  - The plan sets the commit to `.reword` with `newMessage`.
  - The todo becomes `pick <sha>` followed by `exec git commit --amend --allow-empty --quiet -F '<tmp>/message-1'`.
  - Label "Reword".
- **Edge cases:**
  - An unchanged message means no change, so it silently does nothing.
  - Return inside the TextEditor inserts a newline instead of submitting.
  - A pushed commit triggers the RB-08 confirmation.
- **Source:** InteractiveRebase.swift:317-326,473-494,41-74

### RB-05 Edit (Stop Here)
- **Trigger:** Context menu "Edit (Stop Here)".
- **Preconditions:** A non-merge commit.
- **Render:**
  - After the rebase stops, the app switches to the Changes tab.
  - Banner: blue `pencil.circle.fill`, "Rebasing <subject of stopped-sha>", "Stopped to edit this commit. Change it (commit with Amend), then continue."
  - Buttons: "Abort Rebase" and "Continue".
- **Reaction:**
  - The todo line is `edit <sha>`. `git rebase` exits 0 with rebase-merge present, so the rebase counts as stopped.
  - No undo is recorded at that point; Continue records it (UN-06).
  - Label "Edit".
- **Edge cases:** Edit is detected by the `rebase-merge/amend` file.
- **Source:** InteractiveRebase.swift:62,171,279,426; Model.swift:345-361; Conflicts.swift:95-117

### RB-06 Squash or Fixup into Parent
- **Trigger:** Context menu "Squash into Parent" or "Fixup into Parent".
- **Preconditions:** A commit with exactly 1 parent. Also test when the parent is the root, which uses `--root`.
- **Render:** No sheet. The confirmation appears only if a pushed commit would be rewritten.
- **Reaction:**
  - The range starts at the parent.
  - Squash concatenates both messages (separated by a blank line) through an `exec … amend`. Fixup keeps the parent's message only.
  - Labels: "Squash into Parent" or "Fixup into Parent".
- **Edge cases:** A parent that is a merge gives the merge error.
- **Source:** InteractiveRebase.swift:297-303

### RB-07 Move Up / Move Down
- **Trigger:** Context menu "Move Up" or "Move Down".
- **Preconditions:** A linear branch with 3 or more commits.
- **Render:** None.
- **Reaction:**
  - Move Up swaps the commit with the row above it in History (its child). The range starts at the commit. Label "Move Commit Up".
  - Move Down swaps it with its first parent. The range starts at the parent. Label "Move Commit Down".
- **Edge cases:**
  - Move Up is disabled for HEAD.
  - The child is taken from list order, which can be the tip of a merged side branch.
  - A reorder that conflicts stops the rebase (RB-13).
- **Source:** InteractiveRebase.swift:305-315

### RB-08 Confirmations: drop and pushed commits
- **Trigger:** Any context-menu or drag rewrite whose plan drops a commit or rewrites commits a remote already has.
- **Preconditions:** A branch pushed to origin, with local commits on top.
- **Render:** Confirmation dialog.
  - **Pushed commits:**
    - Title: "Rewrite a pushed commit?" or "Rewrite N pushed commits?"
    - Message: "A remote already has these commits:\n<up to 8 lines '<short> <subject>'>[\n…]\n\nAfter the rewrite the branch needs a force push, and anyone who built on them has to rebase."
    - Button: "Rewrite Anyway" (destructive).
  - **Drop only:**
    - Title: "Drop “<subject>”?"
    - Message: "Its changes leave the branch. Undo can bring it back while Loupe is open; after that, git reflog still has it."
    - Button: "Drop Commit" (destructive).
  - Both include Cancel.
- **Reaction:** Confirming resubmits with `confirmed: true`, which executes. Cancel clears the pending rewrite.
- **Edge cases:**
  - Pushed means "not in `rev-list … --not --remotes`".
  - Only commits from the first changed index onward count as rewritten. Unchanged older picks fast-forward and keep their SHAs.
  - A drop of several commits (planner only) gets no confirmation.
- **Source:** InteractiveRebase.swift:76-101,254-264,439-471

### RB-09 Drop commit
- **Trigger:** Context menu "Drop…".
- **Preconditions:** A non-merge commit, local or pushed.
- **Render:** The RB-08 dialog, always shown.
- **Reaction:** The todo line is `drop <sha>`. Label "Drop".
- **Edge cases:** A later commit that depends on the dropped one conflicts (RB-13).
- **Source:** InteractiveRebase.swift:58-59,293-295,431

### RB-10 Interactive Rebase planner sheet
- **Trigger:** Context menu "Interactive Rebase from Here…".
- **Preconditions:** 3 or more local commits, a pushed commit, and also the HEAD commit alone.
- **Render:** Sheet (min 620×420, ideal 700×520).
  - Header "Interactive Rebase" with "<N> commit(s) onto <base sha8 | the root>. Newest on top; drag to reorder. Squash and fixup meld into the commit below."
  - List, newest first. Each row has:
    - an action Picker: Pick, Reword, Edit, Squash, Fixup, Drop
    - the short SHA
    - the subject, or the first line of the edited message; struck through and secondary when Drop
    - a `cloud` icon (tooltip "A remote already has this commit") when the commit is pushed
    - for Reword and Squash, a 64pt monospaced TextEditor below. Its tooltip is "The new message" for Reword or "This commit's part of the combined message" for Squash.
  - Rows can be dragged to reorder (`onMove`).
  - Footer: an orange warning label, then "Cancel" (Esc) and "Start Rebase" (default action).
    - With a plan error, the warning is that error (for example "<short> “subj” has no older commit left to squash into.").
    - Otherwise, when pushed commits would be rewritten: "Rewrites N pushed commit(s): a, b, c…".
    - Start Rebase is disabled while there is an error or the plan changes nothing.
- **Reaction:**
  - Start submits label "Interactive Rebase" with `confirmed: true`, so the pushed and drop confirmations are skipped.
  - Squash and fixup meld into the closest older commit that is not dropped.
- **Edge cases:** A squash or fixup as the oldest item (bottom row) is an error. The range errors of RB-12 appear as the "Git" alert before the sheet opens.
- **Source:** InteractiveRebase.swift:41-74,328-333,496-573

### RB-11 Executing a rewrite
- **Trigger:** Any submitted plan.
- **Preconditions:** Optionally a dirty worktree, to exercise autostash.
- **Render:** The rebase menu is disabled and drops are refused while it runs. History reloads afterwards.
- **Reaction:**
  - Records `Rewrites` with `rev-parse HEAD`. Nothing consumes this record (BROKEN #3).
  - Writes the todo and message files to `$TMPDIR/loupe-rebase-<UUID>/`.
  - Runs `git rebase -i --autostash --no-autosquash <base|--root>` with env `GIT_SEQUENCE_EDITOR="cp '<tmp>/todo'"` and `GIT_EDITOR=true`.
  - Wrapped in an undo `.reset(label, from: head)`.
  - Then `history.reload()` and `refreshLocal()`.
- **Edge cases:**
  - These are refused up front with a "Git" alert:
    - "A rebase is already in progress. Continue or abort it first."
    - "A merge/revert/cherry-pick is in progress. Finish or abort it first."
  - A second execute while one is running is silently ignored.
  - A failure without a stop gives the alert and the temp directory is removed.
  - When the rebase stops, the temp directory is kept.
- **Source:** InteractiveRebase.swift:149-177,266-283

### RB-12 Range validation errors
- **Trigger:** Any rewrite or the planner.
- **Preconditions:**
  - More than 1000 commits to rewrite.
  - A SHA not on HEAD.
  - A merge inside the range.
- **Render:** A "Git" alert with one of:
  - "More than 1000 commits to rewrite. Use a terminal for a rebase this big."
  - "<sha8> isn't on the current branch."
  - "The commits to rewrite include the merge <short> “<subj>”. Rewriting it would flatten the merge, so Loupe leaves this to a terminal (git rebase -i --rebase-merges)."
- **Reaction:**
  - Runs `git rev-parse --verify -q <oldest>^`.
  - Runs `git log --no-show-signature --reverse -n 1001 --format=%H%x1f%P%x1f%B%x1e <base>..HEAD --`.
  - Runs `git rev-list -n 1001 <rev> --not --remotes --`.
- **Edge cases:** When `oldest^` does not resolve (the root), the range is `HEAD` and the rebase uses `--root`.
- **Source:** InteractiveRebase.swift:129-147,247-252

### RB-13 Conflicts during a rebase: Continue / Abort
- **Trigger:** A reorder, squash or drop that conflicts. Then in Changes, the banner buttons "Abort Rebase" or "Continue".
- **Preconditions:** Two commits that touch the same lines, reordered.
- **Render:**
  - The app switches to the Changes tab (`stoppedOnConflicts`).
  - Orange banner: "Rebasing <subject>", "N file(s) with conflicts. Resolve each one, then continue."
  - Continue is disabled while conflicts remain (tooltip "Resolve the conflicts first", otherwise "Finish the rebase").
  - Abort's tooltip: "Go back to how things were before the rebase".
  - Once everything is resolved: green "All conflicts are resolved."
- **Reaction:**
  - Continue runs `git -c core.editor=true rebase --continue` with `GIT_EDITOR=true`. The undo "Rebase" is recorded from orig-head when the rebase finishes.
  - Abort runs `git -c core.editor=true rebase --abort` and records nothing.
  - History reloads either way.
- **Edge cases:**
  - A Continue that stops again records nothing; the next Continue reads orig-head again.
  - Pending `exec` lines still read their messages from the temp directory.
- **Source:** Conflicts.swift:89-123; Model.swift:338-374; ChangesView.swift:234-236; History.swift:339

## Undo

### UN-01 Edit › Undo (⌘Z)
- **Trigger:** Edit menu item titled "Undo <label of the newest entry>" (just "Undo" when empty), ⌘Z.
- **Preconditions:** A worktree is selected and it has at least one undo entry.
- **Render:**
  - If a text view is first responder, the text field's own undo runs instead.
  - If there is no selection or no entries: a beep.
  - During the undo, `busy` is "Undoing".
- **Reaction:**
  - Runs `undoLast()` and then `UndoStack.undo()`.
  - Prunes entries whose recorded head key (`symbolic-ref -q --short HEAD` plus `rev-parse -q --verify HEAD`) no longer matches.
  - Checks for a pushed commit (UN-04).
  - Runs `restore` and removes the entry.
  - Shows the toast "Undone: <label>".
  - `refreshStatus` and then `history.reload()`.
- **Edge cases:**
  - When every entry was pruned at undo time, an NSAlert "Can't Undo" says "Nothing to undo in this worktree.".
  - A ⌘Z while an undo or an action is in flight is silently ignored.
  - Non-refusal errors (for example `reset --keep` refusing because of local changes) show the "Git" alert.
- **Source:** App.swift:81; Undo.swift:143-161,328-347,356-367

### UN-02 Redo
- **Trigger:** Edit › Redo, ⌘⇧Z.
- **Preconditions:** None.
- **Render:** None.
- **Reaction:** Sends `redo:` to the responder chain. Outside text fields it does nothing. There is no git redo.
- **Edge cases:** The ponytail comment documents this as intentional.
- **Source:** Undo.swift:368-370

### UN-03 Undo toast
- **Trigger:** After an action recorded with `toast: true` (Pop Stash, Drop Stash, Discard…), and after every undo.
- **Preconditions:** Pop or drop a stash in History.
- **Render:**
  - Bottom-center capsule (regular material, separator border, shadow, 18pt bottom padding), shown on every tab.
  - After an action: a checkmark icon, the label, and a semibold "Undo" button (tooltip "Undo (⌘Z)"). The button appears only while that entry is still the newest.
  - After an undo: an `arrow.uturn.backward.circle.fill` icon and "Undone: <label>", with no button.
  - Slides in from the bottom with a fade, 0.2 s ease-out, and dismisses itself after 6 s.
- **Reaction:** The Undo button runs `undoLast()`.
- **Edge cases:**
  - Recording a new entry without a toast clears the current toast.
  - Pruning the toast's entry hides the toast.
- **Source:** Undo.swift:79-86,170-174,374-407; ChangesView.swift:22

### UN-04 Refused: pushed history, with a Revert offer
- **Trigger:** Undo of a cherry-pick, revert or commit whose resulting commit is on a remote branch. Push first, then press ⌘Z.
- **Preconditions:** Cherry-pick, push, then undo.
- **Render:** NSAlert "Can't Undo": "<label> is already on <remote branch>. Undoing it would rewrite pushed history." For a cherry-pick, revert or commit (`offerRevert`) it adds "Revert it instead: a new commit that takes it back." with the buttons "Revert Commit" and "Cancel". Otherwise the alert has only OK.
- **Reaction:**
  - The check is `git branch -r --contains <sha>`, ignoring `->` alias lines.
  - Revert Commit runs `git rev-parse -q --verify <sha>^2` (is it a merge?), then `git revert --no-edit [-m 1] <sha>` under busy "Reverting", then reloads History.
  - The entry stays on the stack.
- **Edge cases:**
  - Rebase entries check the new HEAD, which is usually unpushed, so they are allowed even when the old commits were pushed.
  - Pull entries are never treated as published.
  - The revert done from this alert records no undo (BROKEN #6).
- **Source:** Undo.swift:151-157,336-346

### UN-05 Undo of cherry-pick, revert and history rewrites
- **Trigger:** ⌘Z after HI-17, HI-18 or RB-03 to RB-10.
- **Preconditions:** An action that completed without stopping and moved HEAD.
- **Render:** Menu title such as "Undo Cherry-pick of 1a2b3c4d", "Undo Revert of …", "Undo Reorder Commits", "Undo Squash", "Undo Fixup", "Undo Reword", "Undo Drop", "Undo Squash into Parent", "Undo Move Commit Up", "Undo Interactive Rebase".
- **Reaction:**
  - Runs `git reset --keep <HEAD before>`. For a rebase, the "before" value is the HEAD read before the rebase ran.
  - If there was no HEAD before (unborn), only a soft reset could apply; for these recipes nothing is recorded.
- **Edge cases:**
  - Nothing is recorded when HEAD did not move, or when an operation was left in progress.
  - Uncommitted changes that conflict with the reset make git refuse, giving the "Git" alert.
- **Source:** Undo.swift:46-49,100-111; History.swift:258,261; InteractiveRebase.swift:276

### UN-06 Undo recorded by Continue
- **Trigger:** Changes banner "Continue" after a stopped rebase, cherry-pick, revert or merge.
- **Preconditions:** A conflict-stopped cherry-pick or rebase.
- **Render:** The menu then reads "Undo Rebase", "Undo Cherry-pick", "Undo Revert" or "Undo Merge".
- **Reaction:**
  - The starting point is `preOperationHead`: for a rebase, `rebase-merge/orig-head` (or `rebase-apply`); for the others, the current HEAD.
  - The undo is `.reset(op.undoLabel, offerRevert: op != merge && op != rebase, from: start)`.
- **Edge cases:**
  - The original label (for example "Reorder Commits") is lost; the entry says "Rebase".
  - Abort records nothing.
- **Source:** Undo.swift:192-211,221-223; Model.swift:364-374

### UN-07 Undo of stash apply, pop and drop
- **Trigger:** ⌘Z or the toast after HI-19.
- **Preconditions:** A stash with tracked and untracked files.
- **Reaction:**
  - Apply and Pop: before the action, the files from `git stash show -z --name-only --no-renames --include-untracked <sha>` are snapshotted (`hash-object --no-filters --stdin-paths -w`, plus a `write-tree` of the index).
  - Undo restores those files and the index (`read-tree -m <tree>`).
  - For Pop and Drop, undo also runs `git stash store -m <message> <sha>`.
- **Edge cases:**
  - If a file or the index changed since the action, the undo is refused with "<file | N files | The staged changes> changed since <label>, so undoing it would overwrite that. Nothing was changed."
  - That refusal drops the entry for good.
  - For Pop, the stash is still stored and the message adds "The stash is back in the list."
- **Source:** Undo.swift:119-136,227-315

### UN-08 Stack limits and pruning
- **Trigger:** Automatic.
- **Preconditions:** More than 50 actions, or a commit made in a terminal.
- **Reaction:**
  - At most 50 entries per worktree path; the oldest are dropped.
  - Each status refresh prunes entries whose head key differs from the current branch and OID. The prune is skipped while an action is in flight or when a stamp shows a newer action.
  - The stack and the rewrite record are cleared when the worktree is removed, including outside Loupe, and when the repo is removed.
  - The stack lives only in memory (lost on quit). The drop dialog mentions the reflog as the fallback after that.
- **Edge cases:** Undo applies to the sidebar-selected worktree only.
- **Source:** Undo.swift:56-73,163-174; Model.swift:129-133,169-175,298-304; InteractiveRebase.swift:469

# 7. Merge editor and conflicts

## Operations, conflicts, merge editor

### MG-01 Operation banner
- **Trigger:** `operation != nil` (detected from git-dir files: `rebase-merge` or `rebase-apply`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `MERGE_HEAD`), or conflicted files exist.
- **Preconditions:** A stopped merge, cherry-pick, revert or rebase, with or without conflicts.
- **Render:**
  - **Icon:** `exclamationmark.triangle.fill` (orange) with conflicts, `pencil.circle.fill` (blue) at an edit stop, otherwise `checkmark.circle.fill` (green).
  - **Title:** "<Verb> <short sha> <subject>", tail-truncated, with a help tooltip. The verbs are Merging, Cherry-picking, Reverting and Rebasing. The title is "Conflicts" when there is no operation.
  - **Subtitle:** "N file(s) with conflicts. Resolve each one, then continue." / the edit-stop text / "All conflicts are resolved."
  - **Background:** orange at 8% with conflicts, green at 8% without.
  - **Buttons:** only when there is an operation (MG-02, MG-03). Small controls.
  - The banner replaces the PullCreateBanner.
- **Edge cases:** The subject comes from `git log -1 --format="%h %s" <sha>` read from the head file. For a rebase it uses `rebase-merge/stopped-sha`, if present.
- **Source:** Conflicts.swift:89-123, ChangesView.swift:234-239, Model.swift:338-362

### MG-02 Abort the operation
- **Trigger:** "Abort Merge" / "Abort Cherry-Pick" / "Abort Revert" / "Abort Rebase" (help "Go back to how things were before the <op>").
- **Reaction:** `git -c core.editor=true <op> --abort` with `GIT_EDITOR=true`, through `perform("Aborting")`. No undo. History reloads.
- **Source:** Conflicts.swift:111-112, Model.swift:365-374

### MG-03 Continue the operation
- **Trigger:** "Continue" (prominent). It is disabled while conflicts remain; the help then reads "Resolve the conflicts first", otherwise "Finish the <op>".
- **Preconditions:** All conflicts resolved and staged.
- **Reaction:**
  - Loupe first reads where HEAD was before the operation: `orig-head` for a rebase, HEAD for the others.
  - It runs `git -c core.editor=true <op> --continue` with `GIT_EDITOR=true`, so the prepared message is used.
  - The undo is `.reset("Merge" | "Cherry-pick" | "Revert" | "Rebase", offerRevert: op is cherry-pick or revert, from: preHead)`, recorded only when the operation finished.
  - History reloads.
- **Edge cases:** When the rebase stops again, nothing is recorded. When there are unstaged changes at an edit stop, git refuses and the alert shows it.
- **Source:** Conflicts.swift:113-116, Model.swift:365-374, Undo.swift:202-211

### MG-04 Conflicts section and opening the merge editor
- **Trigger:** A file appears under "Conflicts N" (status `u` record) and the user selects it.
- **Preconditions:** A stopped merge with a conflicted text file.
- **Render:** In Compact mode the right pane becomes ConflictPane, keyed by the path. It shows a spinner until the session has loaded.
- **Reaction:**
  - `MergeSession.load` runs `git ls-files -u -z -- <path>`, then `git show :<n>:<path>` for each stage present (skipped for submodules).
  - It checks for a symlink, binary data (a NUL in the first 8000 bytes) or non-UTF-8 text, and reads the merge tool config.
  - It runs `Merge3(base, ours, theirs)` off the main thread.
  - It parses the markers on disk and decides whether Loupe's chunks agree with git's file outside the conflicts.
  - It builds the result and the regions, syntax-highlights the panes, and focuses the first conflict.
- **Source:** ChangesView.swift:267, ChangesView.swift:333-339, MergeEditor.swift:93-180, MergeEditor.swift:681-699, Merge3.swift:7-109

### MG-05 Merge editor layout and labels
- **Trigger:** A text conflict is loaded.
- **Render:**
  - **Header:** an orange triangle, the path (head-truncated, selectable), "added on both sides" when it applies, navigation (MG-07), and on the right "Use <Ours>", "Use <Theirs>", "Reset…", "Merge Tool", "Mark Resolved". Small controls on a bar background.
  - **Top row:** the Ours pane (blue dot), an optional Base pane (gray dot, "the common ancestor"), and the Theirs pane (purple dot). Each pane header shows the title and a middle-truncated detail with a tooltip.
  - **Below:** the chunk bar (MG-09), then the editable Result pane.
  - **Labels per operation:**

| Operation | Ours | Ours detail | Theirs | Theirs detail |
|---|---|---|---|---|
| merge | "Your Branch" | branch | "Incoming" | subject |
| rebase | "Upstream" | "what you're rebasing onto" | "Your Commit" | subject |
| cherry-pick | "Your Branch" | branch | "Cherry-picked" | subject |
| revert | "Your Branch" | branch | "Reverted" | "the code before " + subject |
| none | "Ours" | branch or "HEAD" | "Theirs" | "" |

  - **Pane tints:**
    - A change only on ours: blue at 13%. Only on theirs: purple at 13%. The same change on both: teal.
    - A conflict: orange at 16%; the current conflict: orange at 30% in every pane.
    - Base: gray for changes, orange for conflicts.
    - Picked lines: stronger blue or purple (32%).
  - **Result tints:** ours, theirs and both use the colors above. A conflict is orange (darker when current), or green once resolved (darker when current). A conflict resolved to nothing is drawn as a 2 pt bar.
  - The panes use the editor font and theme. All three support the find bar (⌘F). Ours and Theirs are read-only.
- **Source:** MergeEditor.swift:7-21, MergeEditor.swift:399-427, MergeEditor.swift:542-555, MergeEditor.swift:586-676, MergeEditor.swift:701-815

### MG-06 Base pane toggle
- **Trigger:** The "Base" toggle button (mini) in the Ours pane header (help "Show the common ancestor between the two sides").
- **Reaction:** Shows or hides the Base pane between Ours and Theirs. The setting is per session and not persisted.
- **Source:** MergeEditor.swift:55, MergeEditor.swift:781, MergeEditor.swift:805-808

### MG-07 Conflict navigation
- **Trigger:** The header `chevron.up` (help "Previous conflict", ⌥⌘↑) and `chevron.down` (help "Next conflict", ⌥⌘↓). Moving the caret in the Result pane also changes the current conflict.
- **Preconditions:** A file with three or more conflicts.
- **Render:** "Conflict X of N" in monospaced digits, then "· K left" (secondary) or a green "all resolved" label with `checkmark.circle.fill`.
- **Reaction:** `go(±1)` wraps around. `focus(k)` scrolls the Result pane so the region sits a quarter of the way down, puts the caret at its start, scrolls the Ours, Theirs and Base panes to the chunk, and redraws.
- **Edge cases:** The controls only exist when the file is text and has conflicts. X is clamped to N. The shortcuts are customizable (Shortcuts › Changes).
- **Source:** MergeEditor.swift:725-737, MergeEditor.swift:351-395, Shortcuts.swift:107-108

### MG-08 Next Unresolved
- **Trigger:** The "Next Unresolved" button in the chunk bar. It is shown only while some conflict is unresolved.
- **Reaction:** Focuses the next conflict, wrapping, that still contains markers.
- **Source:** MergeEditor.swift:845-847, MergeEditor.swift:357-360

### MG-09 Chunk bar: take a version of the current conflict
- **Trigger:** In the chunk bar, after "Result · Conflict N":
  - "Use <Ours>" (⌥⌘←)
  - "Use <Theirs>" (⌥⌘→)
  - "<Ours>, Then <Theirs>"
  - "<Theirs>, Then <Ours>"
  - "Use Base" (help "The lines as they were before either side changed them"; disabled when there is no base)
  - "Restore" (help "Put the conflict markers back")
- **Preconditions:** A conflict with a base.
- **Reaction:** The region's text is replaced through the text view, so the change is undoable with ⌘Z. Picks are cleared. The file is written to disk at once. The resolved flags and `markersLeft` are recomputed and the tint turns green.
- **Edge cases:**
  - Restore writes "<<<<<<< <git's label>", ours, "||||||| base", base, "=======", theirs, ">>>>>>> <label>".
  - For conflicts built from Loupe's own chunks the base is always present, possibly empty, so "Use Base" is always enabled and can resolve to nothing.
  - When there are no conflicts the bar shows "· merged without conflicts; edit it freely".
  - The hint "Click lines above to pick them" is shown in tertiary.
- **Source:** MergeEditor.swift:817-855, MergeEditor.swift:287-324, MergeEditor.swift:493-497, Shortcuts.swift:109-110

### MG-10 Click lines to pick them
- **Trigger:** A single click, without dragging or selecting, on a line inside a conflict in the Ours or Theirs pane.
- **Reaction:**
  - The click toggles the pick for that side and line, and the conflict becomes current.
  - The result for that conflict is the picked lines: the side picked from first comes first, and each side's lines keep their order.
  - Picked lines are tinted.
  - Unpicking every line puts the markers back.
- **Edge cases:**
  - Clicks outside conflicts do nothing.
  - When the file was edited outside Loupe, conflicts that cannot be matched to a chunk cannot be picked.
  - Clicks in the Base pane do nothing.
- **Source:** MergeEditor.swift:327-347, MergeEditor.swift:616-622, MergeEditor.swift:658

### MG-11 Editing the result directly (autosave)
- **Trigger:** Typing in the Result pane.
- **Reaction:**
  - Regions follow the edit: they move, grow or shrink, and an empty resolved region takes in text typed at its position.
  - A write is scheduled after 350 ms (debounced). It is atomic and keeps the file's permissions.
  - If neither side ended in a newline, the trailing newline is removed before writing.
  - The resolved state is recomputed.
- **Edge cases:** A write error shows the "Git" alert. Autocorrect, smart quotes and dashes, spellcheck and link detection are all off.
- **Source:** MergeEditor.swift:221-269, MergeEditor.swift:644-645

### MG-12 Undo inside the result
- **Trigger:** ⌘Z or ⇧⌘Z with focus in the Result pane.
- **Reaction:** The NSTextView undo (`allowsUndo`) reverses typing and take actions, because they are registered through `shouldChangeText`. Edit › Undo sends `undo:` to the text view when the first responder is an NSText.
- **Edge cases:** A reload caused by an outside change (MG-18) loses the undo history.
- **Source:** MergeEditor.swift:315-318, MergeEditor.swift:640, Undo.swift:362, Undo.swift:369

### MG-13 Mark Resolved
- **Trigger:** "Mark Resolved" (prominent). It is disabled while `markersLeft`; the help then reads "Resolve every conflict first: the result still has conflict markers", otherwise "Stage the result (git add)".
- **Preconditions:** Every conflict resolved.
- **Reaction:** A pending write is flushed, then `git add -- <path>` runs through `perform("Resolving")`. The file leaves Conflicts, and the same path's staged or unstaged entry gets selected. No undo.
- **Edge cases:** Any line of seven `<` or `>` followed by whitespace or end of line counts as a marker, even in a file where that is legitimate content.
- **Source:** MergeEditor.swift:765-768, MergeEditor.swift:500-504, Model.swift:377-386

### MG-14 Use Ours or Theirs for the whole file
- **Trigger:** The header buttons "Use <Ours>" / "Use <Theirs>". Help: "Take the whole file from <ours> (git checkout --ours) and mark it resolved".
- **Reaction:** Any pending write is discarded. `git checkout --ours|--theirs -- <path>`, then `git add -- <path>`. If the checkout fails because that side deleted the file, Loupe runs `git rm -q -- <path>` instead. No undo.
- **Edge cases:** These header buttons are shown for special (non-text) conflicts too.
- **Source:** MergeEditor.swift:740-743, Model.swift:377-386

### MG-15 Reset… (bring the conflicts back)
- **Trigger:** "Reset…" (help "Throw away the edits to this file and bring its conflicts back (git checkout -m)").
- **Render:** A confirmation dialog "Bring back the conflicts in <filename>?" with "Reset File" (destructive) and the message "Your edits to the result are lost."
- **Reaction:** The pending write is discarded. `perform("Resetting")` runs `git checkout -m -- <path>`, then `session.load()` and `focus(0)`.
- **Edge cases:** When git refuses (for example on a delete/modify conflict), the alert shows it.
- **Source:** MergeEditor.swift:744-751

### MG-16 Merge Tool
- **Trigger:** "Merge Tool" (text conflicts only). It is disabled when no tool is configured or `busy`. Help: "Open the file in <tool> (git mergetool)", or "Set merge.tool (or merge.guitool) in your git config to a GUI merge tool to use it here".
- **Preconditions:** `git config merge.guitool` is set (used with `--gui`), or `merge.tool` is set to something other than vimdiff*, nvimdiff* or emerge.
- **Reaction:** A pending write is flushed. `perform("Merging in <tool>")` runs `git mergetool --no-prompt [--gui] -- <path>` and waits for the tool to exit. Then `reloadIfChangedOutside()` runs.
- **Source:** MergeEditor.swift:752-764, MergeEditor.swift:206-215

### MG-17 AI Suggest
- **Trigger:** "Suggest" (`sparkles`) in the chunk bar. It is shown only when an AI tool is set up. Help: "Ask <Tool> for a merge of this conflict; you review it before it goes in".
- **Preconditions:** A current conflict and a mock AI CLI.
- **Render:**
  - A spinner while the request runs.
  - A popover "Suggested merge" with the text in the editor font (or "(nothing)"), 520×260 and scrollable.
  - The caption "Check it before you use it: it replaces this conflict in the result, and ⌘Z takes it back."
  - Buttons "Discard" and "Use Suggestion" (prominent, Return).
- **Reaction:**
  - The prompt contains the path, the 20 lines before and after the conflict, the base ("(not available)" when missing), ours and theirs.
  - It goes through the same tool, model and run as CM-10.
  - The reply is cleaned: ANSI codes and fences removed, blank lines trimmed at both ends, indentation kept.
  - "Use Suggestion" replaces the conflict the suggestion was asked for, even if the current conflict has changed since.
  - "Discard" or closing the popover clears the proposal.
- **Edge cases:** A failure shows the alert ("No AI tool is set up. Pick one in Settings › General.", a timeout, or exit status). An empty reply resolves the conflict to nothing.
- **Source:** MergeEditor.swift:438-456, MergeEditor.swift:831-840, MergeEditor.swift:858-877, MergeEditor.swift:965-1004

### MG-18 File changed outside Loupe
- **Trigger:**
  - On load, the file on disk does not match Loupe's merge outside the conflicts (edited by hand, a different number of conflicts, or merged with other options).
  - Later, any status refresh while the pane is open.
- **Render:** A footer under the Result pane: "This file was changed outside Loupe, so only its remaining conflict markers are tracked."
- **Reaction:**
  - In this mode the result is the file on disk, and regions are only its marker blocks, matched to chunks where possible.
  - On every `statusVersion` change: if the file or its index stages differ from what Loupe last knew, the session reloads.
  - If the stages are now empty (resolved elsewhere), nothing happens and the pane is about to go.
- **Source:** MergeEditor.swift:129-146, MergeEditor.swift:196-204, MergeEditor.swift:696, MergeEditor.swift:790-794

### MG-19 Added on both sides
- **Trigger:** A conflict with stages 2 and 3 but no stage 1 (both sides added the file).
- **Render:** The header shows "added on both sides". The base is empty, so the whole file is one conflict, trimmed of the lines both sides added alike at the start and end.
- **Source:** MergeEditor.swift:724, Merge3.swift:46-60, Merge3.swift:128-142

### MG-20 Delete/modify conflict
- **Trigger:** Stages (1, 3) mean deleted by us; stages (1, 2) mean deleted by them.
- **Render:**
  - A `trash` icon.
  - Title "Deleted on <Ours>, changed on <Theirs>" or "Changed on <Ours>, deleted on <Theirs>".
  - Detail "Keep the changed file, or delete it as the other side did."
  - A read-only pane showing the surviving side.
  - The header still shows Use Ours, Use Theirs and Reset….
- **Reaction:**
  - "Keep File" (prominent) calls `resolve` with `.current` for deleted-by-them and `.incoming` for deleted-by-us: `git checkout --ours|--theirs`, then `git add`.
  - "Delete File" (destructive) runs `git rm -q -- <path>`.
- **Source:** MergeEditor.swift:110-114, MergeEditor.swift:880-960

### MG-21 Only on one side
- **Trigger:** Stages (2) only mean added by us; stage (3) only means added by them.
- **Render:** A `doc.questionmark` icon. Title "Only on <Ours>" / "Only on <Theirs>". Detail "The other side doesn't have this path, usually because it renamed or deleted the file. Keep it or delete it."
- **Reaction:** "Keep File" uses `.current` for added-by-us and `.incoming` for added-by-them. "Delete File" runs `git rm -q`.
- **Source:** MergeEditor.swift:917-918, MergeEditor.swift:927, MergeEditor.swift:949-954

### MG-22 Binary conflict
- **Trigger:** A stage contains a NUL in the first 8000 bytes, or is not valid UTF-8.
- **Render:** A `doc.richtext` icon. Title "Binary file conflict". Detail "Binary files can't be merged line by line. Keep one side: <Ours> <size>, <Theirs> <size>." (the size is "none" when a stage is missing). No code pane.
- **Reaction:** "Use <Ours>" / "Use <Theirs>" call `resolve(.current | .incoming)`.
- **Source:** MergeEditor.swift:103, MergeEditor.swift:919-935, MergeEditor.swift:955-957

### MG-23 Symbolic link conflict
- **Trigger:** A stage with mode 120000.
- **Render:** Title "Symbolic link conflict". Detail "The two sides point the link at different targets. Keep one side." It is treated like a binary conflict.
- **Reaction:** Same as MG-22.
- **Source:** MergeEditor.swift:87, MergeEditor.swift:103, MergeEditor.swift:919, MergeEditor.swift:929

### MG-24 Submodule conflict
- **Trigger:** Any stage with mode 160000.
- **Render:** A `shippingbox` icon. Title "Submodule conflict". Detail "Both sides moved the submodule to a different commit. Pick the commit to keep."
- **Reaction:** Buttons "Use <Ours> · <sha8>" and "Use <Theirs> · <sha8>" (help "Record the submodule at <sha>") run `git update-index --cacheinfo 160000,<sha>,<path>` through `perform("Resolving")`.
- **Source:** MergeEditor.swift:905-906, MergeEditor.swift:914, MergeEditor.swift:925, MergeEditor.swift:940-948

### MG-25 CRLF files and no final newline
- **Trigger:** A conflicted file with CRLF line endings, or with no trailing newline on either side.
- **Reaction:**
  - Lines are split on "\n" with the "\r" kept on each line, so CRLF is preserved.
  - "=======\r" is recognized as a separator.
  - When neither side ends in a newline, the written result has none either.
  - When the file was edited outside Loupe, its own ending is kept.
- **Source:** Merge3.swift:16-23, Merge3.swift:77-81, Conflicts.swift:41, MergeEditor.swift:148, MergeEditor.swift:256

### MG-26 Leaving the merge editor
- **Trigger:** Selecting another file, switching tab, or switching worktree.
- **Reaction:** `onDisappear` flushes any pending write. Text storages drop their layout managers.
- **Source:** MergeEditor.swift:697, MergeEditor.swift:271, MergeEditor.swift:673-675

---

# 8. Pull Requests (list, detail, files, viewed, Full mode, conversation, review, merge, forge API)

Not implemented anywhere in the Swift code, so there are no flows for them: replying to a review comment, resolving a comment, editing or deleting a server comment, suggestions, closing or reopening a PR, editing the title or description, toggling draft, a delete-branch-after-merge option, and syncing the viewed state to the forge (viewed is local only). The PR list has no CI status dots and no review-state badges. CI is shown only in the detail header (see CI-).


Scope: I read PullRequestsView.swift, PullModel.swift, Conversation.swift and Forgejo.swift in full. I also read the PR wiring in App.swift, Shortcuts.swift, Model.swift and ChangesView.swift (the WorktreeView host and toolbar), plus the parts of DiffTableView.swift, CodeBrowser.swift and FileTree.swift that the PR views call. Vault notes: Projects/loupe/index.md and pr-full-mode-analysis.md. Where the vault and the code disagree, the code wins and the stale vault item is listed under BROKEN/DEAD.

Not implemented anywhere in the Swift code, so there are no flows for them: replying to a review comment, resolving a comment, editing or deleting a server comment, suggestions, closing or reopening a PR, editing the title or description, toggling draft, a delete-branch-after-merge option, and syncing the viewed state to the forge (viewed is local only). The PR list has no CI status dots and no review-state badges. CI is shown only in the detail header (see CI-).

## Pull Requests list

### PR-01 Open the Pull Requests tab
- **Trigger:** toolbar segmented control "Pull Requests" (principal position); View menu > "Pull Requests" (⌘3, can be changed in Settings > Shortcuts > Tabs); the PR create sheet also switches here after creating a PR (see PC-).
- **Preconditions:** a worktree is selected.
- **Render:** WorktreeView switches on `@AppStorage("tab") == "pulls"` to PullRequestsView. The tab survives relaunch because it is a UserDefaults key.
- **Reaction:** sets UserDefaults `tab = "pulls"`. PullRequestsView picks a state: PR-02, PR-03, PR-04, or the browser (PR-05).
- **Edge cases:** the WorktreeView "Git" alert still shows errors from Check Out (PD-33) while on this tab.
- **Source:** App.swift:90, ChangesView.swift:6, ChangesView.swift:16-20, ChangesView.swift:63-71, Shortcuts.swift:97

### PR-02 No Remote state
- **Trigger:** open the tab on a repo whose remotes don't parse as host/owner/name.
- **Preconditions:** `git remote -v` has no parseable fetch URL (or no remotes), remote detection has finished, and tokens are loaded.
- **Render:** ContentUnavailableView "No Remote" (icon network.slash), with the text "This repository has no remote that looks like a Forgejo or GitHub repo."
- **Reaction:** none.
- **Edge cases:** URL parsing accepts `https://host/o/r(.git)`, `ssh://git@host:2222/o/r.git` and `git@host:o/r.git`. An http(s) port is kept in the host ("localhost:3300"); an ssh port is dropped. A path with fewer than 2 components is rejected.
- **Source:** PullRequestsView.swift:22-23, Forgejo.swift:135-149, Model.swift:523-535

### PR-03 Connect account state
- **Trigger:** open the tab on a repo with a remote but no matching account or token.
- **Preconditions:** a remote is detected, and no account in this repo's space matches the remote host (port ignored), or an account exists but has no Keychain token.
- **Render:** ContentUnavailableView titled "Connect <host>" (icon person.crop.circle.badge.plus). Description: "Add a GitHub account for <host> to review pull requests for <owner>/<name>." when host == github.com, otherwise "Add a Forgejo account …". Below it, the prominent button "Open Settings…" (a SettingsLink).
- **Reaction:** the button opens Settings (see the shell agent). When `app.accounts` changes, `detectRemote()` runs again (`git remote -v`). Remote choice order: a remote that has an account, then "origin", then the first one.
- **Edge cases:** an account without a token gives the same "Connect" screen, which is misleading. Account matching is per space: an account in another space doesn't count.
- **Source:** PullRequestsView.swift:13-21, Model.swift:196-201, Model.swift:537-542, Model.swift:523-535

### PR-04 Loading before remote/tokens are known
- **Trigger:** open the tab right after launch or right after selecting a new worktree.
- **Preconditions:** `remoteDetected == false` or `app.tokensLoaded == false` (tokens are read off the main thread once at launch).
- **Render:** a full-size ProgressView.
- **Reaction:** switches to PR-02, PR-03 or PR-05 once both are known.
- **Source:** PullRequestsView.swift:11-12, Model.swift:52, Model.swift:67, Model.swift:285

### PR-05 First load and the disk cache
- **Trigger:** PullBrowser appears, or the state picker changes (`.task(id: list.state)`).
- **Preconditions:** account and token present. Fixture: ≥1 open PR; a previous session's cache file for an instant list.
- **Render:** if state is open and the list is empty, the cached list from `~/Library/Caches/dev.extypen.loupe/pulls-<host>-<owner>-<name>.json` (":" becomes "_") shows at once. Otherwise a centred spinner shows until the first response.
- **Reaction:** loads when `loadedState != state` or the data is older than 30 s. Loading is `GET {api}/repos/{o}/{r}/pulls?state=<open|closed|all>&page=1&limit=50` (GitHub uses `per_page=50`). For state open it keeps fetching page 2, 3, … while the last page was full. Closed and all keep only page 1. The cache is written (open only, and only when the list changed) by a detached task; writes run one after another.
- **Edge cases:** returning to the tab within 30 s does no fetch. The response is discarded if the state changed mid-flight. If the server's maximum page size is under 50, the open list stops after page 1 (truncated) because `all.count == p*50` fails.
- **Source:** PullRequestsView.swift:114-119, PullModel.swift:29-57, PullModel.swift:72-91, Forgejo.swift:258-262

### PR-06 Section grouping
- **Trigger:** the list renders.
- **Preconditions:** fixture with a PR whose head.ref equals the current worktree's branch, a PR whose head.ref is checked out in another worktree of the same repo, and other PRs.
- **Render:** Section "This Branch" (head.ref == current branch), then "Other Worktrees" (head.ref is a branch of another worktree of the same repo), then "All" for the rest. The "All" header is empty when the other two sections are empty. Order inside each section is the server's (newest first). There is no client-side sort.
- **Reaction:** none.
- **Edge cases:** matching is by branch name only, so a fork PR with the same branch name lands in This Branch. Sections are built from the filtered list (PR-10, PR-11).
- **Source:** PullRequestsView.swift:38-40, PullRequestsView.swift:84-102

### PR-07 PR row rendering
- **Trigger:** the list renders.
- **Preconditions:** PRs that are open, closed-unmerged, merged and draft, and one with comments > 0.
- **Render:** 28 pt avatar (PR-08). State icon: merged = purple arrow.triangle.merge, closed = red xmark.circle, open = green arrow.triangle.pull. Title (max 2 lines, medium weight). Caption row: "#N", author login, a "Draft" capsule when draft == true, head ref (accent colour when local, otherwise secondary; tooltip "head → base"). Local PRs also get a branch icon with the tooltip "Checked out in a local worktree". Then "→ base" in tertiary, and a right-aligned bubble with the comment count when comments > 0.
- **Reaction:** none.
- **Edge cases:** GitHub lists don't send `merged`; `merged_at != nil` sets `merged = true`. Rows in This Branch and Other Worktrees are styled local.
- **Source:** PullRequestsView.swift:138-169, Forgejo.swift:266-270

### PR-08 Avatars and the initials fallback
- **Trigger:** any avatar appears (list, header, badges, conversation).
- **Preconditions:** users with `avatar_url`, one with no URL, and one with a URL that returns 404.
- **Render:** a circular image with a 0.5 pt separator stroke and the login as tooltip. While loading, or when loading fails: a coloured circle with the first letter uppercased. The hue is a hash of the login (saturation 0.45, brightness 0.75).
- **Reaction:** an unauthenticated `GET <avatar_url>` through URLSession.shared, one request per URL in flight. The image is decoded to a ≤128 px thumbnail and kept in an NSCache (500 entries).
- **Edge cases:** a reused avatar view (the detail header) whose user changes reloads. A failed load stays on initials for that view.
- **Source:** PullRequestsView.swift:173-200, PullRequestsView.swift:240-271, App.swift:113

### PR-09 State picker Open / Closed / All
- **Trigger:** the segmented picker "Open | Closed | All" at the top left (label hidden).
- **Preconditions:** both open and closed PRs.
- **Render:** the previous state's PRs stay on screen until the new response arrives, because the list isn't cleared.
- **Reaction:** restarts the `.task`, which loads page 1 of the new state (PR-05). Only Open keeps polling (PR-13).
- **Edge cases:** if the new state's load fails, the old state's PRs stay visible under the new picker value with no error shown (the error overlay only shows when the list is empty). The picker resets to Open on relaunch; `state` is not persisted.
- **Source:** PullRequestsView.swift:58-61, PullRequestsView.swift:114-125, PullModel.swift:7, PullModel.swift:50

### PR-10 "Opened by me" filter
- **Trigger:** the toggle button "Opened by me" (person icon).
- **Preconditions:** account username set; PRs by me and by others.
- **Render:** a pressed or unpressed button. Tooltip: "Only pull requests opened by <me>", or "Add a Forgejo account to filter by author" when disabled.
- **Reaction:** a client-side filter: `pr.user.login` equals the account username, case-insensitive. Saved in UserDefaults `pullsMine`. No API call.
- **Edge cases:** disabled when there is no username. The empty overlay reads "No Pull Requests by You".
- **Source:** PullRequestsView.swift:42-47, PullRequestsView.swift:62-65, PullModel.swift:8

### PR-11 Filter text field
- **Trigger:** TextField "Filter pull requests" (rounded, max 320 pt).
- **Preconditions:** several PRs.
- **Render:** the list narrows as you type.
- **Reaction:** client-side match on title (case- and locale-insensitive), `"#<number>".contains(text)`, or author login. Not persisted (view @State).
- **Edge cases:** "#12" also matches #123; "1" matches every number with a 1 in it. The text is not trimmed. No matches shows "No Pull Requests" (not "no matches"). Pages that weren't loaded (closed/all) are not searched.
- **Source:** PullRequestsView.swift:34, PullRequestsView.swift:42-47, PullRequestsView.swift:66

### PR-12 Infinite scroll (closed / all)
- **Trigger:** scroll until the row of the last loaded PR appears.
- **Preconditions:** state Closed or All with more than 50 PRs (vault: fd/fleetdriver has 7,239 closed PRs).
- **Render:** a small spinner at the bottom of the "All" section while loading.
- **Reaction:** `GET …/pulls?state=<s>&page=<page+1>&limit|per_page=50`. PR ids already shown are skipped. Paging stops after a short page (`exhausted`).
- **Edge cases:** skipped while a main load runs or loading is already in progress. Only rows in the "All" section have the `onAppear` trigger (see BROKEN). An error sets `list.error`, which is hidden because the list isn't empty.
- **Source:** PullRequestsView.swift:96-100, PullModel.swift:59-68

### PR-13 Polling the open list
- **Trigger:** automatic while PullBrowser shows state Open and no PR is selected.
- **Preconditions:** change server data between polls.
- **Render:** no re-render when nothing changed.
- **Reaction:** every 60 s, `load()` (the same calls as PR-05). The `pulls` array is replaced and the cache rewritten only when the list differs (`all != pulls`).
- **Edge cases:** closed and all are never auto-refreshed (it would drop scrolled pages). While a detail is open, `load()` returns immediately (`selected != nil`). A call while one for the same state is in flight runs once more afterwards instead of running alongside it. The task prompt said 30 s: that is only the "fresh enough" threshold on returning to the tab; the poll interval is 60 s. The vault agrees: "auto-polled (60 s, on app activate, on returning to the list)".
- **Source:** PullRequestsView.swift:120-124, PullModel.swift:29-40, PullModel.swift:51-55

### PR-14 Refresh button and ⌘R
- **Trigger:** the borderless arrow.clockwise button (tooltip "Refresh (⌘R)"); Review menu > "Refresh" ⌘R (posts `.refresh`).
- **Preconditions:** none.
- **Render:** the spinner replaces "Updated …" while loading.
- **Reaction:** `list.load` (PR-05). ⌘R also runs `model.refreshStatus()` on the worktree (git status and so on).
- **Edge cases:** the button itself has no key equivalent; ⌘R comes from the menu. With a detail open, ⌘R reloads the detail instead (PD-07).
- **Source:** PullRequestsView.swift:78-79, PullRequestsView.swift:134, App.swift:100, ChangesView.swift:100

### PR-15 Refresh on app activation
- **Trigger:** switch back to Loupe from another app.
- **Preconditions:** the PR list is visible.
- **Reaction:** `list.load` (skipped when a detail is open).
- **Source:** PullRequestsView.swift:126

### PR-16 "Updated …" label and list spinner
- **Trigger:** passive.
- **Render:** while loading, a small spinner. Otherwise "Updated <relative, numeric>" (e.g. "Updated 2 min. ago") in tertiary caption, redrawn every 5 s. Then the refresh button and "<owner>/<name>" in secondary.
- **Edge cases:** after a failed refresh the time keeps aging; there is no stale or error marker when PRs are shown.
- **Source:** PullRequestsView.swift:72-80

### PR-17 Empty states
- **Trigger:** the list is empty after filtering.
- **Preconditions:** a repo with 0 open PRs, or a filter that matches nothing.
- **Render:** loading with nothing cached shows a centred spinner. Otherwise ContentUnavailableView "No Pull Requests", or "No Pull Requests by You" when Opened by me is on (icon arrow.triangle.pull).
- **Source:** PullRequestsView.swift:104-108

### PR-18 List load error
- **Trigger:** a load fails while the list is empty.
- **Preconditions:** forge down, 401, 404 repo, or offline, with no cache.
- **Render:** ContentUnavailableView "Couldn't Load" (exclamationmark.triangle) with the message, e.g. "Forgejo 401: token is required", "GitHub 403: API rate limit exceeded…", or a URLError text.
- **Reaction:** the next poll, activation or refresh retries and clears the error on success.
- **Edge cases:** when PRs are already shown, the error is invisible. A cancelled request keeps the previous error.
- **Source:** PullRequestsView.swift:106, PullModel.swift:56, Forgejo.swift:229-237, Git.swift:5-8

### PR-19 "Create Pull Request…" entry point
- **Trigger:** the button "Create Pull Request…" (plus icon) in the list header.
- **Preconditions:** the current branch has commits that the integration target doesn't have (`git rev-list --count <target>..HEAD` > 0), and no open PR has head.ref == branch.
- **Render:** the button appears with the tooltip "Create a pull request for <branch>".
- **Reaction:** sets `app.creatingPull = worktree path`, which opens the PullCreateSheet (see PC-). Availability is computed once per "headOID refsVersion" key and cached on the model.
- **Source:** PullRequestsView.swift:67-70, PullRequestsView.swift:127-132, PullCreate.swift:431-437, App.swift:176-178

### PR-20 Open a PR (select a row)
- **Trigger:** click a row (List selection by PR number).
- **Preconditions:** ≥1 PR.
- **Render:** the detail replaces the list completely (PD-01).
- **Reaction:** `list.selected = number`. `detail(for:)` reuses a cached PullRequestModel or creates one. Only the 2 most recently opened PRs plus the selected one are kept.
- **Edge cases:** the detail can still show when the PR has left `list.pulls` (state switch or poll), because it falls back to `details[n].pr`. The detail view gets `.id(n)` so its view state (the tab) resets per PR.
- **Source:** PullRequestsView.swift:52-54, PullRequestsView.swift:84, PullModel.swift:17-22, PullModel.swift:93-108

### PR-21 Back to the list
- **Trigger:** header button "‹ Pull Requests" (⌘[, action "Back to List", customisable).
- **Render:** the list returns.
- **Reaction:** `selected = nil` trims the cached details and immediately triggers `list.load` (it is not subject to the 30 s rule).
- **Source:** PullRequestsView.swift:133, PullRequestsView.swift:327-328, PullModel.swift:17, Shortcuts.swift:70, Shortcuts.swift:103

### PR-22 Switching worktree drops opened PR models
- **Trigger:** select another worktree in the sidebar, then come back.
- **Reaction:** `releaseHeavyState()` clears `pulls.details`. `selected` and the list stay. When you come back, the detail is rebuilt from `list.pulls` and reloads fully, so viewed state and drafts come back from UserDefaults.
- **Source:** Model.swift:28, Model.swift:638-643

## PR detail

### PD-01 Detail header
- **Trigger:** a PR is opened.
- **Preconditions:** PR with additions/deletions, requested reviewers and reviews.
- **Render:** row 1: "‹ Pull Requests" button, then the title (title3 semibold, 1 line, selectable, tooltip = title), then the "#N ⌄" menu (PD-02/03). On the right: a 16×16 slot holding a spinner only while loading, "Check Out" (PD-33), "Merge" (only when state == open, PD-34), and "Review" or "Review (N)" (prominent, RV-18). Row 2: 18 pt avatar, author login, "head → base" (monospaced, middle-truncated), "+A" in green and "−D" in red (only when both are present), review badges (PD-08), ChecksBadge (PD-09).
- **Source:** PullRequestsView.swift:314-391

### PD-02 Open in Browser
- **Trigger:** click "#N ⌄" > "Open in Browser" (safari icon). Tooltip "Open in Browser or Copy Link".
- **Reaction:** `NSWorkspace.open(pr.html_url)`.
- **Edge cases:** an invalid URL does nothing.
- **Source:** PullRequestsView.swift:331-343

### PD-03 Copy Link
- **Trigger:** "#N ⌄" > "Copy Link".
- **Reaction:** the pasteboard holds `html_url` as a string. No toast.
- **Source:** PullRequestsView.swift:333

### PD-04 View switch Overview / Files / Conversation
- **Trigger:** the toolbar segmented control at the right edge with three icons: square.grid.2x2 "Overview: what the change does", doc.text "Files: the diff", bubble "Conversation".
- **Render:** the default tab is Files, and it resets each time a PR opens. The Compact/Full picker shows in the toolbar only on Files.
- **Reaction:** `tab` is local view state. The controls reach the toolbar through `DiffControlsKey`.
- **Source:** PullRequestsView.swift:280, PullRequestsView.swift:286-290, PullRequestsView.swift:587-599, ChangesView.swift:73-95

### PD-05 Detail load (API sequence)
- **Trigger:** detail opens, poll, app activation, ⌘R, after a review is submitted.
- **Preconditions:** fixture PR with a diff, reviews, issue comments and commits.
- **Render:** cached content stays on screen; the header spinner shows.
- **Reaction:** these four run in parallel: `GET /repos/{o}/{r}/pulls/{n}`; `GET …/pulls/{n}/reviews` (all pages, 50 each); `GET …/issues/{n}/comments`; `GET …/pulls/{n}/commits?verification=false&files=false` (all pages). After the PR arrives: the checks sha is updated and refreshed (see CI-). The diff comes from Forgejo `GET …/pulls/{n}.diff` (Accept text/plain) or GitHub `GET …/pulls/{n}` (Accept application/vnd.github.diff). A diff identical to the last one (SHA-256) is not parsed again. Files are sorted in tree order and fingerprinted. If no file is selected, or the selected one is gone, the first unviewed file is selected (else the first file). Review comments: Forgejo `GET …/pulls/{n}/reviews/{id}/comments` for each review in parallel, sorted by id; GitHub `GET …/pulls/{n}/comments` (all pages).
- **Edge cases:** a commits failure is tolerated (the timeline does without them). Any other failure sets `pull.error` (PD-37), and fields already assigned (the pr) stay updated. Only one load runs at a time; extra calls queue a single rerun.
- **Source:** PullModel.swift:230-271, Forgejo.swift:264, Forgejo.swift:275-279, Forgejo.swift:283-333

### PD-06 Detail polling and refresh
- **Trigger:** automatic; app activation; ⌘R (Review > Refresh).
- **Reaction:** on open, if the PR was loaded less than 30 s ago it waits 60 s first; otherwise it loads at once. Then it loads every 60 s while the detail is mounted. Activation and ⌘R call `pull.load()`. Polling stops when you leave (PR-21) or merge (PD-35).
- **Edge cases:** a poll whose diff is unchanged keeps scroll, selection and highlighting. Offline, every failed poll raises the alert again (BROKEN).
- **Source:** PullRequestsView.swift:292-303, PullModel.swift:236

### PD-07 Review badges in the header
- **Trigger:** passive; clicking a badge opens a popover.
- **Preconditions:** reviews by several users: APPROVED, REQUEST_CHANGES, a dismissed one, a stale one, REQUEST_REVIEW, and requested_reviewers.
- **Render:** up to three badges: "N approved" (green checkmark.circle.fill), "N requested changes" (red exclamationmark.circle.fill), "N pending" (orange circle.fill). The text is secondary and turns primary on hover or while the popover is open; the pointer is a link cursor. The popover has a capitalised title ("Approved", "Requested changes", "Pending") and a 20 pt avatar plus login per user.
- **Reaction:** per user, the latest review by id among non-dismissed, non-stale APPROVED, REQUEST_CHANGES or REQUEST_REVIEW reviews. Users in requested_reviewers become pending when they have no state, or on GitHub when their state is approved (a re-request). COMMENT reviews don't count.
- **Source:** PullRequestsView.swift:204-238, PullRequestsView.swift:394-406, PullModel.swift:472-487

### PD-08 Checks badge (entry only)
- **Trigger:** click the ChecksBadge in header row 2.
- **Reaction:** sets `app.checksOverlay`, which opens the window-wide checks modal (fade, 0.15 s). See CI-.
- **Source:** PullRequestsView.swift:380-383, App.swift:171-173

### PD-09 Overview tab
- **Trigger:** view switch > Overview.
- **Render:** "Fetching the pull request…" spinner, then ChangeOverview (see the Changes agent), or the error view "Can't Read the Pull Request".
- **Reaction:** runs `fetchHead()` (PD-32), then `git rev-parse --verify -q refs/remotes/<remote>/<base>^{commit}` (as target, falling back to base.sha), then `git merge-base <base.sha> <head.sha>`. Opening an item switches to Files and reveals it (PD-23).
- **Source:** PullRequestsView.swift:287, PullRequestsView.swift:602-633

### PD-10 Files tab layout
- **Trigger:** view switch > Files.
- **Render:** a resizable split. The tree pane (width saved as `prTreeWidth`, 250–460 pt) holds the "Filter files" field, the viewed progress (PD-12) and the tree. The file pane is at least 360 pt wide.
- **Source:** PullRequestsView.swift:425-453, FileTree.swift:190

### PD-11 Filter files
- **Trigger:** TextField "Filter files".
- **Reaction:** case-insensitive substring match on the path. In Compact it filters changed files; in Full it filters the whole tree. While a filter is set, every folder shows expanded and collapse clicks have no effect. Next/previous and Mark Viewed & Next work on the filtered list.
- **Edge cases:** the progress bar still counts all files. Marking a folder viewed only affects the filtered files under it.
- **Source:** PullRequestsView.swift:431, PullRequestsView.swift:467-472, PullModel.swift:149, PullModel.swift:178-227

### PD-12 Viewed progress
- **Render:** a ProgressView (viewed / max(1, total)) and "X of Y files viewed" (caption).
- **Source:** PullRequestsView.swift:433-438, PullModel.swift:207-213

### PD-13 Tree rows (Compact and Full)
- **Render:** folders: a chevron (rotated when open), a folder icon, the name, and "done/total" on the right (accent colour when complete, faded secondary otherwise). Changed file: icon, name, "+a −d" (or "binary"), and a status square on the right (A/D/R/M from old/new path). The row is at 50% opacity when viewed, and a bubble count shows when it has comments (RV-15). Full mode, unchanged file: icon and name in secondary, tooltip = path.
- **Reaction:** clicking a folder toggles it (folders can't be selected). Clicking a file selects it. ⌘A in the tree does nothing.
- **Source:** PullRequestsView.swift:463-501, FileTree.swift:76-138, FileTree.swift:159-186, PullRequestsView.swift:635-642

### PD-14 Tree context menu and marking a folder viewed
- **Trigger:** right-click a folder or file in the tree.
- **Render:** on a folder: "Expand" or "Collapse", then "Mark Folder as Viewed" or "Mark Folder as Not Viewed" (the latter when every file under it is viewed), a divider, "Expand All", "Collapse All". On a file: only "Expand All" and "Collapse All".
- **Reaction:** the folder viewed action stores the fingerprints of every file under it, or clears all of them. This is one UserDefaults write. Compact and Full keep separate collapse sets.
- **Edge cases:** a folder with no PR files under it (Full mode) has no progress and its menu says "Mark Folder as Viewed", which does nothing. The vault's "a directory has a viewed checkbox" is stale: it is now a context-menu item plus the counter.
- **Source:** FileTree.swift:111-119, PullRequestsView.swift:473-476, PullModel.swift:178-185

### PD-15 Compact file view
- **Trigger:** select a changed file while Compact and side == Diff.
- **Render:** DiffHeader with the path, "old → new" when renamed, +/− counts, and a "Viewed" checkbox. Body: "Binary File" (doc.zipper); or "No Content Changes" with "Mode change or rename only."; or an LFS pointer view; or the DiffTableView with inline comments (RV-12) and drafts. Nothing selected and loading shows "Loading diff…". Nothing selected otherwise shows "No File Selected".
- **Reaction:** Markdown preview (RV-…/diff agent) loads the whole file with `fetchHead()` and then `git show <head.sha>:<path>`.
- **Source:** PullRequestsView.swift:503-550

### PD-16 Viewed checkbox (per file) and its persistence
- **Trigger:** the "Viewed" checkbox in the file header.
- **Reaction:** stores or removes `viewed[path] = FNV-1a fingerprint of the file's diff lines (text + kind)`. Saved in UserDefaults `viewed:<host>/<owner>/<name>#<n>` as {path: String(UInt64)}; the key is removed when empty. Not synced to Forgejo or GitHub. The tree row dims, and the progress and folder counters update.
- **Edge cases:** a new push that changes the file's diff gives a new fingerprint, so the file shows unviewed again automatically (vault: "Viewed state is keyed by an FNV fingerprint"). If the diff source switches between the server diff and the local fallback, the fingerprint may change.
- **Source:** PullRequestsView.swift:532, PullModel.swift:152-160, PullModel.swift:440-452

### PD-17 Mark Viewed & Next
- **Trigger:** Review menu > "Mark Viewed & Next" (⇧⌘V); the bare key `v` while the diff table has focus (it posts `.markViewed`).
- **Reaction:** marks the selected changed file viewed (if it isn't), then selects the next unviewed file in the filtered list, wrapping around. When none is left, the selection stays.
- **Edge cases:** does nothing when the selection is an unchanged file (Full mode) or nothing is selected. `v` in the Changes tab has no listener.
- **Source:** App.swift:97, DiffTableView.swift:879, PullRequestsView.swift:305, PullModel.swift:454-462, Shortcuts.swift:102

### PD-18 Next / Previous File
- **Trigger:** Review menu > "Next File" (⌥↓), "Previous File" (⌥↑).
- **Reaction:** moves ±1 in the filtered changed-file list, clamped (no wrap). With nothing selected, it goes to the first file. In Full mode it opens the file as a tab. `n`/`p` in the diff jump between hunks (diff agent).
- **Source:** App.swift:95-96, PullRequestsView.swift:304, PullModel.swift:464-468, DiffTableView.swift:877-878

### PD-19 Compact / Full mode toggle
- **Trigger:** the toolbar segmented control (rectangle.compress.vertical "Compact: the changes with a few lines around them" / rectangle.expand.vertical "Full: whole files with the changes inline (⌥⌘F)"), shown on the Files tab; Review menu > "Compact / Full Mode" (⌥⌘F).
- **Reaction:** flips the global UserDefaults `prFullMode`, which the Changes tab shares. PullFilesPane watches UserDefaults changes and syncs `pull.fullMode`. Turning Full on adds the selected file as a tab and loads the tree (PD-20). Back to Compact on an unchanged file selects the first changed file and closes the results panel.
- **Edge cases:** ⌥⌘F works from any tab. Opening an unchanged file from Overview or Conversation turns Full on globally (PD-23). There is a sync gap when the toggle happens outside the Files tab (BROKEN).
- **Source:** App.swift:98, ChangesView.swift:9, ChangesView.swift:73-81, PullRequestsView.swift:454-459, PullModel.swift:299-307, Shortcuts.swift:104

### PD-20 Full mode: the repository tree
- **Trigger:** Full mode on, or the head sha changes while in Full.
- **Render:** while the tree is empty: "Fetching the pull request…" spinner, or "Can't Load the Repository" plus the error.
- **Reaction:** `fetchHead()`, then `git ls-tree -r -z --name-only <head.sha>`. The PR's changed paths are added, so deleted files can still be reached. Folders this PR doesn't touch start collapsed; this applies once, after both the tree and the files are known.
- **Source:** PullRequestsView.swift:439-447, PullModel.swift:316-354

### PD-21 Full mode: whole file and tabs
- **Trigger:** select any tree path in Full mode, or a path in Compact with side Before/After.
- **Render:** the FileTabBar (PD-22) appears when tabs exist. Header: FileNavControls (PD-26), then the Viewed checkbox for a changed file, or "Unchanged" for an unchanged one. Body: a spinner until loaded, then "Binary File", "Empty File", or the DiffTableView with the change ruler.
- **Reaction:** a changed file runs `git diff --no-color --no-ext-diff -M -U1000000 <base.sha>...<head.sha> -- <old/new paths>`; the hunk row is removed so the file reads top to bottom. A binary or deleted file reuses the PR diff. An unchanged file runs `git show <head.sha>:<path>` and becomes context lines; a NUL in the first 8000 bytes means binary. The cache is cleared when the head sha changes. A newly selected path is inserted as a tab right after the current one.
- **Edge cases:** if loading fails, the "Forgejo" alert shows and the spinner stays.
- **Source:** PullRequestsView.swift:509-512, PullRequestsView.swift:515, PullRequestsView.swift:552-582, PullModel.swift:134-141, PullModel.swift:356-402

### PD-22 Tab bar
- **Trigger:** click a tab; the × button (visible on the active or hovered tab, tooltip "Close Tab"); right-click a tab.
- **Render:** file icon and name; the parent folder is added when two tabs share a name. A coloured dot marks a changed file (tooltip "Changed"). The active tab has a text background and a 2 pt accent line on top. The bar scrolls to the selected tab.
- **Reaction:** the context menu has "Close Tab", "Close Other Tabs" (disabled with fewer than 2 tabs), "Close All Tabs", a divider, and "Copy Path". Closing the selected tab selects its neighbour.
- **Source:** CodeBrowser.swift:178-230, CodeBrowser.swift:49-55

### PD-23 Reveal a file or line from Overview / Conversation
- **Trigger:** open an item in Overview, or an inline comment's file header in Conversation (RV-05).
- **Reaction:** switches to Files. If the path isn't changed by the PR and the mode is Compact, Full is turned on (globally). With a line: `open(path, line)`, which records the back history and sets `reveal`. Without one, it only selects the path.
- **Source:** PullRequestsView.swift:287-288, PullModel.swift:421-425, CodeBrowser.swift:29-35

### PD-24 ⌘W closes a tab
- **Trigger:** ⌘W in the Files tab in Full mode.
- **Reaction:** a local key monitor closes the selected tab. With no tab selected it does nothing and never closes the window. In any other tab or in Compact mode, ⌘W goes through to the window and closes it, which quits the app (it terminates after the last window).
- **Source:** PullRequestsView.swift:596-597, PullRequestsView.swift:684-710, App.swift:138

### PD-25 Code navigation (Full mode)
- **Trigger:** ⌘-click an identifier; right-click > "Go to Definition of “w”" / "Find References to “w”".
- **Render:** while it runs, a mini spinner and the word in the file header. Results show in a 220 pt panel under the file: "N definitions of w", or "[No definition found in this repository · ]N references to w" (shown as "2000+" at the cap). The × button or Esc closes it. Each row shows path:line and text; click opens it.
- **Reaction:** `fetchHead()`, then CodeNavigator at head.sha (git grep plus tree-sitter, per the vault). A single definition jumps straight there.
- **Source:** PullModel.swift:427-438, PullRequestsView.swift:521-524, PullRequestsView.swift:573, DiffTableView.swift:616-622, DiffTableView.swift:692-696, CodeBrowser.swift:275-323

### PD-26 Back / Forward and the outline
- **Trigger:** chevron buttons in the header (tooltips "Back" ⌃- / "Forward" ⌃⇧-); the outline menu (list.bullet.indent, "Outline: jump to a definition in this file").
- **Reaction:** walks the jump history. The outline lists "name kind · line" items. A file too large or minified shows a greyed icon with "No outline: this file is too large or minified to parse".
- **Source:** CodeBrowser.swift:233-272, CodeBrowser.swift:37-47, Shortcuts.swift:105-106

### PD-27 Before / After side in a PR
- **Trigger:** set the diff side to Before or After (diff agent's control, `diffSide`) while in Compact.
- **Reaction:** the PR file pane uses the whole-file path (PD-21) even in Compact, with no tab bar unless Full is on.
- **Source:** PullRequestsView.swift:505, PullRequestsView.swift:514-515

### PD-28 Check Out in a new worktree
- **Trigger:** header button "Check Out" (arrow.triangle.branch), tooltip "Check Out in New Worktree".
- **Preconditions:** a PR whose `refs/pull/N/head` exists on the remote.
- **Render:** while it runs, the worktree is busy with "Checking out PR #N".
- **Reaction:** if any known worktree already has branch `pr/<N>`, that worktree is selected. Otherwise: `git fetch <remote> +refs/pull/N/head:refs/remotes/<remote>/pr/N` (with gitAuth), then `git worktree add -B pr/N <parent-of-repo>/<repo>-pr-<N> <remote>/pr/N`. The worktrees are refreshed and the new one is selected if its folder exists.
- **Edge cases:** an error shows in the "Git" alert. An existing non-worktree folder at the destination fails. The existing-worktree check spans all repos (BROKEN). The vault confirms checkout works.
- **Source:** PullRequestsView.swift:348-349, Model.swift:556-569, Model.swift:412-417, ChangesView.swift:97-99

### PD-29 Merge menu and confirmation
- **Trigger:** header menu "Merge" > "Merge" / "Rebase" / "Squash".
- **Preconditions:** state == open.
- **Render:** disabled when `mergeable == false`, with the tooltip "Can't be merged: conflicts with <base>, or a branch protection rule isn't met". Otherwise the tooltip is "Merge into <base>". Choosing an item opens a modal NSAlert "<Style> #N into <base>?" with the buttons "<Style>" and "Cancel".
- **Reaction:** Cancel does nothing. Confirming: Forgejo `POST /repos/{o}/{r}/pulls/{n}/merge` with body `{"Do":"merge|rebase|squash"}`; GitHub `PUT …/pulls/{n}/merge` with `{"merge_method": …}`. No title, message or delete-branch fields are sent.
- **Edge cases:** `mergeable` nil (GitHub list data, or GitHub still computing) leaves the menu enabled. A draft PR isn't blocked by the client. Not shown for closed or merged PRs.
- **Source:** PullRequestsView.swift:350-359, PullRequestsView.swift:408-413, Forgejo.swift:446-454

### PD-30 After a merge: open the target branch's History
- **Trigger:** a successful merge (PD-29).
- **Reaction:** `back()` deselects, which stops polling and reloads the list. Then `showMerged(into: base)`: busy "Fetching <base>", `git fetch <remote> <base>` (with gitAuth), refsVersion bumped, worktrees refreshed. The tab switches to History, scope = branch `<remote>/<base>`. If History was already on that scope, it clears the selection and reloads; otherwise it sets the scope. Either way the top commit, the merge, ends up selected (History side: see the History agent).
- **Edge cases:** the base branch is captured before the merge. A failed fetch shows the "Git" alert and the switch to History still happens.
- **Source:** PullRequestsView.swift:414-419, Model.swift:503-519

### PD-31 Merge failure
- **Trigger:** the server rejects the merge (405 not mergeable, 409 head moved, 403 protection rule).
- **Reaction:** `pull.error = "Forgejo 405: …"` (or "GitHub …"), the alert shows, and you stay on the PR.
- **Source:** PullModel.swift:520-524, PullRequestsView.swift:307-309

### PD-32 Fetching the PR head locally
- **Trigger:** Overview, Full mode, Markdown preview, navigation, or the large-diff fallback.
- **Reaction:** `git cat-file -e <head>^{commit}` and `git cat-file -e <base>^{commit}`. If either is missing: `git fetch --no-tags <remote> +refs/pull/N/head:refs/remotes/<remote>/pr/N +refs/heads/<base>:refs/remotes/<remote>/<base>` with the gitAuth environment. Works for forks.
- **Source:** PullModel.swift:286-295, Model.swift:547-554

### PD-33 Detail error alert
- **Trigger:** any failure in a detail load, merge, review, comment, full-file load or navigation fetch.
- **Render:** an alert titled "Forgejo" (on GitHub too) with the message and an "OK" button.
- **Reaction:** OK clears the error.
- **Source:** PullRequestsView.swift:307-309

## Conversation and review

### RV-01 Conversation layout
- **Trigger:** view switch > Conversation.
- **Render:** a timeline column (max 960 pt, centred) with a 2 pt vertical rail behind the avatars. It opens scrolled to the bottom (the newest). The Composer (RV-10) is under a divider. The sidebar (RV-08/09) shows only when the pane is at least 1250 pt wide (960 + 290).
- **Reaction:** the timeline is rebuilt only when the pr, comments, reviews, inline comments or commits changed (memo).
- **Source:** Conversation.swift:6-47, Conversation.swift:149-158

### RV-02 Timeline order and grouping
- **Preconditions:** a fixture with interleaved comments, reviews and pushes, two comments by the same user within 30 min, and a commit dated before the PR.
- **Render:** the "opened" card is always first. Everything else is sorted by date (stable). Back-to-back commits merge into one row. Back-to-back issue comments by the same user less than 30 min apart merge into one card, divided by a hairline and the time (H:mm). Anything in between, such as a push, starts a new card.
- **Reaction:** a COMMENT review with an empty body and no inline comments is left out. Inline comments whose review id isn't among the reviews show as standalone cards (RV-05).
- **Source:** Conversation.swift:132-200

### RV-03 Opened card (the description)
- **Render:** the author's avatar and "<login> opened this pull request" with a relative date. The body is Markdown (RV-11), or the italic secondary text "No description provided." when it is empty or whitespace. The opened card shows no "Author" tag.
- **Edge cases:** if `created_at` is missing there is no opened card, so the description is not shown at all.
- **Source:** Conversation.swift:52-58, Conversation.swift:162

### RV-04 Comment and review cards
- **Render:** a comment card: "<login> commented". A review card: an avatar badge (green ✓ for APPROVED, orange ! for REQUEST_CHANGES, none otherwise) and a tinted header and border. The header action is "approved these changes", "requested changes", "was asked to review" (REQUEST_REVIEW) or "reviewed". An "earlier commit" tag shows when the review's commit_id differs from head, with the tooltip "Made on a commit that's no longer the head of this pull request". Then the review body in Markdown, then its inline comments (RV-05).
- **Edge cases:** header tags: "bot" when the login ends in "[bot]" or in "bot" (case-insensitive), "Author" when the login is the PR author's. The relative date has the full date as tooltip and is hidden when the date is unknown.
- **Source:** Conversation.swift:59-84, Conversation.swift:290-379

### RV-05 Inline comment block
- **Render:** a clickable header: file icon, path (truncated at the head), "line N" (position, else original_position), an "outdated" tag when commit_id ≠ head, and an ↗ arrow (tooltip "Open in Files"). Below it, the last 4 lines of the diff hunk, syntax highlighted, with +/− marks and add/delete backgrounds. Then an 18 pt avatar, the login, and the body in Markdown. A standalone inline comment's card is titled "<login> commented on a line".
- **Reaction:** clicking the header switches to Files and reveals the path, at `position` when it is > 0 (PD-23).
- **Source:** Conversation.swift:85-91, Conversation.swift:434-511

### RV-06 Commit rows and day markers
- **Render:** commit row: a dotted icon on the rail, the avatar and login of the last commit's author (or the git author name), "added a commit" or "added N commits", a relative date, and one line per commit: short sha plus the first line of the message (tooltip = full message, selectable). Day marker capsule: "Today", "Yesterday", "<Weekday>, <day> <Month>", or "Earlier" when the date is unknown.
- **Source:** Conversation.swift:92-93, Conversation.swift:381-432, Conversation.swift:155

### RV-07 Sidebar: Reviewers
- **Trigger:** click a reviewer row (wide window only).
- **Render:** "No reviews yet", or one row per reviewer state (PD-07 logic): avatar, login, then "Approved" (green), "Changes" (orange) or "Pending" (circle.dashed).
- **Reaction:** scrolls (animated, 0.25 s) to that user's latest review card. The row is disabled when they have no review card (only requested). Tooltip "Go to <login>'s review".
- **Source:** Conversation.swift:228-258

### RV-08 Sidebar: Participants
- **Render:** a grid of 28 pt avatars of every login in the timeline, sorted, with the login as tooltip.
- **Source:** Conversation.swift:259-272

### RV-09 Markdown rendering (comments and bodies)
- **Preconditions:** a comment with headings, a list with [ ] / [x], a quote, a fenced code block with a language, a table with an escaped `\|`, `---`, HTML, `<details><summary>`.
- **Render:** headings (comment sizes), bullets "•" and numbered markers, ☐/☑ checkboxes, quotes with a bar, highlighted code (max 30 lines), tables (header in bold, custom TableLayout), rules. HTML tags are removed and their text kept; `<summary>` becomes a bold heading. Single line breaks are kept (comment mode). Text is selectable.
- **Source:** Conversation.swift:600-768

### RV-10 Post a conversation comment
- **Trigger:** the Composer field "Leave a comment" (3–12 lines) and the "Comment" button, or ⌘↩. The hint reads "Markdown · ⌘↩ to send".
- **Reaction:** `POST /repos/{o}/{r}/issues/{n}/comments` with `{"body": …}`, then `GET …/issues/{n}/comments`. A spinner shows while sending, and the field is cleared afterwards.
- **Edge cases:** disabled while the trimmed text is empty or a send is running. On failure the alert shows but the text is still cleared (BROKEN). The PR's comment count updates only on the next detail load.
- **Source:** Conversation.swift:98-128, PullModel.swift:515-518, Forgejo.swift:335-337

### RV-11 Existing review comments in the diff
- **Preconditions:** a Forgejo review comment on a new-side line (position = new line), one on an old-side line (original_position), and on GitHub a LEFT and a RIGHT comment.
- **Render:** an inline row under the line in a blue (accent) box with the author login and the body.
- **Reaction:** a comment is placed under the new-side line when position > 0, otherwise under the old-side line; context lines take either. GitHub LEFT/RIGHT `line` is mapped to old/new.
- **Edge cases:** a comment whose line isn't in the shown diff is dropped silently (and still counted in the tree). Unchanged files in Full mode show no comments. The vault confirms that position and original_position are line numbers and that comments render on the right side.
- **Source:** PullModel.swift:489-504, DiffTableView.swift:265-288, Forgejo.swift:300-315

### RV-12 Add a draft (pending) comment
- **Trigger:** right-click a diff line > "Add Comment…".
- **Preconditions:** a changed file is shown. In Full mode the line must be inside the PR's hunks.
- **Render:** a transient popover titled "<path> · Line N", or "Line N (old)" for a removed line. It has a focused TextEditor, "Cancel" (Esc) and "Add Comment" (⌘↩, disabled when the text is only spaces).
- **Reaction:** adds `DraftComment{path, newLine (nil for removed), oldLine (removed only), body}`. The draft shows under the line as "You · Pending" in an orange box with a trash button. The header button becomes "Review (N)". Nothing is sent to the server until the review is submitted.
- **Edge cases:** hunk and meta rows can't take a comment. Unchanged files in Full mode have no menu item. Lines outside the original hunks in Full mode have no item. A body of only newlines enables the button but adds nothing. Double-click selects a word; it does not add a comment (the vault's "draft added by double-click" is stale).
- **Source:** DiffTableView.swift:671-686, DiffTableView.swift:697-700, DiffTableView.swift:1230-1258, PullRequestsView.swift:540, PullRequestsView.swift:570-572, PullModel.swift:410-419

### RV-13 Delete a draft
- **Trigger:** the trash icon on a "You · Pending" row.
- **Reaction:** removes the draft by id and saves. The count updates.
- **Source:** DiffTableView.swift:1196-1216, PullRequestsView.swift:541, PullRequestsView.swift:571

### RV-14 Draft persistence
- **Reaction:** every change writes the JSON to UserDefaults `drafts:<host>/<owner>/<name>#<n>` (the key is removed when empty). Drafts are restored when the PR model is created, so they survive relaunch, worktree switches and cache trimming.
- **Edge cases:** drafts keep the line numbers they were made on. After a new push they are not adjusted (BROKEN).
- **Source:** PullModel.swift:151, PullModel.swift:155, PullModel.swift:161-169

### RV-15 Comment counts in the tree
- **Render:** a bubble.left.fill count after a changed file's row. It counts server comments plus drafts for that path.
- **Source:** PullRequestsView.swift:480-481, PullModel.swift:491

### RV-16 Submit review popover
- **Trigger:** header button "Review" or "Review (N)".
- **Render:** a popover headed "Finish your review": a 400×120 TextEditor, a radio group "Comment" (default) / "Approve" / "Request Changes", and "No pending comments" or "N pending comment(s)". Buttons: "Cancel" (Esc) and "Submit Review" (⌘↩, prominent).
- **Reaction:** Submit is disabled while sending, or when the choice isn't Approve and the body is empty and there are no drafts.
- **Edge cases:** a body of only spaces counts as not empty. Body and choice reset each time the popover opens.
- **Source:** PullRequestsView.swift:360-364, PullRequestsView.swift:644-681

### RV-17 Submit review (API)
- **Trigger:** "Submit Review".
- **Reaction:** Forgejo: `POST /repos/{o}/{r}/pulls/{n}/reviews` with body `{body, event: "COMMENT"|"APPROVED"|"REQUEST_CHANGES", commit_id: head.sha, comments: [{path, body, new_position, old_position}]}`; old_position is set only when new_position is nil. GitHub: the same endpoint with `{body, event: "COMMENT"|"APPROVE"|"REQUEST_CHANGES", commit_id, comments: [{path, body, line, side: "RIGHT"|"LEFT"}]}`; drafts with no line are dropped. On success: drafts are cleared, the detail reloads fully (PD-05) and the popover closes.
- **Edge cases:** on failure the alert shows, the popover stays open and the drafts are kept. Approving your own PR or requesting changes on it is rejected by the server and shows the alert. The vault confirms the Forgejo `comments[{path, body, new_position|old_position}]` shape and an end-to-end submit (src/new8.txt line 21).
- **Source:** PullModel.swift:506-513, Forgejo.swift:339-359

## Forge API behaviours

### FG-01 API root, auth and headers
- **Reaction:** Forgejo uses `<base>/api/v1`. github.com uses `https://api.github.com`; GitHub Enterprise uses `<base>/api/v3`. Every request sends `Authorization: token <t>`. Accept is `application/json`; GitHub JSON calls send `application/vnd.github+json`. GETs use `reloadRevalidatingCacheData`, so GitHub 304s don't count against the rate limit.
- **Source:** Forgejo.swift:203-227

### FG-02 HTTP error messages
- **Reaction:** a non-2xx response throws "<Forgejo|GitHub> <status>: <JSON message, or the first 300 bytes of the body>". No response gives "No HTTP response". Network errors show the URLError text. Cancellations are suppressed and keep the previous error. Where they show: the list's "Couldn't Load" (PR-18) or the detail's "Forgejo" alert (PD-33).
- **Source:** Forgejo.swift:229-237, Git.swift:5-8

### FG-03 Bad or revoked token
- **Preconditions:** the mock forge returns 401 or 403.
- **Render:** "Forgejo 401: …" in the list or the alert. There is no re-auth prompt or Settings link at this point; the "Connect" screen appears only when no token is stored at all.
- **Source:** Forgejo.swift:232-235, PullRequestsView.swift:9-21

### FG-04 Rate limit
- **Preconditions:** GitHub 403/429 with the message "API rate limit exceeded".
- **Reaction:** no special handling or backoff. The message shows as in FG-02, and polling keeps going every 60 s.
- **Source:** Forgejo.swift:219, Forgejo.swift:232-235

### FG-05 Network failure or offline
- **Reaction:** the list keeps its last data (or shows "Couldn't Load" when empty). The detail raises the alert on every failed poll. Nothing is retried immediately; the next poll, activation or refresh tries again.
- **Source:** PullModel.swift:56, PullModel.swift:270, PullRequestsView.swift:295-303

### FG-06 Pagination
- **Reaction:** the PR list is paged at 50 (PR-05/PR-12). `getAll` follows page=N with `limit` or `per_page` = 50 until a short page, for reviews, commits, GitHub inline comments and GitHub issue comments. Forgejo issue comments are fetched in one call with no paging (Forgejo ignores it). Forgejo review comments are one call per review. Combined status is paged at 50 (CI-).
- **Source:** Forgejo.swift:210, Forgejo.swift:244-253, Forgejo.swift:283-333

### FG-07 GitHub diff too large: local fallback
- **Preconditions:** a GitHub PR with more than 300 files or 20k lines; the mock returns 406 or 422 for the diff media type.
- **Reaction:** `DiffTooLarge`, then `fetchHead()` (PD-32), then `git diff --no-color --no-ext-diff -M <base.sha>...<head.sha>`. Polls skip the work while the head is unchanged. The vault confirms the output is byte-identical to GitHub's diff and that diffing from base.sha (not the tip) works after a merge.
- **Source:** Forgejo.swift:272-279, PullModel.swift:273-284

### FG-08 GitHub response mapping
- **Reaction:** review states CHANGES_REQUESTED becomes REQUEST_CHANGES, COMMENTED becomes COMMENT, DISMISSED sets `dismissed = true`; the viewer's PENDING review is dropped. `merged_at` sets `merged`. Inline comments with `line`/`side` are mapped to position/original_position. Merge uses PUT with `merge_method`, and review approval uses "APPROVE".
- **Source:** Forgejo.swift:266-270, Forgejo.swift:281-315, Forgejo.swift:341-353, Forgejo.swift:448-452

### FG-09 Git credentials for PR fetches
- **Reaction:** fetches for checkout, the head and a merged base run with `GIT_CONFIG_COUNT=2`. The first entry resets `credential.<baseURL>.helper`; the second is an inline helper that answers with the space account's username and token. SSH remotes keep using the user's own keys.
- **Source:** Model.swift:544-554

### FG-10 Stale data and coalescing
- **Reaction:** the list allows one load per state at a time, with one queued rerun, and drops results when the state changed. The detail allows one load at a time with one queued rerun. Unchanged list or diff data doesn't re-render (PR-13, PD-05). A selection that was removed by a push falls back to the first unviewed file.
- **Source:** PullModel.swift:23-40, PullModel.swift:229-263

### FG-11 Dev aid: `Loupe --api <path>`
- **Trigger:** run `Loupe --api /repos/o/r/pulls/1/reviews` from a terminal.
- **Reaction:** prints the raw response using the first account's token and `<base>/api/v1`, then exits. It always uses `/api/v1`, even for a GitHub account.
- **Source:** App.swift:51-59

# 9. PR create

## PR Create — Entry points

### PC-01 "Create a pull request" banner in Changes
- **Trigger:** The Changes tab, above the list. It is replaced by the operation or conflict banner while a merge or rebase is in progress or there are conflicts.
- **Preconditions:** All of:
  - the branch is set (not detached)
  - an account and remote exist
  - `aheadOfDefault` is not nil: there is an integration target and it is not the branch itself
  - `rev-list --count` is above 0
  - `!isMerged`
  - `openPull` finds no open PR
  - the branch isn't dismissed
- **Render:**
  - An `arrow.triangle.pull` icon in the accent colour.
  - "N commit(s) on <branch> that isn't/aren't on <target>", one line, middle truncation, tooltip "<branch> → <target>".
  - The prominent "Create Pull Request…" button, then an `xmark` button.
  - Small controls on an 8 % accent background, with a divider.
- **Reaction:**
  - Git: `git rev-parse <ref> --abbrev-ref <ref>` for origin/HEAD, origin/main, origin/master, main, master (the first that resolves), then `git rev-list --count <target>..HEAD`.
  - API:
    - GitHub: `GET /repos/{o}/{r}/pulls?state=open&head={owner}:{branch}`.
    - Forgejo: `GET /repos/{o}/{r}/pulls?state=open&page=N&limit=50` (all pages). It looks for `head.ref == branch && head.repo_id == base.repo_id`, then `GET /pulls/{n}`.
- **Edge cases:**
  - An offline or failing API call hides the banner.
  - The target name drops its "origin/" prefix.
  - The check re-runs when headOID, branch, push count or client presence changes.
- **Source:** PullCreate.swift:381-437; ChangesView.swift:233-238; Git.swift:184-190; PullCreate.swift:41-60.

### PC-02 Banner "Create Pull Request…" button
- **Trigger:** Click it.
- **Reaction:** `app.creatingPull = path`, which opens the sheet (PC-07).
- **Source:** PullCreate.swift:401.

### PC-03 Dismiss the banner
- **Trigger:** The `xmark` button (tooltip "Don't offer a pull request for this branch again").
- **Reaction:** Appends "<path>\t<branch>" to the `prBannerDismissed` setting (newline-separated). The banner hides for that branch in that worktree permanently.
- **Edge cases:** There is no way in the UI to undo a dismissal.
- **Source:** PullCreate.swift:387, 393, 403-404.

### PC-04 Banner re-check after a push or after the sheet closes
- **Trigger:**
  - The Changes toolbar "Push" button: `git push --set-upstream <remote> HEAD`, then a `.branchPushed` notification.
  - Or the Create sheet closing for this path.
- **Reaction:** `pushes += 1`, which re-runs the check. After a PR is created the banner disappears because an open PR now exists.
- **Source:** PullCreate.swift:413-415; ChangesView.swift:40.

### PC-05 "Create Pull Request…" in the Pull Requests list toolbar
- **Trigger:** A button with a "plus" icon in the PR list toolbar, after the filter field. Tooltip "Create a pull request for <b>".
- **Preconditions:**
  - `canCreate` (`aheadOfDefault != nil`)
  - the branch is set
  - the loaded list contains no open PR whose head.ref equals the branch
- **Reaction:**
  - `aheadOfDefault` runs once per "headOID refsVersion" key, cached on the WorktreeModel (`aheadKey` / `aheadCache`).
  - `refsVersion` bumps after fetch, pull or push. Tab switches reuse the cache (vault: 1d10383, "checked once per HEAD").
  - Clicking sets `app.creatingPull = path`.
- **Edge cases:**
  - Only the loaded list is checked. With the "Closed" filter selected, an existing open PR isn't seen and the button shows.
  - It doesn't check `isMerged`, unlike the banner.
- **Source:** PullRequestsView.swift:35, 67-70, 127-132; Model.swift:293-295, 507.

### PC-06 Sidebar worktree context menu "Create Pull Request…"
- **Trigger:** Right-click a worktree row → "Create Pull Request…", after a divider.
- **Preconditions:** The worktree has a branch and its model has a client (an account for the remote). The ahead count is not checked.
- **Reaction:** `app.creatingPull = wt.path`.
- **Edge cases:**
  - It can open for a branch with no new commits: the sheet then shows "No commits…" and Create is disabled.
  - It doesn't change the sidebar selection until success.
- **Source:** App.swift:371-374.

There is no menu-bar item and no keyboard shortcut for creating a PR.

## PR Create — Sheet

### PC-07 Opening and loading the sheet
- **Trigger:** `app.creatingPull` is set.
- **Render:**
  - `.sheet(item:)` at the window root. A grouped Form, width 560, min height 520, titled "New Pull Request".
  - A centred ProgressView while loading.
- **Reaction** (`load()`, once):
  1. No client or remote: error "No account for this repository's remote." and stop.
  2. Empty branch: error "HEAD is detached; switch to a branch first." and stop.
  3. Template: read from the worktree folder on disk. The first non-empty file wins, in this order:
     - GitHub: `.github`, `.forgejo`, `.gitea`, then `docs`, then the root
     - Forgejo: `.forgejo`, `.gitea`, `.github`, then `docs`, then the root
     - in each folder, `pull_request_template.md` before `PULL_REQUEST_TEMPLATE.md`
  4. `git for-each-ref --format=%(refname:lstrip=3) --sort=-committerdate refs/remotes/<remote>`, leaving out "HEAD" and the current branch.
  5. Default branch:
     - `GET /repos/{o}/{r}` → `default_branch`
     - if that fails, the integration target without its "<remote>/" prefix
     - it is inserted first if missing and not the current branch
  6. `bases` = the first 200. `base` = the default branch, or the first name.
  7. `existing = openPull(...)`, as in PC-01.
  8. `baseChanged()` (PC-08).
- **Source:** App.swift:176-178; PullCreate.swift:198-212, 104-120, 288-376.

### PC-08 "Into" base picker
- **Trigger:** The Picker "Into" (menu style) listing `bases`.
- **Reaction:** On change (ignored while loading), `baseChanged()` runs:
  - `git log -n 100 --format=%H%x1f%s%x1f%b%x1e <remote>/<base>..HEAD`
  - and `git rev-list --count <remote>/<base>..HEAD`, in parallel
  - The result is discarded if the base changed again meanwhile.
  - It recomputes commits, total, the automatic title and body, and `needsPush` (PC-15).
- **Edge cases:**
  - A default branch from the API that has no remote ref (not fetched) makes the log fail, so there are 0 commits and Create is disabled.
  - Empty bases: an empty picker and `base = ""`, so Create is disabled.
- **Source:** PullCreate.swift:214-230, 305-308.

### PC-09 Title default
- **Render:** A TextField "Title".
- **Reaction:**
  - One commit: its subject.
  - Otherwise: the branch name made readable. Its last path segment has "-" and "_" turned into spaces and the first letter uppercased ("feat/add-login_page" → "Add login page").
  - The title only follows a base change while the user hasn't edited it (`title == autoTitle`).
- **Source:** PullCreate.swift:91-102, 223-226, 310.

### PC-10 Description default and template
- **Render:**
  - A TextEditor, height 160, with a tertiary "Description" placeholder while empty.
- **Reaction:**
  - The body is the template if one exists.
  - Otherwise, a single commit's body; with several commits it is empty.
  - Like the title, it only follows the base until the user edits it.
- **Source:** PullCreate.swift:122-125, 323-328.

### PC-11 Write with AI (sparkles)
- **Trigger:** The `sparkles` button beside Title. It is only present if an AI harness is installed or selected (`CommitAI.selected`; Settings `aiHarness` ≠ "off").
  - Tooltip: "Write the title and description with <harness>, from the commits and their diff".
  - It is disabled, in secondary colour, when there are no commits or something is working.
- **Reaction:**
  - While writing, the button is replaced by a small spinner.
  - `git diff --no-color --no-ext-diff -M <remote>/<base>...HEAD -U2` and `… --stat`.
  - The prompt includes the template and the user's extra AI instructions. The diff is capped at 60,000 bytes and marked "[diff truncated]".
  - The reply is split:
    - the first line is the title, with a leading "# ", "Title:" or "title:" and surrounding quotes removed
    - the rest is the body
  - The title is replaced if the reply has one. The body is always replaced, overwriting any edits.
- **Edge cases:** An error appears in the error section.
- **Source:** PullCreate.swift:127-161, 232-245, 311-321.

### PC-12 Draft toggle
- **Trigger:** The Toggle "Draft".
  - Tooltip on GitHub: "Open it as a draft pull request".
  - Tooltip elsewhere: "Open it as a draft: the title starts with “WIP: ”".
- **Reaction:**
  - GitHub: the body gets `draft: true`. The key is left out when not a draft.
  - Forgejo and Gitea: the title gets a "WIP: " prefix, unless it already starts with "wip:" or "[wip]" (case-insensitive).
- **Source:** PullCreate.swift:6-20, 329-330.

### PC-13 Commits section
- **Render:**
  - Header "1 commit" or "N commits", where N is the full count.
  - Rows: the 7-character sha (caption mono secondary) and the subject on one line. The tooltip is the subject plus the body.
  - "and N more" when there are more than 100.
  - When there are none and loading has finished: "No commits on <branch> that aren't on <base>."
- **Source:** PullCreate.swift:332-344.

### PC-14 Branch row and the "pushed first" note
- **Render:**
  - "Branch" with the branch name in mono, middle truncation, full name as tooltip.
  - If `needsPush`, a caption "pushed first" follows it.
- **Reaction:** `needsPush` is true when `git merge-base --is-ancestor HEAD <remote>/<branch>` fails: no remote branch, or local commits not yet pushed.
- **Source:** PullCreate.swift:227-229, 299-304.

### PC-15 Create button state
- **Trigger:** The confirmation toolbar button, labelled "Push and Create" when `needsPush`, otherwise "Create Pull Request". It is the default action (Return outside the TextEditor).
- **Preconditions:** It is disabled when any of these holds:
  - loading
  - working
  - the base is empty
  - there are no commits
  - the title is blank (only whitespace)
- **Render:** A small spinner beside it while "Pushing" or "Creating".
- **Edge cases:** An existing PR shown at load time doesn't disable Create.
- **Source:** PullCreate.swift:364-371.

### PC-16 Push before create
- **Trigger:** Create with `needsPush`.
- **Reaction:**
  - `git push --set-upstream <remote> HEAD`, using the space's credential env (GIT_CONFIG_COUNT helper with LOUPE_GIT_USER and LOUPE_GIT_TOKEN; HTTPS only).
  - Then `needsPush = false` and `worktree.refreshStatus()`.
- **Edge cases:**
  - A push failure shows git's error in red. `needsPush` stays true, so the label still says "Push and Create".
  - No `.branchPushed` is posted. The banner re-checks when the sheet closes instead.
- **Source:** PullCreate.swift:249-258; Model.swift:547-555.

### PC-17 Create the PR, then show it
- **Reaction:**
  - `POST /repos/{o}/{r}/pulls` with JSON `{head: <branch>, base, title (trimmed), body, draft?}`. The head has no "owner:" prefix, so only same-repo PRs work.
  - On a 2xx response the body is decoded as a PullRequest.
  - Success:
    - the sheet closes (`creatingPull = nil`)
    - the sidebar selects this worktree
    - the tab switches to "pulls"
    - the PR list reloads, the new PR is inserted at the top if missing, and it is selected
    - so the PR detail opens
  - No toast.
- **Source:** PullCreate.swift:33-38, 259-262, 273-283.

### PC-18 A PR for this branch already exists
- **Trigger:**
  - At load, when `openPull` finds one.
  - Or on create: 409 (Forgejo) or 422 (GitHub) with "already exists" in the reason.
- **Render:** A section at the bottom:
  - red error "A pull request for this branch already exists." (create path only)
  - a row "#N title" with the button "Open Pull Request"
- **Reaction:**
  - On the create path, `openPull` runs again to fill the row.
  - "Open Pull Request" runs `show(pr)`, the same navigation as PC-17.
- **Source:** PullCreate.swift:22-31, 210, 263-266, 345-356.

### PC-19 Other create errors
- **Render:** Red callout text, selectable: "<GitHub|Forgejo> <status>: <reason>".
  - The reason is GitHub's `errors[].message` joined with spaces, or the response's `message`, or the first 300 bytes of the raw body.
  - "No HTTP response" if there is none.
  - A response that won't decode shows the decoding error.
- **Edge cases:**
  - Gitea is labelled "Forgejo".
  - The error and existing PR are cleared at the start of each create.
- **Source:** PullCreate.swift:24-37, 267-270.

### PC-20 Cancel
- **Trigger:** The toolbar "Cancel" (cancellation action, Esc).
- **Reaction:** `app.creatingPull = nil`. Nothing is pushed or created, and the banner re-checks (PC-04).
- **Edge cases:** Cancelling during a push or create doesn't stop it: the task finishes, and on success `show()` still navigates.
- **Source:** PullCreate.swift:363.

---

# 10. CI (checks, jobs, logs, tail, rerun, problem lines)

Paths below are relative to /Users/extypen/loupe/Sources/Loupe/. "vault" means /Users/extypen/vaults/extypen-claude/Projects/loupe/index.md.

## CI — Entry, badge and background refresh

### CI-01 Checks badge in the PR header
- **Trigger:** Open any PR (Pull Requests tab → row). The badge sits in the PR header's second row, after the review badges.
- **Preconditions:** A PR whose head sha has 0..N commit statuses or check runs.
- **Render:**
  - total == 0: a `circle.dashed` (secondary) icon and the text "Checks".
  - Otherwise, left to right:
    - failing count with `xmark.circle.fill` in red, only if > 0
    - running count with `circle.fill` in orange, only if > 0; static, it does not pulse
    - queued count with `circle.dashed` in secondary, only if > 0
    - passed count with `checkmark.circle.fill` in green, always shown
    - the word "checks" in secondary
  - Digits are monospaced and the label style is titleAndIcon.
  - Tooltip: "CI checks — P passed, R running, Q queued, F failing".
- **Reaction:** `.task` runs `ChecksModel.refresh()` when the badge appears (see CI-05 to CI-07).
- **Edge cases:**
  - The badge never shows skipped checks. With only skipped checks it reads "0 checks" next to the green check icon.
  - "running" counts statuses whose description contains "running" or "started", and pending statuses whose linked job reports running.
- **Source:** CI.swift:551-574, PullRequestsView.swift:380-383; the comment that the dot does not pulse is at CI.swift:562 (vault: idle energy fix f310e64).

### CI-02 Open the checks overlay
- **Trigger:** Click the checks badge.
- **Preconditions:** PR detail is open.
- **Render:**
  - A window-wide overlay over everything, including the sidebar, with a 0.15 s ease-out opacity transition.
  - Backdrop: black at 35 % opacity.
  - Card: width min(window − 64, 1500) by height window − 64, corner radius 14, separator stroke, shadow (radius 30, y 10), window background.
  - It is a custom ZStack, not a macOS sheet (vault: "a macOS sheet hangs off the top edge and overflowed").
- **Reaction:** `app.checksOverlay = (pull.checks, client.baseURL)`. The ChecksModel is the PR model's lazy instance, so its state is shared with the badge.
- **Edge cases:**
  - The overlay lives at ContentView level. Switching worktree or tab underneath (⌘1/2/3, ⌘K) leaves it on screen.
- **Source:** PullRequestsView.swift:380-382, App.swift:170-174, Model.swift:50, CI.swift:915-935.

### CI-03 Background checks refresh while the PR is open
- **Trigger:** Any PR reload. That covers the 60 s PR poll, the app becoming active, the PR refresh button, ⌘R (Review › Refresh, which posts `.refresh`), and the badge appearing.
- **Preconditions:** PR detail is open.
- **Render:** The badge counts update in place. There is no spinner on the badge.
- **Reaction:**
  - `PullRequestModel.loadOnce` sets `checks.sha = pr.head.sha` when the head moved.
  - It then fires `Task { await checks.refresh() }` without awaiting it.
- **Edge cases:**
  - A new push moves the sha: the same ChecksModel switches to the new sha. Old runs drop out of the `jobs` cache because their ids are no longer present.
  - Vault (2026-11-12): checks now refresh with every PR reload, not only on a new push.
- **Source:** PullModel.swift:150, 245-247; PullRequestsView.swift:295-303; App.swift:104.

### CI-04 Forge feature detection
- **Trigger:** The first refresh of any ChecksModel for a given server.
- **Preconditions:** An account for github.com, a Forgejo server or a Gitea server.
- **Render:** Nothing directly. It decides whether the notice appears (CI-11) and which rerun or cancel buttons appear (CI-32 to CI-35).
- **Reaction:**
  - GitHub: no request is made. Every write is allowed.
  - Forgejo and Gitea: `GET {base}/api/v1/version` → `{version}`.
  - The detected forge is:
    - Forgejo if the major version is ≥ 2, or the string contains "+gitea-" or "forgejo".
    - Gitea otherwise (1.x).
  - What each forge allows:

    | Forge | Actions API | Cancel | Rerun run | Rerun job | Rerun failed |
    |---|---|---|---|---|---|
    | Forgejo | always | ≥ 16 | ≥ 17 | ≥ 17 | never |
    | Gitea | ≥ 1.25 | ≥ 1.27 | ≥ 1.26 | ≥ 1.26 | ≥ 1.26 |
    | GitHub | yes | yes | yes | yes | yes |

  - The result is cached in a static dictionary per baseURL for the app's lifetime.
- **Edge cases:**
  - If /version fails or can't be parsed: the forge is treated as Forgejo with the Actions API on and no writes. This result is not cached, so the next refresh retries.
  - A plain "17.0.0" is Forgejo, not Gitea (an earlier misdetection decoded Forgejo runs with GitHub's shapes).
  - The mock forge reports "11.0.0+gitea-1.22.0", so it is Forgejo 11: no cancel and no rerun.
- **Source:** CI.swift:107-114, CIForges.swift:7-42, 180-183; mockforge.py:279-280.

### CI-05 Statuses on Forgejo or Gitea
- **Trigger:** `refresh()`.
- **Preconditions:** A head sha with commit statuses.
- **Reaction:**
  - `GET /api/v1/repos/{o}/{r}/commits/{sha}/status?page=N&limit=50`, repeated until a page has fewer than 50 statuses or `count ≥ total_count`.
  - This runs in parallel with the runs request (CI-07).
- **Edge cases:**
  - The status list includes non-Actions statuses, such as `staged-ai-review/*` (vault).
  - A missing `total_count` counts as 0, so paging stops after the first page.
- **Source:** Forgejo.swift:363-372, CI.swift:80-82.

### CI-06 Statuses and check runs on GitHub
- **Trigger:** `refresh()` on a GitHub account.
- **Reaction:**
  - Two requests in parallel:
    - `GET https://api.github.com/repos/{o}/{r}/commits/{sha}/status?per_page=100`
    - `GET …/commits/{sha}/check-runs?per_page=100`
  - On GitHub Enterprise the API root is `{base}/api/v3`.
  - Both become CommitStatus rows. For check runs:
    - status: not completed → "pending"; success or neutral → "success"; skipped or stale → "skipped"; anything else → "failure".
    - description = `output.title`
    - target_url = `details_url ?? html_url`
    - `checkRun = {id, app.slug, output.summary, annotations_count}`
  - GET requests use `reloadRevalidatingCacheData`, so URLCache doesn't serve GitHub's max-age=60 responses.
- **Edge cases:**
  - There is no paging past 100 entries on either endpoint.
  - An Actions check run's id equals its job id.
- **Source:** Forgejo.swift:376-404, 203-221.

### CI-07 Runs and jobs, and linking a status row to its job
- **Trigger:** `refresh()`.
- **Reaction:**
  - Runs request (only if `features.actions`):
    - Forgejo: `GET /repos/{o}/{r}/actions/runs?head_sha={sha}&limit=100`, decoded as `{workflow_runs:[ActionRun]}`.
    - GitHub: `GET /repos/{o}/{r}/actions/runs?per_page=100&head_sha={sha}`.
    - Gitea: the same path with `limit=100`.
    - GitHub and Gitea runs are mapped: `index_in_repo = run_number`, `title = display_title ?? name`, `workflow_id = path`, and status + conclusion become one status:
      - completed + success or neutral → success
      - completed + skipped or stale → skipped
      - completed + cancelled → cancelled
      - completed + anything else → failure
      - in_progress → running
      - queued and the like → waiting
  - Jobs request, only for runs with no cached jobs or a status that is not success, failure, cancelled or skipped. Finished runs are not refetched.
    - Forgejo: `GET /actions/runs/{id}/jobs` (no limit; returns no steps, per the vault).
    - GitHub and Gitea: the same path with per_page or limit = 100, decoded as `{jobs:[GHJob]}`, `labels → runs_on`, `run_attempt → attempt`.
  - Linking a status to its job:
    - GitHub: `checkRun.id == job.id`.
    - Forgejo: parse `target_url` "…/actions/runs/<n>/jobs/<k>".
      - `<n>` is matched against `index_in_repo` first, then against id (vault: the URL carries the index but the API wants the id).
      - `<k>` is matched against the job-number segment of each job's `html_url`, keeping the newest attempt.
      - With no `/jobs/` segment and exactly one job, that job is used.
      - Otherwise it falls back to a job whose name equals the context or appears in it as " / name", preferring the longest name, then the highest attempt.
  - Rows are sorted by (state, context lowercased).
  - On success: `lastUpdated = now`, `error = nil`.
- **Edge cases:**
  - If the runs request throws (404 Actions disabled, 403, anything else), runs = [] and only the 403 case sets a notice. Statuses still show.
  - If any jobs request throws, the whole refresh throws. The error goes to the header and the old checks are kept.
  - A cancellation error keeps the previous error.
- **Source:** CI.swift:75-105, 139-158; CIForges.swift:87-149, 185-205; Forgejo.swift:406-418.

### CI-08 How a check's state is classified
- **Trigger:** Rendering any row or count.
- **Reaction:**
  - Status → state:
    - failure, error or warning → Failing
    - success → Successful
    - skipped → Skipped
    - anything else: Running if the description contains "running" or "started" (case-insensitive), else Queued
  - A pending status whose job's state is running becomes Running.
  - Job or step status → state:
    - failure → Failing
    - success → Successful
    - skipped or cancelled → Skipped
    - running → Running
    - else (waiting, blocked) → Queued
  - Section titles, in order: "Failing", "Running", "Queued", "Successful", "Skipped".
  - A check counts as done when its state is Failing, Successful or Skipped.
- **Edge cases:** "warning" counts as failing. "cancelled" counts as skipped, so its icon is `minus.circle`.
- **Source:** CI.swift:11-46.

## CI — Checks overlay (list)

### CI-09 Close the overlay
- **Trigger:** Any of:
  - the "Done" button (keyboardShortcut `.cancelAction`, i.e. Esc)
  - Esc via `onExitCommand`
  - clicking the dimmed backdrop
- **Render:** The overlay fades out over 0.15 s.
- **Reaction:**
  - `app.checksOverlay = nil`.
  - The `.task` running `model.poll()` is cancelled, so polling stops. The ChecksModel stays alive on the PR model.
  - Esc while the detail page is open goes back to the list first; a second Esc closes the overlay.
- **Source:** CI.swift:595-596, 644, 923; App.swift:172.

### CI-10 Overlay header summary
- **Trigger:** The overlay is open in list mode.
- **Render:** One row, left to right:
  1. Status icon, title2 size:
     - any failing: `xmark.octagon.fill` in red
     - else running + queued > 0: `circle.fill` in orange, with `.symbolEffect(.pulse)` active (see BROKEN)
     - else `checkmark.seal.fill` in green
  2. Headline: "F failing · R running · Q queued" (zero parts left out), or "All checks passed" when all three are zero.
  3. Caption: "P of T successful · <sha first 8>".
  4. The "Filter checks" field (CI-12).
  5. A spacer.
  6. Error or notice (CI-11).
  7. "Updated <relative, numeric>", ticking every 1 s through a TimelineView, tertiary style. Only shown once a refresh has succeeded.
  8. Refresh button (`arrow.clockwise`, tooltip "Refresh now").
  9. "Done".
- **Edge cases:** With zero checks the header still shows the green seal and "All checks passed" with "0 of 0 successful".
- **Source:** CI.swift:621-647.

### CI-11 Error and notice in the header
- **Trigger:** A refresh failed, or Actions is unavailable.
- **Render:**
  - An error wins: `exclamationmark.triangle` in orange, caption size, one line.
  - Otherwise a notice: `info.circle` in secondary, up to two lines, with the full text as tooltip. The notice texts are:
    - Gitea older than 1.25: "Gitea <ver> has no Actions API (it arrived in 1.25), so checks show their status only."
    - 403 on runs, GitHub: "This token can't read GitHub Actions (403), so checks show their status only. Jobs and logs need “Actions: Read”."
    - 403 on runs, other forges: "<Forge> won't list Actions runs for this account (403), so checks show their status only."
- **Edge cases:**
  - The 403 is detected as " 403" inside the error message (GitError "Forgejo 403: …").
  - GitHub's 404 for "no Actions access" produces no notice.
  - Gitea errors are labelled "Forgejo" in the message, because `serverName` only knows GitHub or Forgejo. The notice itself says "Gitea".
- **Source:** CI.swift:116-122, 636-637; Forgejo.swift:208, 229-237.

### CI-12 Filter checks
- **Trigger:** The "Filter checks" TextField in the header (rounded border, max width 260).
- **Reaction:**
  - Filters every section by context, case-insensitive substring.
  - A section with no matching rows is hidden.
- **Edge cases:**
  - The filter is local @State and resets when the overlay closes.
  - A filter that matches nothing shows an empty list with no "no results" message (the No Checks overlay only appears when `checks` itself is empty).
- **Source:** CI.swift:581, 603, 634.

### CI-13 Checks list sections and rows
- **Trigger:** The overlay is open.
- **Render:**
  - An inset List with up to five sections in order: Failing, Running, Queued, Successful, Skipped. Each is titled "<Title> · <count>".
  - There is no grouping by workflow or run.
  - Each row:
    - Left: the state icon:
      - failing: `xmark.circle.fill` in red
      - running: `circle.fill` in orange, static, tooltip "Running"
      - queued: `circle.dashed` in secondary, tooltip "Queued / waiting"
      - success: `checkmark.circle.fill` in green
      - skipped: `minus.circle` in tertiary
    - Title: the context is split on " / ". The part after the first segment is shown in medium weight; the first segment (the workflow) follows in secondary.
    - Caption line:
      - the description, if any
      - if a job is linked: runs_on joined (`server.rack`), "attempt N" when N > 1 (`arrow.counterclockwise`), "needs a, b" (`arrow.turn.down.right`)
      - else, if the status has a target_url: "external" (`arrow.up.right.square`)
      - then updated_at as a relative named time ("2 minutes ago")
    - Right: a tertiary `chevron.right`.
  - Row tooltip: the context, plus the description on a new line.
  - Separators run to the leading edge.
- **Reaction:** The list shows only metadata. No log is fetched here (vault: "The list shows metadata only").
- **Edge cases:**
  - An updated_at the ISO8601 formatter can't parse (for example with fractional seconds) shows no time.
  - GitHub rows from check runs have no `needs`.
- **Source:** CI.swift:600-614, 650-697.

### CI-14 Loading and empty states
- **Render:**
  - First load (no checks yet): a centred ProgressView.
  - Loaded and empty: ContentUnavailableView "No Checks", icon `circle.dashed`, text "Nothing has reported a status for this commit."
- **Edge cases:**
  - `loading` is only true while `checks` is empty, so later refreshes show no spinner.
  - If the first refresh errors, "No Checks" appears with the error in the header.
- **Source:** CI.swift:76-77, 615-618.

### CI-15 Overlay polling cadence
- **Trigger:** The overlay is open (`.task` on ChecksSheet).
- **Reaction:**
  - Loop: refresh, then sleep 30 s if every check is done, else 5 s.
  - It runs until the overlay closes. It keeps running while the detail page is shown, because the task belongs to ChecksSheet.
  - Vault: "polls: 5 s while checks are running, 30 s when settled, 2 s for log tails"; there is no websocket or SSE for token clients.
- **Edge cases:** With zero checks `allDone` is false, so it polls every 5 s forever.
- **Source:** CI.swift:160-166, 595.

### CI-16 Manual refresh
- **Trigger:** The `arrow.clockwise` button in the overlay header (tooltip "Refresh now").
- **Reaction:** One immediate `refresh()`, alongside the poll loop. There is no de-duplication.
- **Source:** CI.swift:643.

### CI-17 Open a check's detail page
- **Trigger:** Click a row (a plain button over the whole row).
- **Reaction:**
  - `openCheck = check.id` (the context). The detail page replaces the header and list.
  - The view's identity is "context + jobID.attempt". When a rerun creates a new attempt or job id, the page is rebuilt: a fresh log model and a fresh scroll position.
- **Edge cases:**
  - If the check disappears from `checks` (sha changed), the list shows again, but `openCheck` stays set and reopens the page if the check returns.
  - Checks with the same context share an id (see BROKEN).
- **Source:** CI.swift:580-593, 606-609.

## CI — Detail page

### CI-18 Detail header and back
- **Trigger:** The detail page is open.
- **Render:** A top block with 12 pt padding:
  - "Checks" back button (`chevron.left`, borderless, shortcut ⌘[ "Back to List", user-rebindable)
  - the state icon, using the live job status once the log model exists
  - the context as a headline, up to two lines, selectable
  - a spacer, then the action buttons (CI-32 to CI-35), "Copy Log" (only when there is a job log model), and "Open in <GitHub|Forgejo|Gitea>" (only when there is a web URL)
  - below: the description in callout secondary, selectable
  - the action error in red with `exclamationmark.triangle.fill`
- **Reaction:**
  - Back, ⌘[ and Esc set `openCheck = nil`.
  - That cancels the log poll task, so tailing stops.
- **Edge cases:** ⌘[ is also the PR detail's "Back" shortcut behind the overlay (see BROKEN).
- **Source:** CI.swift:713-741; Shortcuts.swift:70, 103.

### CI-19 Job info line
- **Render:** Only when a job is linked. In caption secondary:
  - Job “<name>”
  - runs_on (`server.rack`)
  - "attempt N" when N > 1
  - a timer with the duration from the first step's start to the last step's stop (or to now while it runs), formatted "Xm Ys" or "Xs"
- **Edge cases:** No duration is shown when there are no steps, a timestamp doesn't parse, the duration is negative, or it is over 3 days.
- **Source:** CI.swift:730-738, 844-851.

### CI-20 Steps strip
- **Render:**
  - Only shown when `log.job.steps` is non-empty, which comes from the per-job GET (Forgejo's list call returns no steps, per the vault).
  - A vertical scroll list, max height 150. Each step shows its state icon, its name (tertiary when skipped) and its duration (not for skipped steps).
- **Reaction:** It updates as each 2 s or 5 s poll replaces `job`.
- **Source:** CI.swift:744, 885-906.

### CI-21 Initial log load
- **Trigger:** The detail page opens for a check with a job (`.task(id: job.id)`).
- **Reaction:**
  - Creates a JobLogModel. It uses `model.features`, or GitHub/Forgejo defaults if those are missing.
  - The problems disclosure starts expanded only when the job is failing.
  - Log request: `GET /repos/{o}/{r}/actions/jobs/{id}/logs`.
    - Accept: `text/plain` on Forgejo or Gitea; `application/vnd.github+json` on GitHub.
    - Forgejo sends `Range: bytes=-16777216` (the last 16 MB). GitHub and Gitea send no Range.
    - A body over 16 MB is cut to its last 16 MB, from the start of a line.
  - `offset` = Content-Range total, or start + count.
  - 206 with `Content-Range: bytes a-b/total` is parsed. 416 means "nothing new" and returns an empty result.
- **Edge cases:**
  - Any non-2xx response throws "Log <code>: <first 200 bytes>". For a running or waiting job this switches to waiting (CI-22); otherwise the error shows below the log in red caption.
- **Source:** CI.swift:226-242, 806-813; Forgejo.swift:420-436.

### CI-22 Waiting-for-log states
- **Render:** A small spinner and callout secondary text:
  - job status queued: "Waiting for a runner to pick up this job…"
  - GitHub: "GitHub makes a job's log available once the job has finished; the steps above update meanwhile."
  - else: "Waiting for the job's log…"
- **Reaction:**
  - Retries the initial load every 5 s.
  - Once the job is done and the log still isn't loaded, it gets 6 more tries (about 30 s). Then it shows the error "<Forge> has no log for this job." and stops.
- **Edge cases:** Vault: "GitHub REST may not serve the logs of in-progress jobs."
- **Source:** CI.swift:205-224, 745-755.

### CI-23 Live tail and following the bottom
- **Trigger:** A running job's log is loaded.
- **Reaction:** Every 2 s:
  1. `GET /actions/jobs/{id}` refreshes the job and its steps. GitHub and Gitea responses are mapped from GHJob.
  2. `GET …/logs` with `Range: bytes=<offset>-`.
- Handling the new bytes:
  - If the server ignored the Range (start 0), only the bytes past the offset are kept.
  - Bytes after the last newline are held back until the next chunk, so a split line or UTF-8 character isn't broken.
  - Formatting runs off the main thread.
  - `version` is bumped after each append.
- The text view scrolls to the end only if it was within 40 pt of the bottom, or on the first fill. Scrolling up stops the follow; scrolling back to the bottom resumes it.
- **Edge cases:**
  - There is no explicit Follow or Stop control. Leaving the page (Back, Esc, Done, backdrop) is the only way to stop.
  - Vault: tail with `bytes=<offset>-`; 416 means no new output.
- **Source:** CI.swift:205-224, 244-257, 275-300, 462-467.

### CI-24 The tail stops when the job finishes
- **Reaction:**
  - On the first poll that finds the job done: one last read, then any held-back partial line is flushed with a trailing "\n", and the loop returns.
  - A job that was already finished when opened makes three requests: the log, the job, then the range read (416).
- **Source:** CI.swift:217-218, 283-288.

### CI-25 Log rendering
- **Render:**
  - An NSTextView, not editable, selectable, inset 8, 11.5 pt monospaced font, no wrapping, horizontal scroller.
  - Per line:
    - CRLF becomes LF.
    - A leading BOM is stripped.
    - A leading ISO timestamp ("2026-10-19T14:33:38.5145900Z ") is stripped, with exactly one following space.
    - `##[endgroup]` lines are removed.
    - `##[group]` becomes "▸ title" in semibold secondary, as a link (CI-26).
    - `##[error]`: prefix stripped, red, semibold.
    - `##[warning]`: orange.
    - `##[notice]`: accent colour.
    - `##[debug]`: tertiary.
    - `[command]`: secondary.
    - A line starting with ⭐: accent semibold.
    - A line containing ✅: green.
    - Problem patterns are red: `##[error]`, ❌, a line starting with "error", "error:", "Error:", "FAILED", "panicked at", or "thread '…panicked".
- **Edge cases:**
  - ANSI escape sequences are neither stripped nor coloured; they show as raw text.
  - A lone `\r` (progress bars) is not handled.
  - Text can be selected and copied with ⌘C.
  - There is no in-log search: `usesFindBar` is not set and there is no find field.
- **Source:** CI.swift:347-403, 428-449.

### CI-26 Fold and unfold `##[group]` sections
- **Trigger:** Click a "▸ title" header line in the log (pointing-hand cursor).
- **Reaction:**
  - Toggles the group in `expanded`. The header glyph swaps between ▸ and ▾ and the body is laid out again.
  - Folded bodies get null glyphs and zero-advancement line breaks, so they take no space.
- **Edge cases:**
  - Every group starts folded, as on the web.
  - A group that opens in one chunk and closes in the next stays one group.
  - Groups loaded by "Load Full Log" get ids offset by 1,000,000.
  - Reloading after a trim (CI-29) folds everything again.
- **Source:** CI.swift:478-544.

### CI-27 Problem lines list and jump
- **Trigger:** The "<N> problem line(s) — click to jump" disclosure (red `exclamationmark.triangle.fill`). It is shown when there is at least one problem line.
- **Render:**
  - Expanded: a scroll list, max height 140, of up to 200 lines.
  - Each line is 11 pt monospaced, two lines at most; orange for `##[warning]` lines, red otherwise.
- **Reaction:** Clicking a line:
  - unfolds its group
  - `scrollRangeToVisible`
  - selects the line
  - shows the yellow find indicator
- **Edge cases:**
  - Problems come from red or orange lines, or problem patterns. Group headers are excluded.
  - At most 200 problems are found per formatted chunk, and 1000 are kept (the newest).
  - The label counts all of them; the list shows the first 200.
  - file:line is not parsed, and clicking does not jump to a file or diff. It only jumps within the log.
- **Source:** CI.swift:171, 405-418, 453-460, 756-775.

### CI-28 Truncated log and "Load Full Log"
- **Trigger:**
  - Shown when `truncatedBytes > 0`: the log is over 16 MB, or the live tail was trimmed.
  - Text: "Showing the last 16 MB of the log." with a small "Load Full Log" button.
- **Reaction:**
  - Not trimmed: `GET …/logs` with `Range: bytes=0-<truncatedBytes-1>`. The result is formatted and inserted at the top, problem offsets are shifted, and the banner disappears.
  - Trimmed: the reload in CI-29.
- **Edge cases:** A server that ignores Range has its body cut to the prefix.
- **Source:** CI.swift:259-273, 776-783.

### CI-29 Trimming a long live tail
- **Trigger:** The live log grows past 24M characters.
- **Reaction:**
  - The oldest quarter is dropped, from a line start. `truncatedBytes` increases and problems in the dropped part are removed.
  - "Load Full Log" then requests `Range: bytes=0-<offset-pending-1>` and replaces all text, unfolding nothing (`expanded` resets).
- **Source:** CI.swift:302-332.

### CI-30 Copy Log
- **Trigger:** The "Copy Log" button (only shown when a log model exists).
- **Reaction:**
  - Puts `storage.string` on the pasteboard: the formatted text, with timestamps stripped, ▸/▾ headers, and folded bodies included.
  - No toast.
- **Source:** CI.swift:723.

### CI-31 Open in the browser
- **Trigger:** "Open in <Forge>" in the detail header, or "Open Details" in the empty state (CI-39).
- **Preconditions:** The status has a `target_url`.
- **Reaction:** `NSWorkspace.open`. A relative URL (Forgejo "/acme/web/actions/runs/…") gets the account baseURL prepended.
- **Edge cases:**
  - The forge name falls back to "Forgejo" when features aren't known yet.
  - Vault: the web log routes don't accept tokens, so the browser opens them with its own login.
- **Source:** CI.swift:711, 724, 839-842.

### CI-32 Re-run Job
- **Trigger:** The "Re-run Job" button (`arrow.clockwise`) in the detail header.
  - Tooltip: "Run this job again (on Forgejo, with the jobs that depend on it)".
- **Preconditions:**
  - `rerunJob` is allowed (CI-04).
  - The job is done, and its run is done (the run status from `model.run(id:)`, or the job status if the run isn't known).
- **Reaction:**
  - There is no confirmation.
  - The button's label becomes a small spinner and all action buttons are disabled.
  - Forgejo and GitHub: `POST /repos/{o}/{r}/actions/jobs/{job}/rerun`.
  - Gitea: `POST /repos/{o}/{r}/actions/runs/{run}/jobs/{job}/rerun`.
  - On success the run's job cache is cleared and `refresh()` runs.
  - A new attempt rebuilds the detail page (CI-17).
- **Edge cases:**
  - If `model.features` is nil, `perform` returns nil without doing anything. No buttons render in that case anyway.
  - Forgejo keeps the job id and bumps the attempt.
- **Source:** CI.swift:124-133, 822-837; CIForges.swift:44-71, 207-209.

### CI-33 Re-run Failed Jobs
- **Trigger:** The "Re-run Failed Jobs" button (`arrow.clockwise.circle`).
- **Preconditions:**
  - `rerunFailed`: GitHub, or Gitea ≥ 1.26. Never on Forgejo.
  - The run is failing, or the run status (or the job status when the run isn't known) is "cancelled".
- **Reaction:** `POST /repos/{o}/{r}/actions/runs/{run}/rerun-failed-jobs`, then the same busy and refresh handling as CI-32.
- **Edge cases:** It can show while the run is still running with an earlier job failed: the run state is failing, so the button appears even though the forge may refuse.
- **Source:** CIForges.swift:57, 67.

### CI-34 Re-run All Jobs
- **Trigger:** The "Re-run All Jobs" button (`arrow.clockwise.circle`).
- **Preconditions:** `rerunRun` is allowed and the run is done.
- **Reaction:** `POST /repos/{o}/{r}/actions/runs/{run}/rerun`.
- **Source:** CIForges.swift:58, 68.

### CI-35 Cancel Run
- **Trigger:** The "Cancel Run" button (`stop.circle`).
- **Preconditions:** `cancel` is allowed (Forgejo ≥ 16, Gitea ≥ 1.27, GitHub) and the run is not done.
- **Reaction:** `POST /repos/{o}/{r}/actions/runs/{run}/cancel`. There is no confirmation.
- **Source:** CIForges.swift:59, 69.

### CI-36 Rerun and cancel errors
- **Render:** A red label with `exclamationmark.triangle.fill` under the description, selectable. It is cleared by the next successful action.
- **Reaction:** The message depends on the error:
  - GitHub 403 or 404: "GitHub refused this (<code>): the token needs write access to Actions — “Actions: Read and write” for a fine-grained token, the repo and workflow scopes for a classic one."
  - A " 403:" anywhere else: "<Forge> refused this (403): your account needs write access to this repository."
  - Anything else: the raw message ("Forgejo 409: …").
- **Source:** CIForges.swift:73-82; CI.swift:727-729.

### CI-37 Check-run output for non-Actions GitHub checks
- **Trigger:** Open the detail of a GitHub check with no linked job, whose check run has a non-empty `output.summary` or more than zero annotations.
- **Reaction:**
  - If annotations_count > 0: `GET /repos/{o}/{r}/check-runs/{id}/annotations?per_page=100`. A failure gives an empty list.
- **Render:**
  - The summary as Markdown, selectable.
  - "Annotations · N", then one row per annotation:
    - icon: `xmark.circle.fill` in red for failure, `exclamationmark.triangle.fill` in orange for warning, `info.circle` otherwise
    - "path:line" in mono secondary
    - the title in medium weight
    - the message in callout
  - Annotations are not clickable.
- **Source:** CI.swift:787-789, 814-818, 854-883; CIForges.swift:151-166, 211-213.

### CI-38 External Status and "Job Not Loaded"
- **Trigger:** The detail of a check with no linked job and no check-run output.
- **Render:** A ContentUnavailableView that fills the page:
  - check run from the github-actions app: title "Job Not Loaded", text = the notice, or "GitHub hasn't listed this job yet; it shows here once it does."
  - otherwise: title "External Status", text "Reported by a service outside <Forge> Actions, so there's no log to show here."
  - Icon: `arrow.up.right.square`.
  - Action: "Open Details" if there is a URL.
- **Edge cases:** A Forgejo status whose target_url doesn't resolve to a job (wrong index, or jobs not listed) lands here as "External Status".
- **Source:** CI.swift:790-803.

### CI-39 GitHub log redirect
- **Trigger:** A GitHub log fetch.
- **Reaction:** GitHub answers with a 302 to a signed blob URL. Authorization is removed when the redirect goes to another host.
- **Edge cases:** The blob store doesn't promise suffix ranges, so the first fetch is whole.
- **Source:** CIForges.swift:168-177; Forgejo.swift:425.

# 11. Settings and accounts

## Settings and accounts

### ST-01 Settings window and tabs
- **Trigger:** Loupe › Settings… (⌘,).
- **Render:** A 720×640 TabView with tabs "General" (gearshape), "Editor" (chevron.left.forwardslash.chevron.right), "Spaces" (square.stack.3d.up) and "Shortcuts" (keyboard).
- **Reaction:** The selected tab persists under `settingsTab`. The default is "spaces", so the first open lands on Spaces.
- **Source:** Settings.swift:22-38

### ST-02 General › Appearance
- **Trigger:** The segmented "Appearance" picker: System / Light / Dark.
- **Render:** The window and app switch appearance immediately.
- **Reaction:** Sets `appearance` to "system", "light" or "dark" (default "system") and sets `NSApp.appearance` to nil, aqua or darkAqua. It is also applied at launch.
- **Edge cases:** An unknown stored value falls back to System. Code themes follow the appearance (Editor tab).
- **Source:** Settings.swift:6-18, Settings.swift:40-55, App.swift:114

### ST-03 General › Commit Messages: tool detection
- **Trigger:** Open General.
- **Render:**
  - While scanning: a small spinner and "Looking for AI tools…".
  - None found: "No AI coding tool found. Install one of: Claude Code, Codex, Gemini CLI, GitHub Copilot CLI, Cursor CLI, opencode, Qwen Code, Amp, Factory Droid."
  - Otherwise: "Found on this Mac" with the detected names.
  - The header has "Commit Messages" and a "Look Again" button (caption), disabled while scanning.
- **Reaction:** Look Again re-runs `AIHarnesses.scan()`. See the commit AI area.
- **Source:** Settings.swift:59-128

### ST-04 General › Commit Messages: "Write with"
- **Trigger:** The picker: "Automatic (<first detected>)", each detected tool, a divider, "Off".
- **Reaction:** Sets `aiHarness`: "auto" (the default), a tool id (claude, codex, gemini, copilot, cursor, opencode, qwen, amp, droid) or "off". With Off, the Model, Style and Instructions fields hide, and ✨ fails with "No AI tool is set up for commit messages. Pick one in Settings › General."
- **Source:** Settings.swift:79-84, CommitAI.swift:145-161

### ST-05 General › Commit Messages: Model
- **Trigger:** The "Model" text field (prompt "<tool> default") and the chevron menu of models (" <tool> default", a divider, then a list).
- **Reaction:**
  - Every edit or submit saves the trimmed value to `aiModel.<harnessId>`.
  - For Claude Code the default is "claude-haiku-4-5-20251001". The old aliases haiku, sonnet and opus are shown as their full ids.
  - The menu appears only when the model list is non-empty. The list starts as the built-in one and is replaced by the tool's own listing where supported.
- **Source:** Settings.swift:85-100, Settings.swift:122-130, CommitAI.swift:149-155

### ST-06 General › Commit Messages: Style and Extra instructions
- **Trigger:** The "Style" picker: "Match the repository" (default), "Concise", "Standard", "Detailed", "Conventional Commits". The "Extra instructions" field is multi-line, 1–3 lines, with the prompt "e.g. Write in Turkish. Mention ticket numbers."
- **Reaction:** Sets `aiStyle` and `aiInstructions`. The footer shows the style summary followed by "The ✨ button next to the summary sends the change and recent commit subjects to <tool> in its read-only mode, on your own subscription."
- **Source:** Settings.swift:101-121, CommitAI.swift:64-80

### ST-07 General › Updates
- **Reaction:** UP-03 and UP-04. The section is hidden when Sparkle is unavailable.
- **Source:** Settings.swift:51, Updates.swift:53-64

### ST-08 Editor tab
- **Trigger:** The Editor tab.
- **Render:**
  - Preset picker, with "Custom" when no preset matches.
  - "Light theme" and "Dark theme" pickers.
  - "Font": "SF Mono (System)", the monospaced families, and "<family> (not installed)" when applicable.
  - "Size" stepper, 9–24 pt.
  - "Line height" slider, 1.2–2.0 in 0.05 steps, with an "N.NN×" readout.
  - A Preview section.
- **Reaction:** See the diff viewer area (EditorTheme/EditorSettings).
- **Source:** Settings.swift:29-30, EditorTheme.swift:194-240

### ST-09 Spaces tab: space list
- **Trigger:** The Spaces tab.
- **Render:**
  - A left list, 200 wide, of space names. The current space has a checkmark (help "Current space").
  - Below it: "plus" (help "New Space") and "minus" (help "Delete Space…").
  - On the right, the selected space's detail, or "No Space Selected".
- **Reaction:**
  - On appear, the current space is selected.
  - Drag rows to reorder spaces (`onMove`); the order persists and the sidebar dots follow.
  - "+" runs `addSpace()`, which also switches the main window to the new space, and selects it in the list.
- **Edge cases:** "−" is disabled with no selection or fewer than two spaces.
- **Source:** Settings.swift:135-177

### ST-10 Spaces tab: Delete space
- **Trigger:** "−" with a space selected.
- **Render:** The same dialog and message as SH-27.
- **Reaction:** `deleteSpace`, then the list selection moves to the current space.
- **Source:** Settings.swift:160-162, Settings.swift:178-182

### ST-11 Space detail: Name and Switch
- **Trigger:** The "Name" text field, and "Switch to This Space" (shown only for a non-current space).
- **Reaction:**
  - The name is bound live to `spaces[i].name`, so every keystroke persists.
  - Switch runs `switchSpace(to:)` (SH-22).
- **Edge cases:** An empty name is allowed here, unlike the sidebar Rename (see BROKEN).
- **Source:** Settings.swift:193-198

### ST-12 Space detail: Accounts list
- **Trigger:** View the accounts section.
- **Preconditions:** Accounts with `spaceID == space`. Accounts without a space belong to "default".
- **Render:**
  - Empty: "No accounts. Pull requests, checks and HTTPS push and fetch need one for the repo's server."
  - Each row: the icon `cat` (GitHub) or `server.rack`, the username (medium) and the baseURL (caption).
  - An orange "No token" (exclamationmark.triangle.fill) when tokens have loaded and this one is missing.
  - A "Remove" button (destructive).
  - Header: "Accounts" and "Add Account…". Footer: "Used for every repository in this space. Adding an account for a server that already has one replaces it."
- **Source:** Settings.swift:199-228

### ST-13 Remove account
- **Trigger:** "Remove" on an account row.
- **Reaction:**
  - Deletes the Keychain item (service `dev.extypen.loupe.forgejo`; account = baseURL for the default space, otherwise "<spaceID> <baseURL>").
  - Clears the in-memory token and removes the account from the `accounts` JSON default.
  - PR, checks and "Create Pull Request…" become unavailable for matching remotes.
- **Edge cases:** No confirmation.
- **Source:** Settings.swift:216, Model.swift:70-73, Forgejo.swift:161-188

### ST-14 Add Account sheet
- **Trigger:** "Add Account…".
- **Render:** A 460-wide form:
  - A segmented "Server" picker: "GitHub" (the default) / "Forgejo / Gitea".
  - "Server URL": default "https://github.com"; the Forgejo prompt is "https://git.example.com".
  - A SecureField "Access Token". Its prompt is "Settings → Developer settings → Personal access tokens" for GitHub, or "Settings → Applications → Generate token".
  - GitHub only: "Use GitHub CLI Token" (help "Reads the token `gh auth login` stored for this host").
  - Footer: "Give the token repository read and write access: Loupe uses it for pushes and fetches over HTTPS too."
  - A bottom status caption, then "Cancel" and "Add". Add is disabled while the URL or token is empty, or while checking.
- **Reaction:**
  - Changing Server resets the URL ("https://github.com", or empty for Forgejo), the token and the status.
  - Add: `https://` is prepended when there is no scheme and trailing slashes are stripped. The status shows "Checking…".
  - Then `GET <base>/api/v1/user` (Forgejo), or the GitHub equivalent through ForgejoClient(github:).
  - On success: an account with the returned login, `github: true` or nil, and the space (nil for default).
  - The token is saved to the Keychain, an old account with the same id is replaced, and the sheet closes.
- **Edge cases:**
  - On failure the status shows the error text and the sheet stays open.
  - An account for the same server in the same space is replaced, not duplicated.
- **Source:** Settings.swift:223, Settings.swift:255, Settings.swift:261-322, Forgejo.swift:255

### ST-15 Use GitHub CLI Token
- **Trigger:** The "Use GitHub CLI Token" button.
- **Preconditions:** `gh` at /opt/homebrew/bin, /usr/local/bin or /usr/bin, and `gh auth login` done for the host.
- **Reaction:** Runs `gh auth token --hostname <host from URL, default github.com>`.
- **Edge cases:**
  - On success the token field is filled and the status reads "Token read from gh."
  - On failure, or when gh is missing, the status reads "No gh login for <host>. Run `gh auth login` first."
- **Source:** Settings.swift:283-286, Settings.swift:324-347

### ST-16 Space detail: Repositories
- **Trigger:** The Repositories section.
- **Render:**
  - Empty: "No repositories. Open one from the sidebar while this space is shown."
  - Each row: a folder icon, the name and the ~path (middle-truncated).
  - A "Move to" menu button (only with more than one space; lists the other spaces).
  - "Remove" (help "Remove from Loupe. Nothing on disk changes.").
- **Reaction:** Move to runs `moveRepos`; Remove runs RW-03.
- **Source:** Settings.swift:229-252

### ST-17 Account matching and git authentication per space
- **Trigger:** Implicit, whenever a worktree uses its remote.
- **Reaction:**
  - A worktree's account is the one in the space holding its repo whose host matches the remote host. The port is ignored on both sides.
  - The token comes from the in-memory `tokens`.
  - `gitAuth` overrides osxkeychain for HTTPS remotes only.
  - Vault: verified with `git credential fill`.
- **Edge cases:**
  - The remote choice (prefer one with an account) is made once, when the model is created (see BROKEN).
  - If more than one account in the space shares a host, the first one wins.
- **Source:** Model.swift:195-201, Model.swift:523-554

### ST-18 Shortcuts tab: list
- **Trigger:** The Shortcuts tab.
- **Render:**
  - Grouped sections in this order:
    - General: Open Repository, Go to Worktree, Toggle Sidebar, Refresh
    - Tabs: Changes, History, Pull Requests
    - Changes: Commit, Next Conflict, Previous Conflict, "Use Ours for Conflict", "Use Theirs for Conflict"
    - Review: Next File, Previous File, "Mark Viewed & Next", "Back to List", "Compact / Full Mode", Navigate Back, Navigate Forward
  - Each row has a bordered recorder button showing the glyphs (⌃⌥⇧⌘ plus the key) or "None" (secondary), monospaced, at least 110 wide.
  - A final section holds an orange note (when present) and "Restore Defaults". Its footer reads "Click a shortcut, then press the new keys. Esc cancels, ⌫ removes the shortcut."
- **Source:** Shortcuts.swift:51-113, Shortcuts.swift:152-189

### ST-19 Record a shortcut
- **Trigger:** Click a recorder, then press keys.
- **Render:** The button reads "Type shortcut…" in accent while recording.
- **Reaction:**
  - A local keyDown monitor swallows keys until the recording ends.
  - Esc (keyCode 53) cancels.
  - A bare ⌫ (keyCode 51, no modifiers) sets "no shortcut".
  - Valid keys:
    - arrows, return, tab, space or delete with at least one modifier
    - a single non-control character with ⌘, ⌃ or ⌥ (⇧ alone is not enough); it is recorded without the shift-applied character, so ⇧⌘1 records as "1" with shift
  - Anything else beeps and keeps recording.
  - The result is saved immediately and the menus rebuild.
- **Edge cases:**
  - If another action already used the combination, it loses it and the note reads '<keys> was used by “<action>”, which now has no shortcut.'
  - Clicking a second recorder moves recording there.
  - Leaving the tab stops the monitor.
  - Conflicts with fixed system or Undo shortcuts (⌘Z, ⌘Q, ⌘W, ⌘,) are not detected.
- **Source:** Shortcuts.swift:36-48, Shortcuts.swift:132-138, Shortcuts.swift:162-167, Shortcuts.swift:192-224

### ST-20 Restore one shortcut, or all
- **Trigger:** The row's "arrow.uturn.backward" button (only when customized; help "Restore <default keys>"), or "Restore Defaults".
- **Reaction:**
  - The row button runs `set(a.standard, for: a)`; the note clears.
  - Restore Defaults empties the overrides.
  - Persistence is UserDefaults `shortcuts`: a JSON map of action rawValue to `{key, modifiers}` or null (null means no shortcut). Only differences from the defaults are stored.
- **Edge cases:** Restoring one action takes its default away from any other action that had it, silently (see BROKEN).
- **Source:** Shortcuts.swift:115-141, Shortcuts.swift:168-171, Shortcuts.swift:181

# 12. Updates

## Updates

### UP-01 Updater availability
- **Trigger:** Launch.
- **Preconditions:** The bundle path ends in ".app", Info.plist has an https `SUFeedURL` and a non-blank `SUPublicEDKey` (scripts/bundle.sh writes them, from `LOUPE_SPARKLE_PUBLIC_KEY`).
- **Render:** When unconfigured, the menu item and the Settings section are hidden, with no alert.
- **Reaction:** When configured, a `SPUStandardUpdaterController(startingUpdater: true)` starts Sparkle's scheduled checks.
- **Edge cases:**
  - Vault: CFBundleVersion is x*10000+y*100+z.
  - The feed is releases/latest/download/appcast.xml.
  - Losing the EdDSA private key means installed copies can never update again.
- **Source:** Updates.swift:6-39, App.swift:115

### UP-02 Check for Updates…
- **Trigger:** Loupe › Check for Updates…
- **Preconditions:** UP-01, and `canCheck` true.
- **Render:** Sparkle's standard UI: "You're up to date", or a release-notes dialog with Install / Skip / Remind Me Later.
- **Reaction:** `controller.checkForUpdates(nil)`. Install replaces the .app in place and relaunches (standard Sparkle behaviour).
- **Edge cases:** The item is disabled while a check is in progress (`canCheckForUpdates` is published).
- **Source:** Updates.swift:31, Updates.swift:43-50

### UP-03 Toggle "Automatically check for updates"
- **Trigger:** Settings › General › Updates.
- **Reaction:** Writes `updater.automaticallyChecksForUpdates`. Sparkle stores it in its own defaults, not under a Loupe key.
- **Edge cases:** The value is read once at init, so a change made through Sparkle's own permission prompt is not reflected while Settings is open (see BROKEN).
- **Source:** Updates.swift:16, Updates.swift:25, Updates.swift:58

### UP-04 Toggle "Automatically download and install updates"
- **Trigger:** The same section.
- **Reaction:** Writes `updater.automaticallyDownloadsUpdates`.
- **Edge cases:** Disabled while automatic checking is off.
- **Source:** Updates.swift:17, Updates.swift:26, Updates.swift:59-60

# 13. Test hooks and miscellaneous

## Test hooks and miscellaneous

### MS-01 `--api <path>` developer aid
- **Trigger:** `Loupe --api /repos/o/r/pulls/1/reviews`.
- **Preconditions:** At least one account in the `accounts` default, and its token in the Keychain.
- **Reaction:**
  - Uses the first account only.
  - Runs `GET <baseURL>/api/v1<path>` with `Authorization: token <t>`, writes the raw body to stdout and exits 0.
  - Prints "no account/token" and exits 1 when no account or token is found.
  - No window opens.
- **Edge cases:** Forgejo URL shape only, even for GitHub accounts (see BROKEN).
- **Source:** App.swift:51-59

### MS-02 `--lens <repo> <revA> <revB|worktree> [<upstream>]` developer aid
- **Trigger:** The command-line flag with at least three following arguments.
- **Reaction:**
  - Runs `git diff --name-status -z -M <revA> [<revB>]` in the repo.
  - With an upstream, prints a BranchLens line: "■ N behind T, K touching, conflicts: …".
  - Then each ChangeLens report: "■ title: summary" with sections and facts. Glyphs: + added, − removed, ~ changed, → renamed, · info, ! warning; at most 40 facts per section, then "… N more".
  - Ends with "(N ms)" and exits 0.
- **Edge cases:** An upstream argument starting with "-" is ignored. See the lenses area.
- **Source:** App.swift:10-50

### MS-03 LOUPE_STRESS=<scenario.json>: stress mode bootstrap
- **Trigger:** The environment variable on a test copy (LoupeMemTest.app, bundle id `dev.extypen.loupe.memtest`, per the vault).
- **Preconditions:** A scenario JSON with: forge, token, user, repos[], big, conflict, submodule, stashes, bigFileWorktree, bigFile, minifiedFile, hugeCommit, hugePR, prs[], iterations, pause, logSeconds, background, steps[]. A mock forge is in scripts/memtest/mockforge.py; repos come from mkrepos.py; the copy from mkcopy.sh.
- **Reaction:**
  - Refuses to run under the empty or real id `dev.extypen.loupe`: stderr "LOUPE_STRESS refused: …", exit 2. An unreadable scenario gives "LOUPE_STRESS: can't read …", exit 2.
  - Wipes its own defaults domain and sets: `spaces` = [Default with the repos, selection = big]; `tab` = changes; `prFullMode` = false; `diffSide` = diff; `lfsMissingBannerDismissed` = true.
  - Creates an in-memory account (forge, user) with the token. It is never written to the Keychain, and the `accounts` default is removed.
  - Starts the run.
- **Source:** StressDriver.swift:1-65

### MS-04 Stress run: output protocol
- **Trigger:** The run.
- **Reaction:**
  - Waits up to 60 s for every repo's worktrees, then pauses 3×.
  - Sets the window to the screen's visible frame and prints "window WxH".
  - With `background`, orders the windows back.
  - Prints "ready".
  - For each iteration and step: "step-begin iter= step=" and "step-end … secs=".
  - Between iterations it returns to big/changes, pauses 2×, and prints "iter iter=N". At the end, "finished".
  - Every line is "LOUPE_STRESS <event> t=<secs>" on stdout. A timed-out wait prints "timeout <what_with_underscores>"; an unknown step prints "unknown-step X".
- **Source:** StressDriver.swift:87-139, StressDriver.swift:158

### MS-05 Stress steps: worktrees, tabs, tabsonly, idle
- **Reaction:**
  - `worktrees`: selects every worktree of every repo with 0.5× pauses, then opens ⌘K for 2× and closes it.
  - `tabs`: for big and conflict, cycles changes → history → pulls → changes.
  - `tabsonly`: on big, writes tab history, then pulls, then changes, 1× each.
  - `idle`: pauses 30× with nothing driven.
- **Source:** StressDriver.swift:141-179

### MS-06 Stress steps: history, bigfile
- **Reaction:**
  - `history`: opens big's History and selects the hugeCommit (waits for at least 5000 files), steps through files, toggles commitOverview, prFullMode and the diffSide values old/new/diff, then scope All and back to Current.
  - `bigfile`: selects bigFile and minifiedFile unstaged in bigFileWorktree, cycles every diffSide and Full mode, then presses the view switch to Overview and back. It prints "missing <path>" or "no-view-switch changes" when those are not found.
- **Source:** StressDriver.swift:181-219

### MS-07 Stress steps: prs, hugepr, mergepr
- **Reaction:**
  - Each one first opens big's pulls and waits up to 90 s for at least 100 PRs.
  - `prs`: opens each PR in `prs` until its files load.
  - `hugepr`: waits for at least 5000 files and comments, samples files, uses Full mode until the tree builds, then the view switch: Overview, then Conversation, then Files.
  - `mergepr`: merges the first open PR in `prs` with a merge commit, then runs `showMerged`. Prints "merged pr= ok= secs= tab= scope= top= error=", or "merge-no-open-pr" / "merge-failed".
- **Source:** StressDriver.swift:221-281

### MS-08 Stress steps: checks, merge, create
- **Reaction:**
  - `checks`: opens the checks overlay of the first PR and clicks the "job-01" row with synthetic mouse events sent to the window only. It watches the log for logSeconds, or prints "no-check-row".
  - `merge`: selects 20 conflicted files in the conflict repo.
  - `create`: opens the PullCreateSheet for big's second worktree for 3×.
- **Source:** StressDriver.swift:283-319, StressDriver.swift:381-402

### MS-09 Stress steps: rebase, stashes, submodule
- **Reaction:**
  - `rebase`: opens the rebase planner from big's commit #25 for 2.5×, then closes it.
  - `stashes`: uses the stashes repo in History scope Stashes (at least 200) and selects every 20th.
  - `submodule`: selects each file in the submodule repo.
- **Source:** StressDriver.swift:321-347

### MS-10 LOUPE_STRESS_LOGSTACKS and LOUPE_STRESS_DUMP
- **Trigger:** Environment variables, in stress mode.
- **Reaction:**
  - `LOGSTACKS`: hooks NSLog (`_NSSetLogCStringFunction`) and echoes every line to stderr. Lines containing "reentrant" are followed by "--- stack … ---". Prints "LOUPE_STRESS no NSLog hook" if the symbol is missing.
  - `DUMP`: when a control is not found, prints the window's view tree (with segment labels) and its accessibility tree to stderr.
- **Source:** StressDriver.swift:55, StressDriver.swift:67-79, StressDriver.swift:426-446

### MS-11 LOUPE_TRACE: git timing log
- **Trigger:** The `LOUPE_TRACE` environment variable is set.
- **Reaction:** Each git run prints "[git <total>ms queued+run] <first three args> (<run>ms run)" to stderr.
- **Source:** Git.swift:19, Git.swift:69

### MS-12 Stress view-switch press helper
- **Reaction:** Finds the rightmost NSSegmentedControl in the toolbar whose segments are all unlabeled, selects segment N and sends its action. Prints "view-switch segment= of=". Testers can use the same method to drive SH-35.
- **Source:** StressDriver.swift:351-365

### MS-13 Git process environment, visible to users
- **Reaction:**
  - git binary: /opt/homebrew/bin/git, then /usr/local/bin/git, then /usr/bin/git.
  - Env: `GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`, `LC_ALL=C`; PATH gets /opt/homebrew/bin and /usr/local/bin appended, so LFS and hook tools are found when launched from Finder.
  - Read-only subcommands (show, diff, log, grep, ls-files, ls-tree, cat-file, rev-list, blame, diff-tree, merge-base, for-each-ref, status, rev-parse, show-ref) are killed when their task is cancelled. Writes always finish.
  - Stdin is fed asynchronously, and stderr is drained concurrently with a 2 s cap after exit.
- **Edge cases:** Error text is stderr trimmed, or "git <sub> failed (<code>)".
- **Source:** Git.swift:18-136

### MS-14 File-type icons
- **Trigger:** Any file row in the Changes, History or PR lists.
- **Render:** Material Icon Theme SVGs, 16 pt by default, cached per name. A missing SVG falls back to the SF Symbol `doc`.
- **Reaction:** Lookup order:
  1. Exact lowercased file name, for example cargo.toml→rust, flake.nix→nix, dockerfile→docker, package.json→nodejs, readme.md→readme, .gitignore→git, go.mod→go, justfile→makefile.
  2. `.env*` → tune, `dockerfile*` → docker.
  3. `*.test.*` or `*.spec.*` with ts→test-ts, tsx/jsx→test-jsx, js→test-js.
  4. yaml or yml under /k8s/ or /kubernetes/ → kubernetes.
  5. By extension (about 110 extensions), otherwise "file".
- **Source:** FileIcons.swift:5-67

### MS-15 Change-kind status icon
- **Trigger:** Trailing marker on file rows.
- **Render (13 pt, medium):**

| Kind | Symbol | Color | Tooltip |
|---|---|---|---|
| added | plus.square | green | Added |
| untracked | plus.square | green | Untracked |
| deleted | minus.square | red | Deleted |
| renamed | arrow.right.square | blue | Renamed |
| copied | arrow.right.square | blue | Copied |
| conflicted | exclamationmark.square | orange | Conflicted |
| type changed | dot.square | yellow | Type changed |
| modified | dot.square | yellow | Modified |

- **Source:** FileIcons.swift:69-108

### MS-16 Status parsing a tester can observe
- **Reaction:**
  - From porcelain v2, each path yields a staged entry (X ≠ ".") and an unstaged entry (Y ≠ ".").
  - "u" entries become conflicted; "?" entries become untracked.
  - A "(detached)" head makes branch nil, so the row reads "detached @ sha" and the title reads "detached HEAD".
  - `ahead` and `behind` come from "# branch.ab".
  - Submodule entries carry SubmoduleStatus.
  - An untracked diff uses `--no-index /dev/null <file>`, which exits 1 and is allowed.
- **Source:** Git.swift:221-277

---

# 14. Known issues in the Swift code (expected divergences)

These were found while tracing. A GPUI port can fix them, but then the side-by-side comparison will differ, so record any fix here when it lands.

## App shell, worktrees, menus, settings, updates

- App.swift:154 and Model.swift:150 (`selection = url.path`): opening a subfolder of a repo (repo/src) adds the repo but selects "repo/src". That is not a worktree path: no sidebar row highlights, and a WorktreeModel and FSWatcher are built for the subfolder.
- Model.swift:154-160 (`removeRepo`): the selection is never cleared. Removing the repo of the selected worktree leaves the detail on a forgotten path, and `app.model(for:)` rebuilds a model and watcher for it. This is the same class of bug the 0.9.1 fix addressed in `refreshWorktrees` (Model.swift:172-174).
- Git.swift:146, 175 (`isPrunable` parsed, never used): worktrees deleted with `rm -rf` stay as rows until "Prune Stale Worktrees". FSWatcher's worktreesChanged does not fire for this, because `.git/worktrees/<name>` remains. Selecting the row runs git in a missing folder.
- App.swift:693-695 (AddWorktreeSheet): picking an existing "origin/x" while a local "x" exists runs `worktree add -b x` and fails with "a branch named 'x' already exists". It should check out local x.
- App.swift:683: `replacingOccurrences(of: "origin-", with: "")` removes "origin-" anywhere in the slug ("fix-origin-check" becomes "fix-check"). Only the "origin" remote is handled ("upstream/x" becomes "upstream-x").
- App.swift:264 and Model.swift:189: "Force Remove" passes one `--force`. Git needs `-f -f` for a locked worktree, so Force Remove of a locked worktree always fails.
- Settings.swift:194 versus App.swift:269-270: the Settings name field allows an empty or whitespace name, while the sidebar Rename rejects it. An empty-named space then shows an empty title and an empty dot tooltip.
- Model.swift:89: the new space name "Space \(count+1)" can duplicate an existing name.
- Settings.swift:157: "+" in Settings › Spaces calls `addSpace()`, which also switches the main window's current space. That may be unexpected from Settings.
- Shortcuts.swift:169: the per-row Restore calls `set(standard)`, which silently strips that combination from any other action without setting the orange note. Restore Defaults is consistent.
- Shortcuts.swift:133-138 and Undo.swift:367-370: shortcut conflict detection covers only ShortcutAction. Assigning ⌘Z, ⇧⌘Z, ⌘Q, ⌘W, ⌘, or ⌘H to an action silently collides with fixed menu items.
- App.swift:94-101: Review menu items are never disabled, including when no diff or PR is open. Compact / Full flips `prFullMode` invisibly when no diff shows. ⇧⌘V does nothing outside an open PR.
- App.swift:88-90: the tab menu items have no disabled state and no checkmark for the current tab.
- Model.swift:285, 523-535: `detectRemote` runs once per model. Adding an account later does not re-pick a remote in multi-remote repos (the "prefer one with an account" choice is stale until relaunch or a model rebuild).
- Updates.swift:25-26: `automaticallyChecks` and `automaticallyDownloads` are copied once at init, not observed. Changes made through Sparkle's own first-run permission prompt or elsewhere are not reflected in the Settings toggles.
- App.swift:54 (`--api`): always builds `<baseURL>/api/v1<path>` with a "token" header, so it is wrong for GitHub accounts. It also uses only the first account, whatever its space.
- App.swift:343: errors from Prune Stale Worktrees are swallowed (`try?`), with no feedback.
- Model.swift:110: when moving the selected repo out of the current space, `selection = space.repos.first`. That is a repo path (correct for the main worktree), or nil when the space is now empty.
- StressDriver.swift:406-424: `press(containing:)` is never called. Dead code.
- Model.swift:135: `cachedModelCount` exists only for tests (by its comment).
- SH-14: there is no drop target after the last repo header, so a repo can only be placed "before" another.

## Changes, commit, stash, merge editor

- **Model.swift:448:** The untracked branch of `apply()` is unreachable. ChangesView.swift:516 gives untracked files `hunkActions = []`, and Full mode passes none.
- **Conflicted files can be staged with markers still in them.** Space (ChangesView.swift:361-368), context "Stage" (ChangesView.swift:419) and the Full-mode "Stage File" (ChangesView.swift:782) all run `git add -A` on a conflicted path. That marks it resolved with the markers still in the file, bypassing the marker check behind Mark Resolved (MergeEditor.swift:767).
- **Discard is offered for conflicted files.** The context menu (ChangesView.swift:421), Delete (ChangesView.swift:369-372) and the Full-mode "Discard…" (ChangesView.swift:781) all do this. `git restore --worktree` on an unmerged path fails, which shows the "Git" alert.
- **Full mode never opens the merge editor or offers hunk/line staging.** ChangesView.swift:266 is checked before line 267, and ChangesView.swift:793 passes no `hunkActions`.
- **Stash push records no undo** (Model.swift:482-489), unlike apply, pop and drop. ⌘Z after a stash undoes an older entry instead.
- **Several conflict actions record no undo:** resolve, whole-file Use Ours/Theirs, Delete File, submodule update-index, Reset… (checkout -m) and Abort (Model.swift:377-386, MergeEditor.swift:744-751, 944, 953, Model.swift:368). The vault's "known gap" notes this.
- **Dead code in Conflicts.swift:** `ConflictText.resolving` (lines 66-83) and `numbered` (lines 22-24, 57-61) are used only by tests (ParserTests.swift:316-318, 385). The comment at lines 22-23, "the conflict pane's body reads it", is stale. `Array[safe:]` (lines 125-127) is unused in Sources.
- **Merge editor adds base sections git did not write.** `fromChunks` always attaches a base (MergeEditor.swift:467), so `markerBlock` always emits a "||||||| base" section (line 495). The result therefore shows diff3-style base sections even when git wrote merge-style markers. The first autosave rewrites unresolved conflicts with those sections, which contradicts the comment at line 123 ("a write changes only what the user resolved"). The same cause makes "Use Base" always enabled for chunk-built conflicts, including both-added files with an empty base.
- **Duplicate buttons on special conflicts.** For binary and symlink conflicts the header's Use Ours/Use Theirs (MergeEditor.swift:740-743) duplicate the body buttons (lines 956-957). For delete/modify they sit next to Keep/Delete File, and Reset… is offered where `checkout -m` may fail.
- **Status colors disagree.** `FileChange.Kind.color` (ChangesView.swift:480-490: modified blue, renamed purple) does not match StatusIcon (FileIcons.swift:87-95: modified yellow, renamed blue). Kind.color is used in ChangeOverview and the tab bar.
- **Commit can run twice.** The Commit button and ⌘↩ are not disabled while `busy` (ChangesView.swift:691), so a double press makes a second commit that fails.
- **AI generation cannot be cancelled** and overwrites text typed meanwhile (Model.swift:495).
- **AI amend on a root commit describes the wrong change.** With no HEAD~1, CommitAI.swift:192-194 falls back to the staged diff or the worktree diff against HEAD, not the commit being amended.
- **BranchPicker: Return with an empty filter switches branch** to the most recent non-current one (ChangesView.swift:148).
- **An empty AI merge suggestion deletes the conflict.** The popover shows "(nothing)", and "Use Suggestion" resolves the conflict to nothing (MergeEditor.swift:447, 455, 867).
- **Mark Resolved can be blocked by legitimate content.** `hasMarkers` treats any line of seven `<` or `>` followed by whitespace as a marker (MergeEditor.swift:500-504).
- **Inconsistent casing for cherry-pick.** The Abort label "Abort Cherry-Pick" comes from `"cherry-pick".capitalized` (Conflicts.swift:111), while the verb is "Cherry-picking" and the undo label "Cherry-pick".
- **No Abort or Continue for conflicts without an operation.** A stash apply or pop that conflicts shows a "Conflicts" banner with no buttons (Conflicts.swift:110). The user can only resolve.
- **⌘A is not swallowed in the status list.** FileListView lacks the `.onCommand(selectAll)` that FileTreeList has (FileTree.swift:125). The vault (0.6.1) notes that List hangs on ⌘A over large trees.
- **A Return at the start of the Description is eaten** (ChangesView.swift:708).
- **Long argument lists.** stage, unstage and discard pass every path as argv (Model.swift:421, 429-430, 438). FileSnapshot uses stdin on purpose to avoid overflowing argv (Undo.swift:295), so Stage All on thousands of files could hit ARG_MAX.
- **Two confirmation styles for discard.** Hunk and line discard use a blocking NSAlert (ChangesView.swift:519-524); file discard uses a SwiftUI confirmationDialog.
- **Reset… loads twice.** It calls `session.load()` explicitly, and the status refresh's `reloadIfChangedOutside` reloads as well (MergeEditor.swift:749, 696).
- **Blame silently does nothing for deleted files.** The Blame/History buttons are shown (ChangesView.swift:503), but blame of the missing worktree file fails silently (FileInspector.swift:110).
- **Vault corrections (index.md):**
  - index.md:136 says line-level staging and a conflict UI are not built. Both exist.
  - index.md:164 describes a card ConflictPane with "Use Current / Incoming / Both". It is now the three-way MergeEditor.
  - index.md:165-166 describe a separate File window and "Blame Before This Commit". That window is gone (index.md:220).
  - index.md:168 says `--model haiku` and a toggle. It is now the full id `claude-haiku-4-5-20251001` and a choice of nine tools.

## Diff viewer, overview, LFS, submodules

- Truncated copy: Theme.maxDrawnChars says "copy still gets full text" (DiffTableView.swift:935), but `selectedText` uses `text(row:)`, which goes through `Syntax.displayText` (DiffTableView.swift:570-571, 732-742). Copy truncates at 1500 characters and turns tabs into 4 spaces, as the ponytail comment at line 738 admits. Find works on the same truncated text, so a match past column 1500 is never found.
- Blank Changes pane: Changes compact DiffPane has no `lines.isEmpty` state (ChangesView.swift:512-533). A pure rename, chmod-only change or empty new file shows a blank table, while History and PR show "No Content Changes / Mode change or rename only." (History.swift:577, PullRequestsView.swift:536) and the Full tab shows "Empty File" (ChangesView.swift:790).
- Dead ruler parameter: DiffPane passes `changeRuler: fullMode` (ChangesView.swift:531), but DiffPane is only shown when `!fullMode` (ChangesView.swift:266), so the value is always false.
- Stage/Discard in Before/After: in Changes, Before and After keep `hunkActions` and the context-menu Stage/Discard Line and Hunk items (DiffTableView.swift:697-714). The diff is the whole file there (one hunk via wholeFile, Model.swift:395-399), so "Stage Hunk" or "Discard Hunk…" acts on every change in the file while no hunk headers are visible.
- Overview ignores the line in Changes: Changes' onOpen discards it (`{ path, _ in` at ChangesView.swift:261), while History and PR reveal it (History.swift:554, PullModel.swift:421-425). Clicking a path that isn't in the list only closes the Overview.
- LFS pointers not recognized in two hosts: the PR Full-mode file (PullRequestsView.swift:565-576) and the File-History picked commit diff (FileInspector.swift:130-135) never check `LFSDiff`, so raw pointer text shows there. Every other host shows LFSObjectView.
- Unmodified n/p/v keys: DiffTable.keyDown matches `n`, `p` and `v` without checking modifiers (DiffTableView.swift:875-879), so ⌃n, ⌥p and so on also fire. `v` posts `.markViewed` from any host; only PRs listen.
- Settings find bar: the ⌘F/⌘G monitor (DiffTableView.swift:474-489) is installed by every DiffTableView, including the Settings preview (EditorTheme.swift:278). ⌘F in the Settings window opens a find bar on the preview diff.
- Comment button mismatch: Add Comment enables on `trimmingCharacters(in: .whitespaces)` but submits `.whitespacesAndNewlines` (DiffTableView.swift:1249-1252). A body made only of newlines enables the button, then the popover closes and nothing is added.
- Inaccurate n/p comment: `jumpHunk` says runs inside a hunk are stops only "in a whole-file view" (DiffTableView.swift:744-745), but `starts()` makes every change run a stop in all views (lines 754-758).
- Vault is out of date: index.md:56 says "unified/split layouts" (split was removed, index.md:222), and index.md:316 mentions "tree-sitter passes", but Syntax.swift is a hand-written tokenizer (Syntax.swift:3-5).
- No similarity shown: `FileDiff.similarity` and `copiedFrom` are parsed (Diff.swift:146-148) but never shown in the diff header; only the Code lens uses them (Lenses.swift:495).
- Hover cursor on exit: `updateHover` sets the iBeam cursor on mouseExited (DiffTableView.swift:226-227, 917-920), even though the pointer has left the table. Minor; the next view resets it.
- Stale selection on rebuild: the selection is stored as row indices and kept across same-path rebuilds (DiffTableView.swift:106). After a Diff↔Before/After switch the highlighted span can land on different lines.

## Lenses, code browser, quick switcher, file tree

1. **QuickSwitcher lists only the current space.** It uses `app.repoPaths` (QuickSwitcher.swift:25), which is `space.repos` (Model.swift:20-23). Yet `selection.didSet` (Model.swift:31-33) says "Jumping to a worktree in another space (⌘K…)". Worktrees of other spaces can't be reached with ⌘K.
2. **Clean worktree re-folds the Full-mode tree on every refresh.** `collapseApplied = !changed.isEmpty` (CodeBrowser.swift:140). With no changes it stays false, so each `loadTree` (key = statusVersion, which goes up on every refresh, Model.swift:299) resets `collapsed` to every directory and undoes what the user expanded.
3. **Empty repo or empty worktree spins forever in the Full-mode tree.** `WorktreeTree` shows a ProgressView whenever `tree.isEmpty` (ChangesView.swift:729), and a failed `ls-files` leaves it empty too (CodeBrowser.swift:125).
4. **Changes Overview drops the line.** A fact with a line opens only the file, `{ path, _ in` (ChangesView.swift:261-263). History and PR reveal the line. A fact whose path is not a changed file (e.g. a column defined in an older, unchanged migration) only closes the Overview.
5. **Changes Overview recomputes and flashes "Reading the change…" on every status refresh.** The key includes `statusVersion` (ChangesView.swift:261), and the body shows the spinner while `computedFor != key` (ChangeOverview.swift:19). Full-mode open files also reload and flash a spinner each refresh (CodeBrowser.swift:145, ChangesView.swift:802).
6. **Tree and tab clicks bypass history and keep a stale reveal (PLAUSIBLE).** Only `open()` records Back; tree and tab clicks set `selectedPath` directly. `reveal` keeps the last jump's line from another file, so Back/Forward store `reveal?.line` of a different file (CodeBrowser.swift:30-46). A rebuilt DiffTableView coordinator could reveal that stale line in the newly selected file (ChangesView.swift:794, DiffTableView.swift:126-130).
7. **Back/Forward are missing in Compact mode, though ⌘-click works there.** They exist only inside `FileNavControls`, which is in Full-mode headers only (CodeBrowser.swift:245-248). ⌃- does nothing in Compact.
8. **⌘-click switches every view to Full mode.** In Changes Compact, a jump to an unchanged file forces `fullMode = true` (ChangesView.swift:298). That is the global `prFullMode`, so PR and History switch to Full as well.
9. **"2000+ references" is shown for exactly 2000 matches** (CodeBrowser.swift:285, the cap at CodeIndex.swift:234).
10. **Reference rows misparse paths that contain ":"** (`split(":", maxSplits: 2)`, CodeIndex.swift:231).
11. **Go-to-definition is offered in every file type.** The ⌘-underline and the context-menu items appear even in YAML, SQL and Markdown, where no definition can ever be found and it falls back to references (DiffTableView.swift:222, 692).
12. **The Code lens "Show N More" link reads oddly.** The noun is the section title, e.g. "Show 3 More Imports across files" or "Show 2 More Copied" (ChangeOverview.swift:1022).
13. **New/deleted config files lose their key count in the UI.** ConfigRow shows the detail only when `tag == nil`, so "new file · N keys" renders as just the path with a "new file" tag (ChangeOverview.swift:1002).
14. **Branch-row commit rows are not clickable**, unlike every other row in the Overview (ChangeOverview.swift:1140-1151).
15. **A filter makes folder clicks dead.** While the PR filter is on, the collapse binding is `.constant([])`, so clicking a folder, Expand All and Collapse All do nothing and give no feedback (PullRequestsView.swift:472).
16. **There is no keyboard way to fold tree folders.** Folders can't be selected (FileTree.swift:108).
17. **Dead parser paths:**
    - `SQL.Trigger` is parsed for `CREATE CONSTRAINT TRIGGER`, but `risks()` ignores triggers. That is harmless.
    - `CodeSymbol` reference kinds (call, implementation, type, class) are computed but never used: `definitions` keeps only definitions, and references use `git grep`.
    - The Go tags query's captures that have no definition/reference tag are dropped (CodeIndex.swift:108-110).
18. **QuickSwitcher creates a WorktreeModel for every row** (`app.model(for:)` at QuickSwitcher.swift:44), for every worktree of the space, each time the sheet renders.

## History, rebase, undo

1. History.swift:330 against History.swift:167 and Model.swift:637-640. `releaseHeavyState` clears the History commit's files when you switch worktrees. Coming back, `.task` finds the same key with commits non-empty and returns early, so the `reload()` branch meant to re-select the released detail (line 167) never runs. Expected result, still to be confirmed in the app: the header shows "0 files" and the tree shows "No File Changes" until you re-select a commit or press ⌘R.
2. History.swift:187-199 and 291-293. `fetch` reads the live `search` text. Typing without pressing Return and then scrolling to the bottom (`loadMore`) mixes an unfiltered first page with filtered later pages at a wrong `--skip`. `setScope` and ⌘R also apply unsubmitted text.
3. InteractiveRebase.swift:179-195. `Rewrites.lastRewrite` and the `.historyRewritten` notification have no consumers anywhere, so this is dead code. `Rewrites.record` even runs before the rebase, including for rebases that fail.
4. History.swift:640-646. Stash entries are merge commits, so their header shows "merge of" with parent links that can never resolve. The tooltip "scroll the list to load more" is misleading because the Stashes scope is always exhausted.
5. InteractiveRebase.swift:425-431. The rebase menu items are not gated by `rebaser.local`. They are enabled for commits below a merge or for the root, and only fail later with the merge error. Drag-and-drop does use `local`, so the two paths are inconsistent.
6. Undo.swift:343-345. The "Revert Commit" offered by the Can't Undo alert runs `git revert` outside `undo.run`, so no undo entry is recorded. A revert from History records one.
7. InteractiveRebase.swift:532-536. The planner submits with `confirmed: true`, skipping the drop confirmation; its footer warns only about pushed commits. Multi-drops never get confirmed.
8. InteractiveRebase.swift:172-173. When a rebase stops and is then continued in Changes, the `$TMPDIR/loupe-rebase-*` directory is never removed (a small leak).
9. InteractiveRebase.swift:487-488. In the Reword sheet, the default-action Return is swallowed by the TextEditor (inserts a newline). Only a click submits.
10. History.swift:395 against History.swift:382. Label inconsistency: the context menu says "Revert Commit", the header says "Revert".
11. FileInspector.swift:195 against History.swift:572-574. The File History panel opened from a commit's diff ignores `FileTarget.rev` and logs from HEAD. Blame honors `rev`.
12. History.swift:313. During the first load the list is empty and `loading` is true, so the list is blank and there is no spinner.
13. The vault is stale. The vault says "300 commits per page"; the code uses 100 (History.swift:154). The vault also says "Not built yet: … a commit graph", which is outdated: the graph exists (History.swift:80-131).

## Pull Requests

- Conversation.swift:119 with PullModel.swift:515-518: the comment text is cleared even when posting fails (postComment swallows the error), so the typed comment is lost.
- PullRequestsView.swift:96-98: `loadMore` hangs off `onAppear` of rows in the "All" section only, and only for `list.pulls.last`. If the last loaded PR is in This Branch or Other Worktrees, or is hidden by search or "Opened by me", closed/all paging never triggers and the list is stuck at 50.
- PullRequestsView.swift:114-118 and PullModel.swift:50-56: on a state switch, `pulls` isn't cleared. A failed load leaves the previous state's PRs under the new picker value, and the error is hidden because the list isn't empty.
- PullRequestsView.swift:106: any list error is invisible once PRs are on screen, so a dead token or offline state goes unnoticed apart from the "Updated" time aging.
- PullRequestsView.swift:307: the detail alert is titled "Forgejo" for GitHub accounts too.
- PullRequestsView.swift:295-300 with 307: offline or with an expired token, every 60 s poll raises the modal alert again.
- Model.swift:559: `checkoutInWorktree` looks for a `pr/<N>` branch across every repo's worktrees, so PR #5 of repo B can select repo A's `pr/5` worktree.
- PullModel.swift:497-500 vs DiffTableView.swift:265-288: comments whose line isn't in the shown diff (outdated, or GitHub `line: null`, which maps to 0/0) are dropped from the diff but still counted in the tree badge. They can be reached only from the Conversation tab.
- PullRequestsView.swift:398 vs Conversation.swift:295 and 247: "requested changes" is red in the header badge but orange in the conversation and sidebar.
- PullRequestsView.swift:676: the empty-body check isn't trimmed, so a body of only spaces lets Comment or Request Changes submit with no drafts. GitHub REQUEST_CHANGES with an empty body and only drafts likely gets 422 (PLAUSIBLE).
- PullModel.swift:151 and 508: drafts keep their old line numbers after a new push and are submitted with the new `head.sha` as commit_id. There is no staleness warning, so a comment may land on the wrong line or fail.
- PullRequestsView.swift:455-458 (PLAUSIBLE): `pull.fullMode` follows `prFullMode` only through a UserDefaults observer in PullFilesPane, which is mounted only on the Files tab. A ⌥⌘F press on Overview or Conversation, and cached models of other PRs, stay out of date until the next UserDefaults write. The toolbar picker then shows a mode the pane isn't in.
- PullModel.swift:423: revealing an unchanged file from Overview or Conversation flips the global `prFullMode`, which also changes the Changes tab's mode.
- PullModel.swift:377 with PullRequestsView.swift:578: if `loadFull` fails, the file pane spins forever (it shows the alert once).
- CI.swift:718 and PullRequestsView.swift:328: both use `.shortcut(.back)` (⌘[). With the checks overlay open over a PR detail, ⌘[ is ambiguous.
- PullRequestsView.swift:597: outside Full mode, ⌘W closes the window and quits the app (App.swift:138). The tab-close behaviour applies only in Full mode on Files.
- PullRequestsView.swift:63-65: if `pullsMine` is saved as true and `me` is nil, the filter hides everything while the toggle is disabled, so it can't be turned off. This is nearly unreachable because the account always exists here.
- Conversation.swift:162: no `created_at` means no description card at all.
- PullRequestsView.swift:357: GitHub's list and early detail data have `mergeable: null`, so Merge is enabled until GitHub computes it. Draft PRs aren't blocked by the client.
- PullModel.swift:47-48: the open list fetches all pages only while a page holds exactly 50. A server capped lower (Forgejo MAX_RESPONSE_ITEMS < 50) silently truncates it to one page.
- App.swift:54: `--api` always uses `/api/v1`, so it is wrong for GitHub accounts.
- Gaps, not bugs: no reply, resolve, edit or delete of server comments, no suggestions, no close or reopen, no delete-branch on merge, no merge message, and viewed state is never synced to the forge. Polling is 60 s; 30 s is only the freshness threshold.
- Stale vault items (code wins): index.md "A draft added by double-click" (double-click now selects a word; comments come from the context menu "Add Comment…"). index.md "a directory has a viewed checkbox" (it is now a context-menu item plus a done/total counter). pr-full-mode-analysis.md "Compact/Full picker in the tree pane" (it is now in the window toolbar).

## CI and PR create

- CI.swift:628: the overlay header icon uses `.symbolEffect(.pulse, isActive: busy && s.failing == 0)`. It pulses for as long as anything is running or queued with the overlay open. That is the same endless animation removed from the badge for energy reasons (CI.swift:562, f310e64). With CI.swift:639 (a 1 Hz TimelineView for "Updated …") it keeps redrawing the window. This doesn't match the "static running dot" rule.
- CI.swift:625-631: with zero checks the header shows the green `checkmark.seal.fill` and "All checks passed", "0 of 0 successful", which is wrong. The same happens during the first load.
- CI.swift:9, 585, 607: `Check.id = status.context`. Duplicate contexts (a GitHub status and a check run with the same name, matrix jobs with the same name, re-reported statuses) collide in `ForEach` ids, and `openCheck` always opens the first match.
- CI.swift:778: the banner always says "Showing the last 16 MB of the log", even after a live-tail trim (CI.swift:320-331), where the dropped amount is unrelated to 16 MB.
- CI.swift:718 with PullRequestsView.swift:328: the overlay's "Checks" back button and the PR detail's back button both bind ⌘[ while the overlay is on top of the PR detail. Which one fires is not defined, so ⌘[ may close the PR behind the overlay. (Plausible, not confirmed.)
- CI.swift:75-101: `refresh()` has no in-flight guard. While the overlay is open, the badge `.task`, every PR reload (PullModel.swift:247), the 5 s overlay poll and the manual refresh can run overlapping refreshes, each with a full set of status, runs and jobs requests.
- CI.swift:91-94: one failing jobs request fails the whole refresh. Statuses that loaded fine are then thrown away, and the old list stays.
- CI.swift:161-165: with no checks, `allDone` is false, so the overlay polls every 5 s forever.
- CIForges.swift:57: "Re-run Failed Jobs" can appear while the run is still running, because the run state is failing after an early job failed. The forge will refuse it, and the comment at CIForges.swift:50-51 says reruns need a finished run.
- CI.swift:366-403: no ANSI escape handling and no lone-CR handling, so coloured CI output shows raw escape codes.
- CI.swift:428-449: there is no find or search in the log text view (`usesFindBar` is not set, unlike MergeEditor.swift:642), and there is no file:line parsing. Problem lines and GitHub annotations never jump to a file or diff.
- CI.swift:125-126: `perform` returns nil (silent success) when `features` is nil. That can't be reached today because buttons need `features`, but it is a silent no-op.
- Forgejo.swift:208 and PullCreate.swift:36: Gitea servers are labelled "Forgejo" in error text.
- PullRequestsView.swift:67: the "Create Pull Request…" toolbar button only checks the loaded list's open PRs. With the state filter on Closed or All, past the first page, it shows even though an open PR exists. It also ignores `isMerged`, unlike the banner (PullCreate.swift:420).
- App.swift:371-374: the sidebar menu item doesn't check whether the branch is ahead or already has a PR, so the sheet can open with nothing to create.
- PullCreate.swift:243: AI writing always overwrites a body the user edited (the title only when the reply has one).
- PullCreate.swift:207-209: a default branch from the API that has no fetched remote ref is preselected. `git log origin/<def>..HEAD` then fails silently, giving "No commits…" and Create disabled, with no hint to fetch.
- PullCreate.swift:255-257: the push inside create doesn't post `.branchPushed`. Harmless, because the banner re-checks when the sheet closes. It also doesn't bump `refsVersion` the way `remoteOp` does (Model.swift:507), so the PR tab's `canCreate` cache key doesn't change after this push.
- PullCreate.swift:260: the head is sent without "owner:", so PRs from forks are not supported.
- PullCreate.swift:363: Cancel doesn't cancel an in-flight push or create. A later success still navigates.
- PullCreate.swift:403: dismissing the banner is permanent, with no UI to bring it back.
- The vault is stale: index.md:136 lists "CI re-run" as "Not built yet", but rerun and cancel exist (CIForges.swift:44-82, CI.swift:822-837). index.md:121 says GitHub has "no log tailing", but JobLogModel does poll GitHub logs, which are just unavailable until the job ends.
- scripts/memtest/mockforge.py:262-266, 280: the mock can't exercise PC-17 (the POST /pulls reply doesn't decode as a PullRequest) or CI-32 to CI-35 (version 11 gives no write buttons, and POSTs are ignored).

# 15. Fixture requirements (mock backend coverage)

Each line names a flow and the specific fixture data it needs. Check the mock forge and the git fixture repos against this list.

## App shell, worktrees, menus, settings, updates

- SH-01, RW-09: a defaults domain with two or more spaces, a saved `currentSpace`, per-space selections, `collapsed` and `recent`. Also a legacy-only `repos`/`selection` variant to test migration.
- SH-11: in one repo:
  - the main worktree;
  - a linked worktree whose folder is prefixed "<repo>-";
  - a detached worktree;
  - a locked one (`git worktree lock`);
  - one ahead and behind its upstream;
  - one with staged and unstaged changes to the same file (count once);
  - a squash-merged branch on origin/main (Merged badge);
  - a fresh branch cut from main with no commits (not Merged);
  - main itself on a long-lived branch contained in main (no badge).
- SH-12: a collapsed repo with dirty worktrees, for the summed count.
- SH-14, SH-15: at least three repos in one space and at least two spaces.
- SH-20, SH-29, SH-30, SH-31, ST-17, RW-08: an HTTPS remote whose host matches a mock-forge account and token in the repo's space. A second remote without an account to test the remote preference. An upstream with commits to pull.
- SH-22 to SH-28: at least three spaces, one of them empty, and a repo in a non-current space for SH-28.
- SH-27, ST-10: a space with accounts and Keychain tokens, to verify their deletion.
- SH-37: a repo with an ignored `.direnv/` folder, a nested `.worktree/x` linked worktree inside the main folder, and a `.gitignore` change.
- RW-01: a non-repo folder, a linked-worktree folder, a repo already in another space, a submodule, and a repo subfolder.
- RW-05: local and remote branches (origin/x with no local x; origin/y with a local y), an existing destination path, and a branch already checked out.
- RW-06: a dirty linked worktree, and a locked one.
- RW-07: a worktree removed with `git worktree remove` in a terminal while selected, and one deleted with `rm -rf` (prunable).
- ST-12, ST-13: an account entry whose Keychain item is missing (shows "No token").
- ST-14: a mock forge answering `GET /api/v1/user` with `{login}`, a GitHub-mode `/user`, a 401 for a bad token, and an unreachable URL.
- ST-15: a `gh` binary on a standard path with and without a login.
- ST-03 to ST-06: PATH with none, one, or several AI CLIs.
- UP-01 to UP-04: a bundled .app with SUFeedURL and SUPublicEDKey plus a mock appcast; an unbundled binary for the hidden case.
- MS-01: an account plus a Keychain token, and a Forgejo endpoint.
- MS-03 to MS-09: the scripts/memtest scenario JSON, mockforge.py with at least 100 PRs, PR #1 with 5000 files and comments, a PR with a check "job-01", a conflict repo with at least 20 conflicted files, a stashes repo with at least 200 stashes, a submodule repo, a big file (150k lines) and a 30 MB minified file, a 5000-file commit, and at least 31 commits.

## Changes, commit, stash, merge editor

- **CH-01–03:** A repo with an HTTPS remote on the mock forge. The branch is ahead by 2 and, separately, behind by 2 (fast-forwardable), plus a diverged case so Pull fails.
- **CH-04/05:** Local branches main, feature-a and feature-b, with feature-b checked out in a second worktree. Remote-only branches origin/remote-x and origin/feature/nested. A dirty file that conflicts with feature-a, for the error case.
- **CH-08:** A repo with the LFS-missing condition and an uninitialized submodule, so both banners show.
- **CH-09/10:** A modified, a deleted, a renamed (staged), an untracked and an unchanged file. A symbol defined in an unchanged file, for the ⌘-click jump.
- **CH-11–17:** At least 10 files across nested folders (`src/a/b/c.swift`, `src/x.ts`, `README.md`). One file both staged and modified. One untracked file in a new directory. One deleted file, one type-changed file (file → symlink), one conflicted file.
- **CH-14:** A clean repo on a branch, and one on a detached HEAD.
- **CH-21:** A submodule with new commits (dirty), and a `*.png` tracked by LFS with a lock held by another user on the mock forge.
- **CH-22–27:** Unstaged modify, delete and untracked files; a staged rename (`git mv a b`); a new repo with no commits and staged files, for CH-27.
- **CH-29–30, 36–37:** A modified tracked text file with at least 2 hunks, each with mixed `+` and `-` lines, plus a "\ No newline at end of file" case.
- **CH-32–35:** Tracked modifications, untracked files and an executable script. Edit a file after discarding it, to trigger the undo refusal.
- **CH-41/42:** A worktree with 50+ files, and an empty file.
- **CM-03–06:** Staged changes. A repo with no commits (first commit, then undo). A commit pushed to the mock remote, for the undo refusal and Revert offer. A failing `pre-commit` hook. A stopped merge, for the amend refusal.
- **CM-07:** An interactive rebase stopped at an `edit` step.
- **CM-08–12:** A fake AI CLI on PATH, for example `~/.local/bin/claude` as a script that echoes a fixed message. Variants: empty output, non-zero exit with stderr, a hang longer than 120 s, output wrapped in ``` fences and quotes, output with ANSI escapes. A diff larger than 60 KB. A repo where HEAD~1 does not exist (root commit), for amend.
- **CM-13:** The `aiHarness` default set to auto, a specific id, and off. `aiStyle` and `aiInstructions` set. `aiModel.claude` set to the legacy alias "sonnet".
- **SS-01–03:** Tracked changes plus untracked files. A selection of one tracked file and one untracked file.
- **SS-04–10:** 2–3 stashes, one including untracked files, one with a message. A worktree edit that conflicts with stash@{0}, for SS-10.
- **MG-01–03:** Stopped states for each operation: a merge with conflicts, a cherry-pick with conflicts, a revert with conflicts, a rebase with conflicts, a merge with all conflicts resolved, and an edit stop.
- **MG-04–13:** A text file with 3 conflicts (one with matching added lines at the edges, so trimming applies), one region that merged cleanly on ours, one on theirs, and one changed identically on both. Run both with `merge.conflictStyle=diff3` and without.
- **MG-16:** `git config merge.guitool` set to a stub GUI tool that rewrites the file. Separately, `merge.tool=vimdiff`, so the button is disabled.
- **MG-17:** The fake AI CLI returning merge lines, and one returning nothing.
- **MG-18:** A conflicted file whose text outside the conflicts was edited after the merge stopped, plus an external edit while the pane is open.
- **MG-19:** Both sides add the same path with different content.
- **MG-20:** Delete/modify in both directions.
- **MG-21:** Added-by-us and added-by-them, from a rename/delete combination.
- **MG-22:** A binary file (PNG) changed on both sides.
- **MG-23:** A symlink retargeted on both sides.
- **MG-24:** A submodule moved to different commits on both sides.
- **MG-25:** A CRLF file conflict, and files with no final newline.
- **FI-01–06:** A tracked file with 5+ commits, including one rename (`git mv`) in its history, a binary file in its history, and a Markdown file. A deleted file (for the silent blame failure). An untracked file and an added file.

## Diff viewer, overview, LFS, submodules

- DV-01/02: a worktree with staged, unstaged and untracked files, a file with both staged and unstaged hunks, and a clean worktree.
- DV-05/06/21: a file with 3 or more hunks, function context in the @@ headers, two separate change runs inside one hunk, and line numbers of 4 digits or more (a 1000+ line file).
- DV-07: files without a trailing newline on the old side, the new side, and both.
- DV-08: a line with tabs; a minified JS file with one line over 1500 characters.
- DV-09: a file whose only change is LF to CRLF on one line.
- DV-10: a 1-line edit in mid-line; a 3-removed/3-added run; a 2-removed/3-added run (no highlight); a 51-pair run; a 1200-character changed line; a whole-line replacement; a changed line containing a tab.
- DV-11: one small changed file per language in the DV-11 table, plus Dockerfile, Makefile, justfile, .envrc, an unknown extension, a block comment opened on a removed line, a Python triple-quoted string across lines, Rust lifetimes and macros.
- DV-12/13: a binary file (NUL bytes), an untracked binary, and a non-LFS PNG change.
- DV-14: a 100% rename, a rename with edits, a chmod-only change, a new empty file, and a copy (for `-C`).
- DV-15: a 150k-line file and a diff with more than 5000 changed lines.
- DV-16/17: a file with both additions and deletions, scrolled to the middle; a file with additions only.
- DV-18/19: a long file with separate added-only, removed-only and mixed runs.
- DV-22/34/35/36: a PR with posted comments on new, old and context lines, an outdated comment, and drafts.
- DV-29/30: identifiers defined elsewhere in the repo (for navigation).
- DV-32: repeated words, accented text, and matches beyond column 200.
- DV-37/38: a file with several commits and uncommitted lines.
- DV-44: a changed .md file.
- LF-01..08: a repo with `.gitattributes` `*.png filter=lfs`, pointers for a modified, an added and a deleted PNG; objects present for one version and missing for the other; an unstaged working-tree binary; an object of 64 MiB or more; a non-image LFS file (.bin); a mock LFS server that answers `git lfs locks --json` with locks by another owner; a remote with `lfs.<url>.locksverify false`; a machine without git-lfs on PATH.
- SM-01..07: a superproject whose submodule is (a) ahead by 2 commits, (b) behind (rewound), (c) dirty-only (modified and untracked), (d) newly added, (e) staged pointer, (f) more than 300 commits off; a submodule with a commit on a detached HEAD on no branch; a `.gitmodules` with 5 uninitialized submodules; a History commit that changes a gitlink.
- CO-01..22: a change touching SQL migrations (new, dropped, renamed and changed tables, 3+ untouched columns, more than 12 columns, a CHECK longer than 90 characters, triggers, functions over and under 25 lines, INSERT/UPDATE/DELETE seeds, warnings including an up.sql), package manifests (more than 10 changes, a major bump, a downgrade), config with nested keys (more than 10, a breaking one), code symbols, TODO/FIXME/@ts-ignore lines (more than 8), generated files, more than 16 files of one kind; a change with only code files (for "Nothing structural"); a PR whose base moved on, with both a conflicting and a clean variant, and more than 8 touching commits; a PR whose head can't be fetched (the error state).

## Lenses, code browser, quick switcher, file tree

- LN-01, LN-02, LN-03: a repo with HEAD plus uncommitted edits, a commit with a parent, a root commit (Overview A = empty), a stash entry (no Overview), and a mock-forge PR whose base moved on after it branched.
- LN-04, LN-05: one change touching code, tests, a .sql, a manifest, a config, `.github/workflows/x.yml`, docs, a PNG, a lock file, and a file whose first line says "// Code generated … DO NOT EDIT".
- LN-10: a PR base branch with 2 newer commits: one editing a file the PR also edits, conflicting; one touching an unrelated file. Also git 2.38 or newer, plus an older git, for the no-pill case.
- LN-11: `db/migrations/0001_init.sql`, `0002_x.sql`; a Diesel `migrations/2026-11-01-000000_a/{up,down}.sql`; seeds in `dev/test_data/*.sql`; one `.sql` at the repo root (root "/"); `V2__x.sql` / `U2__x.sql` (Flyway undo is left out).
- LN-12: a migration adding a table with a PK, FK, DEFAULT, CHECK, an unnamed UNIQUE, and more than 12 columns.
- LN-13: a migration with `DROP TABLE a, b` on tables that have indexes and triggers.
- LN-14: `ALTER TABLE a RENAME TO b`; and a drop + create with identical columns.
- LN-15: a migration with ADD COLUMN (with and without IF NOT EXISTS), DROP COLUMN, ALTER TYPE … USING, SET/DROP NOT NULL, SET/DROP DEFAULT, RENAME COLUMN, on a table with 6 or more untouched columns.
- LN-16: a PK change; a constraint DROP + ADD with a long CHECK; CREATE [UNIQUE] INDEX [CONCURRENTLY] [IF NOT EXISTS], named and unnamed; ALTER INDEX RENAME; CREATE TRIGGER … BEFORE UPDATE OF col … FOR EACH ROW EXECUTE FUNCTION f() where f RAISEs EXCEPTION with ERRCODE; an index on a table defined in another root.
- LN-17: CREATE TYPE AS ENUM, then ALTER TYPE ADD VALUE / RENAME VALUE / RENAME TO; CREATE OR REPLACE FUNCTION with a `$$` body over 25 lines and one under 25; CREATE MATERIALIZED VIEW; DROP FUNCTION.
- LN-18: a seed file with an INSERT added, one removed, one edited; and an INSERT on a table renamed in a migration.
- LN-19: a new migration with DROP TABLE on an existing table, DROP COLUMN, ADD COLUMN NOT NULL without DEFAULT, ALTER TYPE, SET NOT NULL, CREATE INDEX without CONCURRENTLY, UPDATE without WHERE, DELETE with WHERE; and the same in a non-migration directory (UPDATE/DELETE not flagged).
- LN-20: a new migration reusing an existing version number, and one numbered older than 3 existing ones (plus one that only exists on the PR upstream).
- LN-21: a changed migration with `DO $$ … $$`, 2× CREATE POLICY, and VACUUM.
- LN-22: SQL with nested comments, `E'\''`, `""`-escaped identifiers, and a `;` inside a `$fn$` body.
- LN-23: Cargo.toml (`[dependencies]`, `[dev-dependencies]`, `[target.x.dependencies]`, `{version=…}`, `workspace = true`, `path = …`), Cargo.lock with a duplicate crate, package.json with all 4 sections, package-lock.json with nested node_modules, go.mod with a `require (` block, Package.resolved v1 and v2, flake.lock, requirements-dev.txt, a pyproject with PEP 621 and Poetry. Bumps covering major, minor, patch and downgrade.
- LN-24: app config JSON, YAML, TOML, .env (export, comments), .properties; a secret key (`api_token`); a list value change; a multi-line value; a new file; a deleted file; a new nested group of 2-4 keys and one of 5 or more; a file under assets/ (ignored); tsconfig.json (ignored).
- LN-25: a multi-document k8s YAML (Deployment + Service) with containers named by `name`; RBAC rules; Expo `app.json` plugins; a Helm template with `{{ }}`; `|` and `>` scalars; a flow map.
- LN-26: two single-document CRD files, both getting the same new field, one also with a type change, a required field, and a new printer column; a CRD with several versions.
- LN-27: a change copying 26 files from `client/src/` to `client/src/classic/`; 5 modified TS files swapping `import { Button } from './Button'` for `PillButton`; a Python `from x import y` added in 3 files; an Expo app (expo-updates in package.json, the same runtimeVersion) with a `.swift` file changed.
- LN-28: added lines with TODO, `it.only(`, `@ts-ignore`, a TODO inside a string literal, "don't" in a comment, a TODO in a .md file and one in a generated file (ignored); more than 8 matches of one kind.
- CB-02, CB-03: a worktree with nested folders (including a single-child chain), an untracked file, a deleted file, a binary file, an empty file, an LFS pointer, a locked LFS file; and also a clean worktree and an empty repo.
- CB-04: a PR touching 3 of many files, including a deleted and a binary file; a head that needs fetching; a repo where the fetch fails.
- CB-05, CB-06: two `mod.rs` or `index.ts` files in different folders.
- CB-09 through CB-12: a Rust or TS repo where `foo` is defined once, `bar` is defined in 3 files (the same file, the same directory, elsewhere), `baz` is only called and never defined, a word with more than 2000 references, a word found only in an untracked file, and a path containing ":".
- CB-16, CB-17: a Swift file with a class (methods, properties, init); a Go file with type, func, method; a 3 MB file; a minified 200 KB single-line `.js`; a `.sql` file (no outline).
- CB-18: one small file per supported extension (`.h`, `.mm`, `.pyi`, `.mts`, `.cjs`, …).
- QS-01 through QS-07: 2 or more spaces, each with 2 repos of 2-3 worktrees, including a detached HEAD, a merged branch, a collapsed repo, and folder names prefixed with the repo name.
- FT-01 through FT-09: paths that need natural sorting (`file2`, `file10`), mixed-case names, a deep single-child chain, and a PR with viewed and unviewed files under one folder.

## History, rebase, undo

- HI-01/HI-22: one repo with an unborn branch (no commits); one with history.
- HI-03: a forge account plus a remote so avatars resolve; an author email with no forge account.
- HI-04: a merge commit on main; 2-3 diverging feature branches; a branch where origin/main is ahead of main; more than 12 concurrent branches to hit the lane cap.
- HI-05: one commit carrying HEAD→main, origin/main, two tags and a local branch; a detached HEAD worktree; refs/pull/* refs.
- HI-06: more than 250 commits on the current branch.
- HI-09: at least 2 local and 2 remote branches (one deletable mid-test).
- HI-10/HI-19: 3 or more stashes, one with untracked files, one that conflicts with the working tree.
- HI-11: commits by 2 or more authors with distinctive subjects.
- HI-12/HI-14: a commit with nested directories, a rename, a mode change, a binary file, an LFS pointer, a .md file, a deleted file; the root commit.
- HI-13: a commit with a Markdown body; a merge whose second parent lies beyond page 1.
- HI-15/HI-17: a branch feature-x 2 commits ahead of main, viewed from main's worktree; a commit on it that conflicts with main; a merge commit not in HEAD; an already-applied (empty) cherry-pick.
- HI-18: a commit whose revert conflicts with later commits.
- HI-20: a repo with a free path next to it for the new worktree.
- HI-21: a mock forge with an open PR that merges server-side and serves git.
- RB-01/RB-02: a branch with 5 linear local commits above a merge commit, plus a root-adjacent linear range.
- RB-03/RB-07/RB-13: two commits that edit the same lines, so reordering them conflicts.
- RB-04/RB-06: commits with multi-line messages; a commit whose parent is the root.
- RB-08: the branch pushed to origin with 2 extra unpushed commits on top.
- RB-10: 4 or more local commits, at least 1 pushed.
- RB-11: a dirty worktree (autostash); a repo with an in-progress cherry-pick.
- RB-12: a range containing a merge; a repo with more than 1000 linear commits.
- UN-04: a cherry-picked commit that is then pushed.
- UN-07: pop a stash, then edit one of its files before undoing.
- UN-08: commit in a terminal after a Loupe action.

## Pull Requests

- PR-02: a repo with no remote, or `origin` = `file:///tmp/x`.
- PR-03: a repo whose remote host has no account in the current space; and a variant where the account exists but its Keychain token was removed.
- PR-05: ≥51 open PRs (to see page 2 fetched), plus an existing `pulls-<host>-<owner>-<name>.json` cache file.
- PR-06: a worktree on branch `feat/a` with an open PR from `feat/a`, a second worktree on `feat/b` with a PR from `feat/b`, 3 other PRs, and a fork PR whose head ref is also `feat/a`.
- PR-07: one open, one closed-unmerged, one merged (and on GitHub, `merged_at` only), one draft, one with comments=3.
- PR-08: users with a valid avatar_url, a 404 avatar_url, and none.
- PR-09/PR-12: more than 100 closed PRs; a mock that fails the closed list (for the stale-list bug).
- PR-10: account username `alice`, PRs by `Alice` (different case) and by `bob`.
- PR-13: a mock that changes a PR title between two polls, and one that returns identical data (check the cache file's mtime doesn't change).
- PR-17/PR-18: an empty repo; a mock returning 500 or 401 with no cache.
- PR-19: the current branch ahead of the default branch with no open PR; the same branch with an open PR.
- PD-05: a PR with a 10-file diff, 3 reviews (APPROVED, REQUEST_CHANGES, COMMENT-empty), 4 issue comments, 5 commits, and 3 inline comments in 2 reviews; a mock where `/commits` fails (it should be tolerated).
- PD-07: reviews: alice APPROVED (id 1) then alice REQUEST_REVIEW (id 5); bob REQUEST_CHANGES; carol APPROVED + dismissed; dave APPROVED + stale; requested_reviewers=[erin, alice]; GitHub variant with alice APPROVED and in requested_reviewers.
- PD-13/PD-14: a PR touching `src/a/x.ts`, `src/a/y.ts`, `src/b/z.ts`, a binary file, a rename, a delete and a mode-only change.
- PD-15: a binary file, a mode-change-only file, an LFS pointer file and a `.md` file.
- PD-16: a PR whose second push changes one file's diff (it should turn unviewed again) and leaves another unchanged.
- PD-20/PD-21: a repo with nested folders the PR doesn't touch, a large unchanged file, an empty file, and a binary unchanged file; a mock where `refs/pull/N/head` can't be fetched (treeError).
- PD-25: a symbol defined once (it jumps), one defined twice (picker), and a built-in with no definition (references only).
- PD-28: a PR whose `pr/N` worktree already exists; a destination folder `<repo>-pr-N` already on disk.
- PD-29/PD-31: an open PR with mergeable=true, one with mergeable=false, one with mergeable=null; a mock merge returning 405 or 409; a draft PR.
- PD-30: a mock forge that really merges and serves git (vault: the memtest mock forge merges for real), with the base branch `main`.
- RV-02: same-user comments 10 min apart, a push between two comments, the same user 40 min apart, a commit dated before the PR's created_at, items on different days, and an item with no date.
- RV-03: a PR with an empty or whitespace body; a PR with created_at missing.
- RV-04: reviews APPROVED, REQUEST_CHANGES, REQUEST_REVIEW and COMMENT with a body, a review on an older commit_id, and a `renovate[bot]` user.
- RV-05: an inline comment whose review id isn't in the review list (standalone), an outdated inline comment (commit_id ≠ head), and one with a diff_hunk.
- RV-09: a comment holding every Markdown construct listed in RV-09.
- RV-10: a mock where POST comment fails (500).
- RV-11: Forgejo comments with position=21/original_position=0 and position=0/original_position=7; GitHub LEFT and RIGHT comments; an outdated comment whose line is no longer in the diff.
- RV-12/RV-13/RV-14: a PR in Full mode with a changed file whose hunk covers lines 10–20 of 200; 2 pending drafts left from a previous launch.
- RV-17: a mock that records the review POST body (check the Forgejo vs GitHub shapes); a mock rejecting a self-approve (422).
- FG-04: GitHub mock returning 403 `{"message":"API rate limit exceeded"}`.
- FG-07: GitHub mock returning 406 for `application/vnd.github.diff`, with refs/pull/N/head fetchable.

## CI and PR create

- CI-01, CI-10, CI-13, CI-14: a PR head sha with a mix of statuses: failure, error, warning, success, skipped, pending with description "running", pending with "queued", and cancelled. Also a sha with zero statuses (No Checks), and one with only skipped statuses.
- CI-04: the forge's /version set per scenario:
  - "17.0.0" (Forgejo, all writes but rerun-failed)
  - "16.0.0" (cancel only)
  - "11.0.0+gitea-1.22.0" (the mock's: no writes)
  - Gitea "1.24.0" (no Actions API, shows the notice)
  - Gitea "1.26.0" (rerun but no cancel)
  - Gitea "1.27.0" (everything)
  - a /version answering 500 (Forgejo fallback)
- CI-05: more than 50 statuses, to exercise paging with total_count.
- CI-06: a GitHub-mode account. Combined statuses plus check-runs, with app.slug "github-actions" (id = job id) and third-party runs carrying an output summary and annotations_count > 0, plus `/check-runs/{id}/annotations`.
- CI-07: a run whose `index_in_repo` differs from its `id`. Status target_url "/o/r/actions/runs/<index>/jobs/<n>". Job html_url ".../jobs/<n>/attempt/<a>", with two attempts of the same n. A status context "wf / job (push)" with no `/jobs/` segment, for the name fallback.
- CI-11: `/actions/runs` answering 403, and separately 404, while statuses succeed.
- CI-15: all checks done (30 s cadence) vs one still running (5 s).
- CI-20, CI-23: the per-job GET returns steps with started and stopped (running, success, skipped). The list endpoint returns jobs without steps.
- CI-21, CI-23, CI-24, CI-28, CI-29: `/actions/jobs/{id}/logs` honouring Range (206 + Content-Range, 416 when nothing is new), a log that grows while the job runs (the mock does about 25 lines/s), a finished log over 16 MB, and a server that ignores Range (200 whole body).
- CI-22: a queued job whose log returns 404, a GitHub running job whose log returns 404, and a finished job whose log keeps returning 404 (gives up after about 30 s).
- CI-25, CI-26, CI-27: a failing job log with:
  - ISO timestamps and a BOM
  - `##[group]` / `##[endgroup]`, including a group split across two tail chunks
  - `##[error]`, `##[warning]`, `##[notice]`, `##[debug]` and `[command]` lines
  - ⭐ and ✅ lines
  - "error: …", "FAILED", "thread 'x' panicked at", and file:line errors inside a folded group
  - more than 200 problem lines
  - ANSI colour codes, CRLF, and a lone CR
- CI-31, CI-38: statuses with absolute and relative target_url, and one with none. A non-Actions status ("staged-ai-review/x") with an external URL.
- CI-32 to CI-36: the POST rerun, rerun-failed-jobs, cancel and jobs/{id}/rerun endpoints. Make them return 204 and then bump the job attempt or status. Also variants returning 403 and 409. The mock currently answers any POST with 201 {"message":"mock: ignored"} and its /version disables the writes: it needs a version ≥ 17 to show the buttons.
- CI-39: GitHub mode only. Logs answer a 302 to another host, and that host must not see the Authorization header.
- PC-01 to PC-06:
  - a local branch with N commits ahead of origin/main (and with origin/HEAD set)
  - a branch with no remote branch (needsPush)
  - a branch fully pushed
  - a branch already merged or squashed into main (isMerged)
  - a detached HEAD
  - a worktree with no account
  - an open PR for the branch from the same repo, and a fork PR with the same head ref name but a different head.repo_id
- PC-07, PC-10: a `.forgejo/pull_request_template.md` and a `.github/PULL_REQUEST_TEMPLATE.md` (order test), and `GET /repos/{o}/{r}` returning default_branch. A variant where that request fails, to fall back to integrationTarget.
- PC-08: several remote branches, including the default branch missing from refs/remotes, and an unrelated base (gh-pages) with more than 100 commits ("and N more").
- PC-11: an AI harness installed (Settings aiHarness).
- PC-17: POST /pulls returning 201 with a full PullRequest JSON. The mock's "mock: ignored" body fails to decode as a PullRequest, so with the mock as it is, create always shows an error.
- PC-18: POST /pulls → 409 {"message":"pull request already exists"} (Forgejo), and 422 {"message":"Validation Failed","errors":[{"message":"A pull request already exists for o:b."}]} (GitHub).
- PC-19: POST /pulls → 422 with another reason, and a 500 with a non-JSON body.
- PC-16: an HTTPS remote whose push needs the token, plus a push rejection (non-fast-forward).

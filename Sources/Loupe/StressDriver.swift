import AppKit
import SwiftUI

/// Memory stress test driver (scripts/memtest/). Off unless the environment has LOUPE_STRESS=<scenario.json>: then
/// `makeModel()` is the only thing that runs differently, and with the variable unset it is one getenv and AppModel().
///
/// In stress mode the app:
/// - refuses to run under the real bundle id (dev.extypen.loupe): the test copy has its own UserDefaults domain, which
///   is wiped at launch;
/// - never touches the Keychain: the mock forge's account and its dummy token live in memory only;
/// - opens the scenario's mock repos and loops its steps through the models on the main actor, pausing so views render;
/// - prints `LOUPE_STRESS <event> ...` lines on stdout (iteration and step boundaries) for scripts/memtest/measure.py.
@MainActor
enum StressDriver {
    struct Scenario: Decodable {
        var forge: String
        var token: String
        var user: String
        var repos: [String]
        var big: String
        var conflict: String
        var submodule: String
        var stashes: String
        var bigFileWorktree: String
        var bigFile: String
        var minifiedFile: String
        var hugeCommit: String
        var hugePR: Int
        var prs: [Int]
        var iterations: Int
        var pause: Double
        var logSeconds: Double
        var background: Bool
        var steps: [String]
    }

    static func makeModel() -> AppModel {
        guard let path = ProcessInfo.processInfo.environment["LOUPE_STRESS"] else { return AppModel() }
        let id = Bundle.main.bundleIdentifier ?? ""
        guard !id.isEmpty, id != "dev.extypen.loupe" else {
            fputs("LOUPE_STRESS refused: run a copy with its own bundle id (got \"\(id)\"), never the real Loupe\n", stderr); exit(2)
        }
        let s: Scenario
        do { s = try JSONDecoder().decode(Scenario.self, from: Data(contentsOf: URL(fileURLWithPath: path))) }
        catch { fputs("LOUPE_STRESS: can't read \(path): \(error)\n", stderr); exit(2) }

        // A clean defaults domain (the copy's own), with just the mock repos in the default space.
        let d = UserDefaults.standard
        d.removePersistentDomain(forName: id)
        d.set(try? JSONEncoder().encode([Space(id: Space.defaultID, name: "Default", repos: s.repos, selection: s.big)]), forKey: "spaces")
        d.set("changes", forKey: "tab")
        d.set(false, forKey: "prFullMode")
        d.set(DiffSide.diff.rawValue, forKey: "diffSide")
        d.set(true, forKey: "lfsMissingBannerDismissed")
        if ProcessInfo.processInfo.environment["LOUPE_STRESS_LOGSTACKS"] != nil { logStacks() }
        let app = AppModel()   // no accounts in defaults: its Keychain read finds nothing to read
        let account = ForgejoAccount(baseURL: s.forge, username: s.user)
        app.accounts = [account]
        d.removeObject(forKey: "accounts")   // in memory only: a later launch must not look this up in the Keychain
        app.tokens[account.id] = s.token
        app.tokensLoaded = true
        Task { await StressRun(app: app, s: s).run() }
        return app
    }
}

/// LOUPE_STRESS_LOGSTACKS=1: every NSLog line that mentions "reentrant" (AppKit's NSTableView warning) is followed by
/// the stack that logged it. Foundation's private NSLog hook runs on the logging thread, so the stack is the culprit's.
private func logStacks() {
    typealias LogFn = @convention(c) (UnsafePointer<CChar>?, UInt32, Bool) -> Void
    guard let sym = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "_NSSetLogCStringFunction") else { fputs("LOUPE_STRESS no NSLog hook\n", stderr); return }
    let set = unsafeBitCast(sym, to: (@convention(c) (LogFn) -> Void).self)
    set { msg, _, _ in
        guard let msg else { return }
        let line = String(cString: msg)
        fputs(line + "\n", stderr)
        if line.contains("reentrant") { fputs("--- stack\n" + Thread.callStackSymbols.joined(separator: "\n") + "\n---\n", stderr) }
    }
}

@MainActor
private struct StressRun {
    let app: AppModel
    let s: StressDriver.Scenario
    let start = Date()

    func mark(_ event: String) {
        let line = String(format: "LOUPE_STRESS %@ t=%.2f\n", event, Date().timeIntervalSince(start))
        fputs(line, stdout); fflush(stdout)
    }

    func pause(_ factor: Double = 1) async { try? await Task.sleep(for: .seconds(s.pause * factor)) }

    /// Polls until `cond` holds, up to `timeout` seconds; says so when it gave up.
    @discardableResult
    func wait(_ what: String, _ timeout: Double = 30, _ cond: () -> Bool) async -> Bool {
        let end = Date().addingTimeInterval(timeout)
        while !cond() {
            if Date() > end { mark("timeout \(what.replacingOccurrences(of: " ", with: "_"))"); return false }
            try? await Task.sleep(for: .milliseconds(100))
        }
        return true
    }

    func select(_ path: String, tab: String) async {
        app.selection = path
        UserDefaults.standard.set(tab, forKey: "tab")
        await pause()
    }

    func model(_ path: String) -> WorktreeModel { app.model(for: path) }

    func run() async {
        await wait("worktrees", 60) { s.repos.allSatisfy { app.worktrees[$0] != nil } }
        await pause(3)
        // As wide as the screen: in a narrower window the toolbar clips Compact/Full and the view switch into its overflow.
        if let w = NSApp.windows.first(where: { $0.canBecomeMain }), let screen = w.screen ?? NSScreen.main {
            w.setFrame(screen.visibleFrame, display: true)
            mark(String(format: "window %.0fx%.0f", w.frame.width, w.frame.height))
        }
        if s.background {
            // Behind the owner's windows and never key: the test must not take keystrokes or cover what they're doing.
            for w in NSApp.windows where w.canBecomeMain { w.orderBack(nil) }
        }
        mark("ready")
        for i in 1...max(1, s.iterations) {
            for step in s.steps {
                let t = Date()
                mark("step-begin iter=\(i) step=\(step)")
                await perform(step)
                mark(String(format: "step-end iter=%d step=%@ secs=%.1f", i, step, Date().timeIntervalSince(t)))
            }
            // The same resting state after every iteration, so samples compare.
            await select(s.big, tab: "changes")
            await pause(2)
            mark("iter iter=\(i)")
        }
        mark("finished")
    }

    func perform(_ step: String) async {
        switch step {
        case "worktrees": await worktrees()
        case "tabs": await tabs()
        case "tabsonly": await tabsOnly()
        case "idle": await pause(30)   // nothing driven: background work only (file events, polls)
        case "history": await history()
        case "bigfile": await bigFile()
        case "prs": await prs()
        case "hugepr": await hugePR()
        case "checks": await checks()
        case "merge": await merge()
        case "create": await create()
        case "rebase": await rebase()
        case "stashes": await stashes()
        case "submodule": await submodule()
        default: mark("unknown-step \(step)")
        }
    }

    /// Every worktree of every repo, then ⌘K.
    func worktrees() async {
        for repo in s.repos { for wt in app.worktrees[repo] ?? [] { app.selection = wt.path; await pause(0.5) } }
        app.showSwitcher = true; await pause(2)
        app.showSwitcher = false; await pause()
    }

    func tabs() async {
        for wt in [s.big, s.conflict] {
            for t in ["changes", "history", "pulls", "changes"] { await select(wt, tab: t); await pause() }
        }
    }

    /// Only the view switch, on one worktree: what a person flipping Changes / History / Pull Requests does.
    func tabsOnly() async {
        if app.selection != s.big { await select(s.big, tab: "changes") }
        for t in ["history", "pulls", "changes"] { UserDefaults.standard.set(t, forKey: "tab"); await pause(1) }
    }

    /// The 5,000-file commit: Overview, Compact/Full, Diff/Before/After; then All Branches.
    func history() async {
        await select(s.big, tab: "history")
        let h = model(s.big).history
        await wait("history") { !h.commits.isEmpty }
        h.select(s.hugeCommit)
        await wait("huge commit") { h.files.count >= 5000 }
        await pause(2)
        for p in h.files.prefix(400).map(\.path).enumerated().filter({ $0.offset % 40 == 0 }).map(\.element) { h.selectedPath = p; await pause(0.3) }
        let d = UserDefaults.standard
        d.set(true, forKey: "commitOverview"); await pause(5)
        d.set(false, forKey: "commitOverview"); await pause()
        d.set(true, forKey: "prFullMode"); await pause(2)
        d.set(false, forKey: "prFullMode"); await pause()
        for side in [DiffSide.old, .new, .diff] { d.set(side.rawValue, forKey: "diffSide"); await pause(1.5) }
        h.setScope(.all)
        await wait("all branches") { !h.commits.isEmpty }
        await pause(2)
        h.setScope(.current)
        await wait("current") { !h.commits.isEmpty }
    }

    /// The 150k-line file and the 30 MB minified one in Changes, in every mode; then the Overview of the worktree.
    func bigFile() async {
        let m = model(s.bigFileWorktree)
        await select(s.bigFileWorktree, tab: "changes")
        await wait("status") { !m.files.isEmpty }
        let d = UserDefaults.standard
        for path in [s.bigFile, s.minifiedFile] {
            guard let f = m.files.first(where: { $0.path == path && !$0.staged }) else { mark("missing \(path)"); continue }
            m.select(f.id)
            await wait("diff \(path)", 60) { m.diff?.path == path }
            await pause(2)
            for side in [DiffSide.old, .new, .diff] { d.set(side.rawValue, forKey: "diffSide"); await pause(2) }
            d.set(true, forKey: "prFullMode"); await pause(3)
            d.set(false, forKey: "prFullMode"); await pause(2)
        }
        if pressViewSwitch(0) { await pause(6); _ = pressViewSwitch(1); await pause() } else { mark("no-view-switch changes") }
    }

    var bigModel: WorktreeModel { model(s.big) }

    func openPulls() async -> PullListModel {
        await select(s.big, tab: "pulls")
        let l = bigModel.pulls
        await wait("pull list", 90) { l.pulls.count >= 100 }
        return l
    }

    func prs() async {
        let l = await openPulls()
        for n in s.prs {
            l.selected = n
            await wait("pr \(n)") { l.details[n]?.files.isEmpty == false }
            await pause(1.5)
            l.selected = nil
            await pause(0.5)
        }
    }

    /// PR #1: 5,000 files, hundreds of inline comments. Files, Full mode, Overview, Conversation.
    func hugePR() async {
        let l = await openPulls()
        l.selected = s.hugePR
        await wait("huge pr", 120) { (l.details[s.hugePR]?.files.count ?? 0) >= 5000 && !(l.details[s.hugePR]?.comments.isEmpty ?? true) }
        guard let p = l.details[s.hugePR] else { return }
        await pause(2)
        for f in p.files.enumerated().filter({ $0.offset % 500 == 0 }).map(\.element) { p.selectedPath = f.path; await pause(0.4) }
        UserDefaults.standard.set(true, forKey: "prFullMode")
        await wait("pr tree", 60) { !p.tree.isEmpty }
        for f in p.files.prefix(5) { p.selectedPath = f.path; await pause(0.4) }
        await pause(2)
        UserDefaults.standard.set(false, forKey: "prFullMode"); await pause()
        if pressViewSwitch(0) {
            await pause(8)
            _ = pressViewSwitch(2); await pause(4)
            _ = pressViewSwitch(1); await pause()
        } else { mark("no-view-switch pr") }
        l.selected = nil
        await pause()
    }

    /// The checks overlay of a PR, then a running job's live log for logSeconds.
    func checks() async {
        let l = await openPulls()
        let n = s.prs.first ?? s.hugePR
        l.selected = n
        await wait("checks pr") { l.details[n] != nil }
        guard let p = l.details[n] else { return }
        await pause()
        app.checksOverlay = (p.checks, s.forge)
        await wait("checks") { !p.checks.checks.isEmpty }
        await pause(2)
        if clickCheck(p.checks, containing: "job-01") {
            await wait("job log", 20) { !isShowingList(p.checks) }
            await pause(s.logSeconds / max(s.pause, 0.01))
        } else { mark("no-check-row") }
        app.checksOverlay = nil
        await pause()
        l.selected = nil
        await pause()
    }

    /// Opens the merge editor on 20 of the conflicted files (a different 20 each time round) and leaves it.
    func merge() async {
        let m = model(s.conflict)
        await select(s.conflict, tab: "changes")
        await wait("conflicts") { !m.conflicted.isEmpty }
        let c = m.conflicted
        for k in 0..<min(20, c.count) { m.select(c[(k * 7) % c.count].id); await pause(0.8) }
        await select(s.big, tab: "changes")
    }

    func create() async {
        let wt = (app.worktrees[s.big] ?? []).dropFirst().first?.path ?? s.big
        await select(wt, tab: "changes")
        app.creatingPull = wt; await pause(3)
        app.creatingPull = nil; await pause()
    }

    func rebase() async {
        await select(s.big, tab: "history")
        let h = model(s.big).history
        await wait("history") { h.commits.count > 30 }
        guard h.commits.count > 30 else { return }
        await h.rebase.openPlanner(from: h.commits[25])
        await pause(2.5)
        h.rebase.planner = nil
        await pause()
    }

    func stashes() async {
        await select(s.stashes, tab: "history")
        let h = model(s.stashes).history
        h.setScope(.stashes)
        await wait("stashes") { h.commits.count >= 200 }
        for c in h.commits.enumerated().filter({ $0.offset % 20 == 0 }).map(\.element) { h.select(c.sha); await pause(0.5) }
        h.setScope(.current)
        await pause()
    }

    func submodule() async {
        let m = model(s.submodule)
        await select(s.submodule, tab: "changes")
        await wait("submodule status") { !m.files.isEmpty }
        for f in m.files { m.select(f.id); await pause(1) }
    }

    // MARK: Controls only views own

    /// The toolbar's Overview / Files (/ Conversation) switch is view state (it comes up through a preference): pick
    /// a segment of the rightmost image-only segmented control in the window's toolbar, as a click would.
    func pressViewSwitch(_ segment: Int) -> Bool {
        guard let w = NSApp.windows.first(where: { $0.canBecomeMain }), let frame = w.contentView?.superview else { return false }
        var found: [NSSegmentedControl] = []
        func walk(_ v: NSView) {
            if let c = v as? NSSegmentedControl, c.segmentCount >= 2, (0..<c.segmentCount).allSatisfy({ (c.label(forSegment: $0) ?? "").isEmpty }) { found.append(c) }
            v.subviews.forEach(walk)
        }
        walk(frame)
        guard let c = found.max(by: { $0.convert($0.bounds, to: nil).maxX < $1.convert($1.bounds, to: nil).maxX }), segment < c.segmentCount else { dump(); return false }
        c.selectedSegment = segment
        mark("view-switch segment=\(segment) of=\(c.segmentCount)")
        return c.sendAction(c.action, to: c.target)
    }

    /// The checks list's NSTableView: the one with a row per check plus one per state section.
    func checksTable(_ m: ChecksModel) -> NSTableView? {
        let sections = Set(m.checks.map(\.state)).count
        var found: NSTableView?
        func walk(_ v: NSView) {
            if let t = v as? NSTableView, t.numberOfRows == m.checks.count + sections { found = t }
            v.subviews.forEach(walk)
        }
        NSApp.windows.filter(\.isVisible).compactMap(\.contentView).forEach(walk)
        return found
    }

    func isShowingList(_ m: ChecksModel) -> Bool { checksTable(m) != nil }

    /// Clicks a check's row in the overlay's list (which job is open is ChecksSheet's own @State): a mouse down/up sent
    /// to the window at the row, never through the system event stream, so the real pointer and other apps are untouched.
    func clickCheck(_ m: ChecksModel, containing text: String) -> Bool {
        guard let t = checksTable(m), let w = t.window else { dump(); return false }
        var row = 0, target: Int?
        for st in [CheckState.failing, .running, .pending, .success, .skipped] {
            let rows = m.checks.filter { $0.state == st }
            guard !rows.isEmpty else { continue }
            row += 1   // the section header
            for c in rows { if target == nil, c.id.contains(text) { target = row }; row += 1 }
        }
        guard let r = target else { return false }
        t.scrollRowToVisible(r)
        let rect = t.rect(ofRow: r)
        let p = t.convert(NSPoint(x: rect.midX, y: rect.midY), to: nil)
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            guard let e = NSEvent.mouseEvent(with: type, location: p, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                             windowNumber: w.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1) else { return false }
            w.sendEvent(e)
        }
        return true
    }

    /// Presses the first accessibility button whose label (or a descendant's text) contains `text`. Only for the CI
    /// checks list, whose open row is the sheet's own @State.
    func press(containing text: String) -> Bool {
        func label(_ e: NSAccessibilityProtocol) -> String {
            [e.accessibilityLabel(), e.accessibilityTitle(), e.accessibilityValue() as? String].compactMap { $0 }.joined(separator: " ")
        }
        func texts(_ e: NSAccessibilityProtocol, _ depth: Int) -> Bool {
            if label(e).contains(text) { return true }
            guard depth > 0 else { return false }
            return (e.accessibilityChildren() ?? []).contains { ($0 as? NSAccessibilityProtocol).map { texts($0, depth - 1) } ?? false }
        }
        func find(_ e: NSAccessibilityProtocol, _ depth: Int) -> NSAccessibilityProtocol? {
            if e.accessibilityRole() == .button, texts(e, 4) { return e }
            guard depth > 0 else { return nil }
            for c in e.accessibilityChildren() ?? [] { if let c = c as? NSAccessibilityProtocol, let hit = find(c, depth - 1) { return hit } }
            return nil
        }
        for w in NSApp.windows where w.isVisible { if let b = find(w, 40) { return b.accessibilityPerformPress() } }
        dump()
        return false
    }

    /// LOUPE_STRESS_DUMP=1: when a control can't be found, the window's views and accessibility tree go to stderr.
    func dump() {
        guard ProcessInfo.processInfo.environment["LOUPE_STRESS_DUMP"] != nil else { return }
        func views(_ v: NSView, _ d: Int) {
            var extra = ""
            if let c = v as? NSSegmentedControl { extra = " segments=" + (0..<c.segmentCount).map { c.label(forSegment: $0) ?? "nil" }.joined(separator: "|") }
            fputs(String(repeating: " ", count: d) + String(describing: type(of: v)) + extra + "\n", stderr)
            v.subviews.forEach { views($0, d + 1) }
        }
        func ax(_ e: NSAccessibilityProtocol, _ d: Int) {
            guard d < 30 else { return }
            fputs(String(repeating: " ", count: d) + "\(e.accessibilityRole()?.rawValue ?? "?") [\(e.accessibilityLabel() ?? "")] [\(e.accessibilityTitle() ?? "")]\n", stderr)
            for c in e.accessibilityChildren() ?? [] { if let c = c as? NSAccessibilityProtocol { ax(c, d + 1) } }
        }
        for w in NSApp.windows where w.isVisible {
            fputs("=== window \(w.title) views\n", stderr)
            if let f = w.contentView?.superview { views(f, 0) }
            fputs("=== window \(w.title) accessibility\n", stderr)
            ax(w, 0)
        }
    }
}

import SwiftUI
import AppKit
import Observation

// MARK: - Plan

enum RebaseAction: String, CaseIterable, Identifiable {
    case pick, reword, edit, squash, fixup, drop
    var id: String { rawValue }
    var title: String { rawValue.capitalized }
}

/// One commit of an interactive rebase and what to do with it.
struct RebaseItem: Identifiable, Hashable {
    var sha: String
    var parents: [String]
    var message: String              // the commit's full message
    var action = RebaseAction.pick
    var newMessage: String? = nil    // reword / squash: this commit's part of the result message, when edited
    var id: String { sha }
    var subject: String { String(message.prefix { $0 != "\n" }) }
    var short: String { String(sha.prefix(8)) }
    var effectiveMessage: String { newMessage ?? message }
}

/// The commits `base..HEAD`, oldest first. `base` nil means from the root commit.
struct RebaseRange {
    var base: String?
    var items: [RebaseItem]
    var pushed: Set<String>          // commits some remote branch already has
}

enum RebaseTodo {
    struct Built: Equatable {
        var todo: String
        var messages: [String: String]   // file name in the message directory → the message it holds
    }

    static func shellQuoted(_ s: String) -> String { "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'" }

    /// The todo for `git rebase -i`, oldest first. Git only ever sees pick/edit/fixup/drop/exec: a reword or squash
    /// becomes a fixup followed by `exec git commit --amend -F <prepared message>`, so no editor ever opens.
    /// Squash/fixup meld into the closest older commit that isn't dropped, as in git.
    static func build(_ items: [RebaseItem], messageDir: String) throws -> Built {
        var lines: [String] = [], messages: [String: String] = [:]
        var group: (headMessage: String, parts: [String])?
        func flush() {
            guard let g = group else { return }
            group = nil
            let message = g.parts.joined(separator: "\n\n")
            guard message != g.headMessage else { return }
            let name = "message-\(messages.count + 1)"
            messages[name] = message
            lines.append("exec git commit --amend --allow-empty --quiet -F " + shellQuoted((messageDir as NSString).appendingPathComponent(name)))
        }
        for item in items {
            switch item.action {
            case .drop:
                lines.append("drop \(item.sha)")
            case .pick, .reword, .edit:
                flush()
                lines.append("\(item.action == .edit ? "edit" : "pick") \(item.sha)")
                group = (item.message, [item.action == .reword ? item.effectiveMessage : item.message])
            case .squash, .fixup:
                guard group != nil else {
                    throw GitError(message: "\(item.short) “\(item.subject)” has no older commit left to \(item.action.rawValue) into.")
                }
                lines.append("fixup \(item.sha)")
                if item.action == .squash { group?.parts.append(item.effectiveMessage) }
            }
        }
        flush()
        return Built(todo: lines.joined(separator: "\n") + "\n", messages: messages)
    }

    /// Index of the first item the plan changes (order or action), nil when the plan does nothing. Commits before
    /// it keep their SHAs: git fast-forwards over unchanged picks.
    static func firstChange(original: [String], plan: [RebaseItem]) -> Int? {
        var first: Int? = plan.count < original.count ? plan.count : nil
        for (i, item) in plan.enumerated() {
            if i >= original.count || original[i] != item.sha
                || (item.action == .reword && item.effectiveMessage != item.message)
                || (item.action != .pick && item.action != .reword) { first = i; break }
        }
        // A squash or fixup also rewrites the commit it melds into, which may sit before the first change.
        var head: Int?
        for (i, item) in plan.enumerated() {
            switch item.action {
            case .pick, .reword, .edit: head = i
            case .squash, .fixup: if let h = head, h < (first ?? .max) { first = h }
            case .drop: break
            }
        }
        return first
    }

    /// The commits the plan rewrites that a remote already has.
    static func rewrittenPushed(_ range: RebaseRange, plan: [RebaseItem]) -> [RebaseItem] {
        guard let first = firstChange(original: range.items.map(\.sha), plan: plan) else { return [] }
        return plan[first...].filter { range.pushed.contains($0.sha) }
    }

    enum Zone: Equatable { case above, below, onto }   // as seen in History: newest on top

    /// Drag-and-drop in History: `sha` dropped above / below / onto `target`. Onto squashes (or fixes up) into it.
    static func move(_ items: [RebaseItem], _ sha: String, to zone: Zone, of target: String, fixup: Bool = false) -> [RebaseItem] {
        guard sha != target, var moved = items.first(where: { $0.sha == sha }) else { return items }
        var out = items.filter { $0.sha != sha }
        guard let t = out.firstIndex(where: { $0.sha == target }) else { return items }
        // Oldest first here, so "above" (newer) is after the target.
        if zone == .onto { moved.action = fixup ? .fixup : .squash }
        out.insert(moved, at: zone == .below ? t : t + 1)
        return out
    }

    static func parseLog(_ s: String) -> [RebaseItem] {
        s.split(separator: "\u{1e}").compactMap { rec in
            let f = rec.drop { $0 == "\n" }.split(separator: "\u{1f}", maxSplits: 2, omittingEmptySubsequences: false)
            guard f.count == 3, !f[0].isEmpty else { return nil }
            return RebaseItem(sha: String(f[0]), parents: f[1].split(separator: " ").map(String.init),
                              message: f[2].trimmingCharacters(in: .whitespacesAndNewlines))
        }
    }
}

// MARK: - Running it

extension Git {
    static let rebaseLimit = 1000

    /// `oldest^..HEAD` (or all of HEAD from the root), refusing merges: a hand-built todo can't keep them.
    static func rebaseRange(from oldest: String, in dir: String) async throws -> RebaseRange {
        let base = (try? await string(["rev-parse", "--verify", "-q", "\(oldest)^"], in: dir))?.trimmingCharacters(in: .whitespacesAndNewlines)
        let rev = base.map { "\($0)..HEAD" } ?? "HEAD"
        async let log = string(["log", "--no-show-signature", "--reverse", "-n", "\(rebaseLimit + 1)", "--format=%H%x1f%P%x1f%B%x1e", rev, "--"], in: dir)
        async let local = string(["rev-list", "-n", "\(rebaseLimit + 1)", rev, "--not", "--remotes", "--"], in: dir)
        let items = RebaseTodo.parseLog(try await log)
        if items.count > rebaseLimit { throw GitError(message: "More than \(rebaseLimit) commits to rewrite. Use a terminal for a rebase this big.") }
        guard items.contains(where: { $0.sha == oldest }) else {
            throw GitError(message: "\(oldest.prefix(8)) isn't on the current branch.")
        }
        if let m = items.last(where: { $0.parents.count > 1 }) {
            throw GitError(message: "The commits to rewrite include the merge \(m.short) “\(m.subject)”. Rewriting it would flatten the merge, so Loupe leaves this to a terminal (git rebase -i --rebase-merges).")
        }
        let notPushed = Set((try await local).split(separator: "\n").map(String.init))
        return RebaseRange(base: base, items: items, pushed: Set(items.map(\.sha)).subtracting(notPushed))
    }

    /// Runs the plan with `git rebase -i --autostash`. The prepared todo is copied in by GIT_SEQUENCE_EDITOR and
    /// GIT_EDITOR is `true`, so no editor opens. Returns true when the rebase stopped (an edit, a conflict, a failed
    /// hook) and waits in the git dir for Continue or Abort.
    @discardableResult
    static func runRebase(_ range: RebaseRange, plan: [RebaseItem], in dir: String) async throws -> Bool {
        let gitDir = try await string(["rev-parse", "--path-format=absolute", "--git-dir"], in: dir).trimmingCharacters(in: .whitespacesAndNewlines)
        func has(_ name: String) -> Bool { FileManager.default.fileExists(atPath: (gitDir as NSString).appendingPathComponent(name)) }
        // Otherwise git's "already a rebase in progress" would look like our own stop below.
        if let busy = ["rebase-merge", "rebase-apply", "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"].first(where: has) {
            throw GitError(message: busy.hasPrefix("rebase") ? "A rebase is already in progress. Continue or abort it first."
                                                             : "A \(busy == "MERGE_HEAD" ? "merge" : busy == "REVERT_HEAD" ? "revert" : "cherry-pick") is in progress. Finish or abort it first.")
        }
        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-rebase-\(UUID().uuidString)").path
        try FileManager.default.createDirectory(atPath: tmp, withIntermediateDirectories: true)
        let built = try RebaseTodo.build(plan, messageDir: tmp)
        let todo = (tmp as NSString).appendingPathComponent("todo")
        try built.todo.write(toFile: todo, atomically: true, encoding: .utf8)
        for (name, m) in built.messages { try m.write(toFile: (tmp as NSString).appendingPathComponent(name), atomically: true, encoding: .utf8) }
        let env = ["GIT_SEQUENCE_EDITOR": "cp " + RebaseTodo.shellQuoted(todo), "GIT_EDITOR": "true"]
        var failure: Error?
        do { try await run(["rebase", "-i", "--autostash", "--no-autosquash", range.base ?? "--root"], in: dir, env: env) }
        catch { failure = error }
        let stopped = has("rebase-merge")
        // The exec lines still to run read their messages from here; keep it until the rebase is over.
        if !stopped { try? FileManager.default.removeItem(atPath: tmp) }
        if let failure, !stopped { throw failure }
        return stopped
    }
}

/// The last history rewrite Loupe started per worktree, for an Undo: `git reset --keep <sha>` puts the branch
/// back where it was. Also posted as `.historyRewritten` (object: the worktree path, userInfo "label" and "sha").
@MainActor enum Rewrites {
    private static var last: [String: (label: String, sha: String)] = [:]

    static func lastRewrite(in worktree: String) -> (label: String, sha: String)? { last[worktree] }
    static func forget(_ worktree: String) { last[worktree] = nil }

    static func record(_ label: String, sha: String, in worktree: String) {
        last[worktree] = (label, sha)
        NotificationCenter.default.post(name: .historyRewritten, object: worktree, userInfo: ["label": label, "sha": sha])
    }
}

extension Notification.Name {
    static let historyRewritten = Notification.Name("loupe.historyRewritten")
}

// MARK: - State behind the History UI

@MainActor @Observable
final class Rebaser {
    struct Pending: Identifiable {
        let id = UUID()
        var label: String
        var range: RebaseRange
        var plan: [RebaseItem]
        var pushed: [RebaseItem]
        var dropping: RebaseItem?
    }
    struct Planner: Identifiable {
        let id = UUID()
        var range: RebaseRange
        var items: [RebaseItem]      // newest first, as History shows them
    }
    struct Reword: Identifiable {
        let id = UUID()
        var sha: String
        var text: String
    }

    // weak: a rebase still running when the worktree goes away must not touch a freed model
    weak var history: HistoryModel?
    let path: String
    /// HEAD's first-parent commits back to the first merge (a rebase can't cross it) and short of the root: these can
    /// be dragged. Pushed ones too: `rewrite` asks before rewriting what a remote already has.
    var local: Set<String> = []
    var dragging: String?
    var dropTarget: (sha: String, zone: RebaseTodo.Zone, fixup: Bool)?
    var pending: Pending?
    var planner: Planner?
    var reword: Reword?
    var running = false

    init(history: HistoryModel) { self.history = history; path = history.path }

    func refreshLocal() async {
        let out = (try? await Git.string(["rev-list", "--first-parent", "--parents", "-n", "\(Git.rebaseLimit)", "HEAD", "--"], in: path)) ?? ""
        var set = Set<String>()
        for line in out.split(separator: "\n") {
            let ids = line.split(separator: " ")   // the commit, then its parents
            guard ids.count == 2 else { break }    // a merge (or the root) ends what a rebase can reach
            set.insert(String(ids[0]))
        }
        local = set
    }

    /// Loads `oldest^..HEAD`, applies `change` and runs it, asking first when it drops a commit or rewrites pushed ones.
    func rewrite(_ label: String, from oldest: String, _ change: @escaping ([RebaseItem]) -> [RebaseItem]) async {
        do {
            let range = try await Git.rebaseRange(from: oldest, in: path)
            submit(label, range: range, plan: change(range.items))
        } catch { history?.error = error.localizedDescription }
    }

    func submit(_ label: String, range: RebaseRange, plan: [RebaseItem], confirmed: Bool = false) {
        guard RebaseTodo.firstChange(original: range.items.map(\.sha), plan: plan) != nil else { return }
        do { _ = try RebaseTodo.build(plan, messageDir: "/") } catch { history?.error = error.localizedDescription; return }
        let pushed = RebaseTodo.rewrittenPushed(range, plan: plan)
        let dropping = plan.filter { $0.action == .drop }
        if !confirmed, !pushed.isEmpty || !dropping.isEmpty {
            pending = Pending(label: label, range: range, plan: plan, pushed: pushed, dropping: dropping.count == 1 ? dropping[0] : nil)
            return
        }
        Task { await execute(label, range: range, plan: plan) }
    }

    private func execute(_ label: String, range: RebaseRange, plan: [RebaseItem]) async {
        guard !running else { return }
        running = true; defer { running = false }
        do {
            let head = try await Git.string(["rev-parse", "HEAD"], in: path).trimmingCharacters(in: .whitespacesAndNewlines)
            Rewrites.record(label, sha: head, in: path)
            // An edit or a conflict waits in Changes, where the banner offers Continue / Abort.
            // A rebase that finishes here is undone by Edit › Undo (reset --keep to `head`); one that stops is
            // recorded by Continue in Changes, from the rebase's orig-head.
            var stopped = false
            try await UndoStack.for(path).run(.reset(label, from: head)) {
                stopped = try await Git.runRebase(range, plan: plan, in: path)
            }
            if stopped { history?.stoppedOnConflicts = true }
        } catch { history?.error = error.localizedDescription }
        await history?.reload()
        await refreshLocal()
    }

    // MARK: Entry points

    func drop(_ sha: String, zone: RebaseTodo.Zone, onto target: String, fixup: Bool) async {
        guard let oldest = history?.commits.last(where: { $0.sha == sha || $0.sha == target })?.sha else { return }
        let label = zone == .onto ? (fixup ? "Fixup" : "Squash") : "Reorder Commits"
        await rewrite(label, from: oldest) { RebaseTodo.move($0, sha, to: zone, of: target, fixup: fixup) }
    }

    func set(_ action: RebaseAction, _ c: Commit) async {
        await rewrite(action.title, from: c.sha) { items in items.map { var i = $0; if i.sha == c.sha { i.action = action }; return i } }
    }

    /// Squash or fixup into the parent: the parent is the oldest commit rewritten.
    func meldIntoParent(_ c: Commit, fixup: Bool) async {
        guard let p = c.parents.first else { return }
        await rewrite(fixup ? "Fixup into Parent" : "Squash into Parent", from: p) { items in
            items.map { var i = $0; if i.sha == c.sha { i.action = fixup ? .fixup : .squash }; return i }
        }
    }

    /// Swaps the commit with its child (up) or its parent (down).
    func shift(_ c: Commit, up: Bool) async {
        if up {
            guard let commits = history?.commits, let i = commits.firstIndex(where: { $0.sha == c.sha }), i > 0 else { return }
            let child = commits[i - 1].sha
            await rewrite("Move Commit Up", from: c.sha) { RebaseTodo.move($0, c.sha, to: .above, of: child) }
        } else {
            guard let p = c.parents.first else { return }
            await rewrite("Move Commit Down", from: p) { RebaseTodo.move($0, c.sha, to: .below, of: p) }
        }
    }

    func beginReword(_ c: Commit) async {
        let m = (try? await Git.string(["show", "-s", "--format=%B", c.sha], in: path)) ?? c.subject
        reword = Reword(sha: c.sha, text: m.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    func finishReword(_ r: Reword) async {
        await rewrite("Reword", from: r.sha) { items in
            items.map { var i = $0; if i.sha == r.sha { i.action = .reword; i.newMessage = r.text }; return i }
        }
    }

    func openPlanner(from c: Commit) async {
        do {
            let range = try await Git.rebaseRange(from: c.sha, in: path)
            planner = Planner(range: range, items: range.items.reversed())
        } catch { history?.error = error.localizedDescription }
    }
}

// MARK: - Views

/// What a History row needs for drag-and-drop: commits no remote has can be dragged above or below one another,
/// or dropped onto one to squash into it (⌥ to fixup).
struct RebaseDragRow: ViewModifier {
    @Bindable var rebaser: Rebaser
    var commit: Commit
    var enabled: Bool
    @State private var height: CGFloat = 40

    func body(content: Content) -> some View {
        let movable = enabled && rebaser.local.contains(commit.sha) && commit.parents.count < 2
        if movable {
            content
                .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
                .onDrag {
                    rebaser.dragging = commit.sha
                    return NSItemProvider(object: commit.sha as NSString)
                }
                .onDrop(of: [.text], delegate: Delegate(rebaser: rebaser, target: commit.sha, height: height))
                .overlay { indicator }
        } else {
            content
        }
    }

    @ViewBuilder private var indicator: some View {
        if let t = rebaser.dropTarget, t.sha == commit.sha {
            switch t.zone {
            case .above: VStack { Rectangle().fill(Color.accentColor).frame(height: 2); Spacer() }.allowsHitTesting(false)
            case .below: VStack { Spacer(); Rectangle().fill(Color.accentColor).frame(height: 2) }.allowsHitTesting(false)
            case .onto:
                RoundedRectangle(cornerRadius: 5).strokeBorder(Color.accentColor, lineWidth: 2)
                    .background(Color.accentColor.opacity(0.1), in: RoundedRectangle(cornerRadius: 5))
                    .overlay(alignment: .trailing) {
                        Text(t.fixup ? "Fixup" : "Squash")
                            .font(.caption2.weight(.semibold)).foregroundStyle(.white)
                            .padding(.horizontal, 6).padding(.vertical, 2)
                            .background(Color.accentColor, in: Capsule()).padding(.trailing, 6)
                    }
                    .allowsHitTesting(false)
            }
        }
    }

    struct Delegate: DropDelegate {
        var rebaser: Rebaser
        var target: String
        var height: CGFloat

        private func zone(_ info: DropInfo) -> RebaseTodo.Zone {
            let y = info.location.y
            return y < height * 0.3 ? .above : y > height * 0.7 ? .below : .onto
        }
        func validateDrop(info: DropInfo) -> Bool {
            guard let d = rebaser.dragging else { return false }
            return d != target && rebaser.local.contains(d) && !rebaser.running
        }
        func dropUpdated(info: DropInfo) -> DropProposal? {
            guard validateDrop(info: info) else { return DropProposal(operation: .forbidden) }
            let z = zone(info), f = NSEvent.modifierFlags.contains(.option)
            if let t = rebaser.dropTarget, t.sha == target, t.zone == z, t.fixup == f {} else { rebaser.dropTarget = (target, z, f) }
            return DropProposal(operation: .move)
        }
        func dropExited(info: DropInfo) { if rebaser.dropTarget?.sha == target { rebaser.dropTarget = nil } }
        func performDrop(info: DropInfo) -> Bool {
            defer { rebaser.dropTarget = nil; rebaser.dragging = nil }
            guard validateDrop(info: info), let d = rebaser.dragging else { return false }
            // `dragging` outlives a drag dropped elsewhere (no end callback), so only act when the payload is that commit.
            guard let provider = info.itemProviders(for: [.text]).first else { return false }
            let z = zone(info), fixup = NSEvent.modifierFlags.contains(.option), rebaser = rebaser, target = target
            _ = provider.loadObject(ofClass: NSString.self) { s, _ in
                let sha = (s as? NSString).map { $0 as String }
                Task { @MainActor in if sha == d { await rebaser.drop(d, zone: z, onto: target, fixup: fixup) } }
            }
            return true
        }
    }
}

/// The rewrite items of a History commit's context menu.
struct RebaseMenu: View {
    var rebaser: Rebaser
    var commit: Commit
    var isHead: Bool

    var body: some View {
        let c = commit, linear = c.parents.count < 2, hasParent = c.parents.count == 1
        Group {
            Button("Reword…") { Task { await rebaser.beginReword(c) } }.disabled(!linear)
            Button("Edit (Stop Here)") { Task { await rebaser.set(.edit, c) } }.disabled(!linear)
            Button("Squash into Parent") { Task { await rebaser.meldIntoParent(c, fixup: false) } }.disabled(!hasParent)
            Button("Fixup into Parent") { Task { await rebaser.meldIntoParent(c, fixup: true) } }.disabled(!hasParent)
            Button("Move Up") { Task { await rebaser.shift(c, up: true) } }.disabled(isHead || !linear)
            Button("Move Down") { Task { await rebaser.shift(c, up: false) } }.disabled(!hasParent)
            Button("Drop…", role: .destructive) { Task { await rebaser.set(.drop, c) } }.disabled(!linear)
            Divider()
            Button("Interactive Rebase from Here…") { Task { await rebaser.openPlanner(from: c) } }
        }
        .disabled(rebaser.running)
    }
}

/// The sheets and confirmations behind History's rewrite actions.
struct RebaseSheets: ViewModifier {
    @Bindable var rebaser: Rebaser

    func body(content: Content) -> some View {
        content
            .sheet(item: $rebaser.planner) { p in RebasePlannerSheet(rebaser: rebaser, planner: p) }
            .sheet(item: $rebaser.reword) { r in RewordSheet(rebaser: rebaser, reword: r) }
            .confirmationDialog(title, isPresented: Binding(get: { rebaser.pending != nil }, set: { if !$0 { rebaser.pending = nil } })) {
                if let p = rebaser.pending {
                    Button(p.dropping != nil && p.pushed.isEmpty ? "Drop Commit" : "Rewrite Anyway", role: .destructive) {
                        rebaser.pending = nil
                        rebaser.submit(p.label, range: p.range, plan: p.plan, confirmed: true)
                    }
                }
            } message: { Text(message) }
    }

    private var title: String {
        guard let p = rebaser.pending else { return "" }
        if !p.pushed.isEmpty { return "Rewrite \(p.pushed.count == 1 ? "a pushed commit" : "\(p.pushed.count) pushed commits")?" }
        return "Drop “\(p.dropping?.subject ?? "")”?"
    }

    private var message: String {
        guard let p = rebaser.pending else { return "" }
        if !p.pushed.isEmpty {
            let list = p.pushed.prefix(8).map { "\($0.short) \($0.subject)" }.joined(separator: "\n")
            return "A remote already has these commits:\n\(list)\(p.pushed.count > 8 ? "\n…" : "")\n\nAfter the rewrite the branch needs a force push, and anyone who built on them has to rebase."
        }
        return "Its changes leave the branch. Undo can bring it back while Loupe is open; after that, git reflog still has it."
    }
}

struct RewordSheet: View {
    var rebaser: Rebaser
    @State var reword: Rebaser.Reword
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Reword \(reword.sha.prefix(8))").font(.headline)
            TextEditor(text: $reword.text).font(.body.monospaced()).frame(minWidth: 480, minHeight: 160)
                .scrollContentBackground(.hidden).padding(4)
                .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 5))
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Reword") { let r = reword; dismiss(); Task { await rebaser.finishReword(r) } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(reword.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(16)
    }
}

/// `oldest^..HEAD` with an action per commit, newest on top like History. Drag rows to reorder.
struct RebasePlannerSheet: View {
    var rebaser: Rebaser
    @State var planner: Rebaser.Planner
    @Environment(\.dismiss) private var dismiss

    private var plan: [RebaseItem] { planner.items.reversed() }   // oldest first, as git applies it
    private var problem: String? {
        do { _ = try RebaseTodo.build(plan, messageDir: "/"); return nil } catch { return error.localizedDescription }
    }

    var body: some View {
        let pushed = RebaseTodo.rewrittenPushed(planner.range, plan: plan)
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Interactive Rebase").font(.headline)
                Text("\(planner.items.count) commit\(planner.items.count == 1 ? "" : "s") onto \(planner.range.base.map { String($0.prefix(8)) } ?? "the root"). Newest on top; drag to reorder. Squash and fixup meld into the commit below.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .padding(14)
            Divider()
            List {
                ForEach($planner.items) { $item in PlannerRow(item: $item, pushed: planner.range.pushed.contains(item.sha)) }
                    .onMove { planner.items.move(fromOffsets: $0, toOffset: $1) }
            }
            .listStyle(.inset)
            Divider()
            HStack(spacing: 8) {
                if let problem {
                    Label(problem, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.orange).lineLimit(2)
                } else if !pushed.isEmpty {
                    Label("Rewrites \(pushed.count) pushed commit\(pushed.count == 1 ? "" : "s"): \(pushed.prefix(3).map(\.short).joined(separator: ", "))\(pushed.count > 3 ? "…" : "")",
                          systemImage: "exclamationmark.triangle.fill").foregroundStyle(.orange).lineLimit(2)
                }
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Start Rebase") {
                    let p = planner
                    dismiss()
                    // Confirmed: the footer already shows what's pushed.
                    rebaser.submit("Interactive Rebase", range: p.range, plan: p.items.reversed(), confirmed: true)
                }
                .keyboardShortcut(.defaultAction)
                .disabled(problem != nil || RebaseTodo.firstChange(original: planner.range.items.map(\.sha), plan: plan) == nil)
            }
            .font(.callout)
            .padding(12)
        }
        .frame(minWidth: 620, idealWidth: 700, minHeight: 420, idealHeight: 520)
    }
}

private struct PlannerRow: View {
    @Binding var item: RebaseItem
    var pushed: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Picker("", selection: $item.action) { ForEach(RebaseAction.allCases) { Text($0.title).tag($0) } }
                    .labelsHidden().fixedSize()
                Text(item.short).font(.caption.monospaced()).foregroundStyle(.secondary)
                Text(item.newMessage.map { String($0.prefix { $0 != "\n" }) } ?? item.subject).lineLimit(1)
                    .strikethrough(item.action == .drop).foregroundStyle(item.action == .drop ? .secondary : .primary)
                Spacer(minLength: 4)
                if pushed { Image(systemName: "cloud").foregroundStyle(.secondary).help("A remote already has this commit") }
            }
            if item.action == .reword || item.action == .squash {
                TextEditor(text: Binding(get: { item.effectiveMessage }, set: { item.newMessage = $0 }))
                    .font(.callout.monospaced()).frame(height: 64)
                    .scrollContentBackground(.hidden).padding(3)
                    .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 4))
                    .help(item.action == .squash ? "This commit's part of the combined message" : "The new message")
            }
        }
        .padding(.vertical, 3)
    }
}

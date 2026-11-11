import SwiftUI
import AppKit

// MARK: - Undo
//
// Edit › Undo <action> (⌘Z outside text fields) and the "… Undo" toast take back what Loupe did to a worktree.
// Every undo is made of git objects saved before the action, so nothing is lost: blobs for file contents
// (`hash-object -w`), a tree for the index (`write-tree`), the commit HEAD was on, a stash's commit.
//
// One `UndoStack` per worktree (`model.undo`, or `UndoStack.for(path)` where only the path is at hand), at most 50
// entries. Each entry remembers the branch and HEAD it left; once HEAD moves some other way (a commit in a
// terminal), the entries above it no longer apply and are dropped. An undo that would rewrite pushed history is
// refused with a message (and a Revert offer for a single commit); one whose files changed since is refused and
// dropped.
//
// For other features (interactive rebase, merge editor), two ways in:
//
//     // A history-changing command: undo = `reset --keep` to where HEAD was before it, recorded only if the
//     // command finished (no rebase/merge left stopped) and moved HEAD.
//     try await model.undo.run(.reset("Rebase onto main")) { try await Git.run(["rebase", "main"], in: path) }
//     // Files it rewrites: their contents (and the index) come back, if nobody touched them since.
//     try await model.undo.run(.files("Resolve conflict in a.swift", ["a.swift"], index: true)) { … }
//
//     // Anything else: do the work, then record how to take it back.
//     await model.recordUndo("Rename Branch") { try await Git.run(["branch", "-m", new, old], in: path) }
//
// A rebase or merge that stops on conflicts is recorded when Loupe continues it (`finishOperation`).

@MainActor @Observable
final class UndoStack {
    struct Entry: Identifiable {
        let id = UUID()
        let label: String              // "Discard of a.txt": the menu says "Undo Discard of a.txt"
        let head: String               // `key(branch:oid:)` after the action
        let published: String?         // the commit undoing removes; refused once a remote branch has it
        let offerRevert: Bool          // refused for being pushed → offer `git revert` of `published`
        let restore: @MainActor () async throws -> Void
    }
    struct Toast: Equatable {
        let id = UUID()
        let text: String
        let entry: UUID?               // nil: it reports an undo
    }

    /// How an action is taken back; `run(_:_:)` saves what's needed before the action and records it after.
    enum Recipe {
        /// History moved: `reset --keep` (`--soft` keeps the changes staged) to HEAD from before, or `from`.
        case reset(String, soft: Bool = false, published: Bool = true, offerRevert: Bool = false, from: String? = nil,
                   then: (@MainActor () -> Void)? = nil)
        /// File contents (and with `index`, the whole index) back to what they were before.
        case files(String, [String], index: Bool = false, toast: Bool = true)
        /// A stash commit is stored again; `applied`: what applying it changed is taken back first.
        case stash(String, sha: String, message: String, applied: Bool, dropped: Bool)
    }

    static let limit = 50
    let path: String
    private(set) var entries: [Entry] = []
    var toast: Toast?
    @ObservationIgnored private var inFlight = 0   // an action is between its git command and its record: don't prune
    @ObservationIgnored private var undoing = false

    private static var stacks: [String: UndoStack] = [:]
    static func `for`(_ path: String) -> UndoStack {
        if let s = stacks[path] { return s }
        let s = UndoStack(path: path)
        stacks[path] = s
        return s
    }
    init(path: String) { self.path = path }

    var title: String { entries.last.map { "Undo \($0.label)" } ?? "Undo" }

    // MARK: Recording

    func record(_ label: String, published: String? = nil, offerRevert: Bool = false, toast: Bool = false,
                restore: @escaping @MainActor () async throws -> Void) async {
        inFlight += 1; defer { inFlight -= 1 }
        let e = Entry(label: label, head: await Self.head(in: path), published: published, offerRevert: offerRevert, restore: restore)
        entries.append(e)
        if entries.count > Self.limit { entries.removeFirst(entries.count - Self.limit) }
        self.toast = toast ? Toast(text: label, entry: e.id) : nil
    }

    /// Runs `op` with nothing pruned meanwhile; `record` inside it sees the state `op` left.
    func track<T>(_ op: () async throws -> T) async rethrows -> T {
        inFlight += 1; defer { inFlight -= 1 }
        return try await op()
    }

    /// Runs `op` (the action) and records its undo per `recipe`; nil just runs it. Nothing is recorded if `op` throws.
    func run(_ recipe: Recipe?, _ op: () async throws -> Void) async throws {
        guard let recipe else { return try await op() }
        let dir = path
        try await track {
            switch recipe {
            case let .reset(label, soft, published, offerRevert, from, then):
                var before = from
                if before == nil { before = await Self.oid(in: dir) }
                try await op()
                let after = await Self.oid(in: dir), stopped = await Self.operationInProgress(in: dir)
                guard let after, after != before, !stopped else { return }
                if before == nil && !soft { return }   // nothing to reset --keep to
                await record(label, published: published ? after : nil, offerRevert: offerRevert) {
                    if let before { try await Git.run(["reset", soft ? "--soft" : "--keep", before], in: dir) }
                    else { try await Git.run(["update-ref", "-d", "HEAD"], in: dir) }   // the first commit: back to unborn
                    then?()
                }
            case let .files(label, paths, index, toast):
                var snap = try await FileSnapshot.capture(paths, index: index, in: dir)
                try await op()
                try await snap.seal(in: dir)
                guard !snap.isEmpty else { return }
                let s = snap
                await record(label, toast: toast) { try await s.restore(in: dir, label: label) }
            case let .stash(label, sha, message, applied, dropped):
                var snap: FileSnapshot?
                if applied {
                    let names = try await Git.string(["stash", "show", "--name-only", "--no-renames", "--include-untracked", sha], in: dir)
                    snap = try await FileSnapshot.capture(names.split(separator: "\n").map(String.init), index: true, in: dir)
                }
                try await op()
                try await snap?.seal(in: dir)
                let s = snap
                await record(label, toast: dropped) {
                    let store = { if dropped { try await Git.run(["stash", "store", "-m", message, sha], in: dir) } }
                    do { try await s?.restore(in: dir, label: label) } catch var r as UndoRefusal where dropped {
                        try await store()   // the files can't go back, but the stash still can
                        r.message += " The stash is back in the list."
                        throw r
                    }
                    try await store()
                }
            }
        }
    }

    // MARK: Undoing

    /// Takes back the newest entry that still applies. Throws `UndoRefusal` when that's unsafe.
    func undo() async throws {
        guard !undoing else { return }   // a second ⌘Z while the first runs would restore the same entry twice
        undoing = true; inFlight += 1; defer { undoing = false; inFlight -= 1 }
        drop(unless: await Self.head(in: path))
        guard let e = entries.last else { throw UndoRefusal(message: "Nothing to undo in this worktree.") }
        if let sha = e.published,
           let on = String(decoding: try await Git.run(["branch", "-r", "--contains", sha], in: path, allowFailure: true), as: UTF8.self)
            .split(separator: "\n").first?.trimmingCharacters(in: .whitespaces) {
            throw UndoRefusal(message: "\(e.label) is already on \(on). Undoing it would rewrite pushed history."
                              + (e.offerRevert ? " Revert it instead: a new commit that takes it back." : ""),
                              revert: e.offerRevert ? sha : nil)
        }
        do { try await e.restore() } catch let r as UndoRefusal where r.drop { entries.removeAll { $0.id == e.id }; throw r }
        entries.removeAll { $0.id == e.id }
        toast = Toast(text: "Undone: \(e.label)", entry: nil)
    }

    /// Status refresh: HEAD (or the branch) moved without Loupe, so the newest entries no longer apply.
    func prune(branch: String?, oid: String) {
        guard inFlight == 0 else { return }
        drop(unless: Self.key(branch: branch, oid: oid))
    }

    private func drop(unless head: String) {
        guard entries.last.map({ $0.head != head }) == true else { return }
        while let e = entries.last, e.head != head { entries.removeLast() }
        if let t = toast?.entry, !entries.contains(where: { $0.id == t }) { toast = nil }
    }

    // MARK: Git state

    nonisolated static func key(branch: String?, oid: String) -> String { "\(branch ?? "HEAD") \(oid == "(initial)" ? "" : oid)" }

    nonisolated static func oid(in dir: String) async -> String? {
        let s = String(decoding: (try? await Git.run(["rev-parse", "-q", "--verify", "HEAD"], in: dir, allowFailure: true)) ?? Data(), as: UTF8.self)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return s.isEmpty ? nil : s
    }

    nonisolated static func head(in dir: String) async -> String {
        let b = String(decoding: (try? await Git.run(["symbolic-ref", "-q", "--short", "HEAD"], in: dir, allowFailure: true)) ?? Data(), as: UTF8.self)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return key(branch: b.isEmpty ? nil : b, oid: await oid(in: dir) ?? "")
    }

    /// A rebase, merge, cherry-pick or revert stopped partway (an interactive rebase's "edit" exits 0 there).
    nonisolated static func operationInProgress(in dir: String) async -> Bool {
        guard let git = try? await Git.string(["rev-parse", "--path-format=absolute", "--git-dir"], in: dir) else { return false }
        let d = git.trimmingCharacters(in: .whitespacesAndNewlines)
        return ["rebase-merge", "rebase-apply", "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"]
            .contains { FileManager.default.fileExists(atPath: (d as NSString).appendingPathComponent($0)) }
    }

    /// Where HEAD was before `op` started, read while it's stopped: a rebase keeps it in orig-head; the others
    /// haven't committed yet, so it's HEAD.
    nonisolated static func preOperationHead(_ op: GitOperation, in dir: String) async -> String? {
        if op == .rebase, let git = try? await Git.string(["rev-parse", "--path-format=absolute", "--git-dir"], in: dir) {
            let d = git.trimmingCharacters(in: .whitespacesAndNewlines)
            for sub in ["rebase-merge", "rebase-apply"] {
                if let s = try? String(contentsOfFile: "\(d)/\(sub)/orig-head", encoding: .utf8) { return s.trimmingCharacters(in: .whitespacesAndNewlines) }
            }
            return nil
        }
        return await oid(in: dir)
    }
}

struct UndoRefusal: LocalizedError {
    var message: String
    var revert: String? = nil          // a commit to offer `git revert` for instead
    var drop = false                   // the entry can never apply again
    var errorDescription: String? { message }
}

extension GitOperation {
    var undoLabel: String { switch self { case .merge: "Merge"; case .cherryPick: "Cherry-pick"; case .revert: "Revert"; case .rebase: "Rebase" } }
}

// MARK: - File snapshot

/// Some files' contents (and optionally the index) before and after an action, kept as git objects.
struct FileSnapshot: Sendable {
    struct File: Sendable {
        var path: String
        var before: Content?, after: Content?   // nil: no file
    }
    enum Content: Sendable, Equatable { case blob(String, executable: Bool), link(String) }
    var files: [File]
    var indexBefore: String?, indexAfter: String?

    var isEmpty: Bool { files.isEmpty && indexBefore == indexAfter }

    static func capture(_ paths: [String], index: Bool, in dir: String) async throws -> FileSnapshot {
        let unique = Array(Set(paths)).sorted()
        let before = try await contents(unique, write: true, in: dir)
        return FileSnapshot(files: zip(unique, before).map { File(path: $0, before: $1) },
                            indexBefore: index ? await tree(in: dir) : nil)
    }

    /// Records the state the action left, which undo checks is still there.
    mutating func seal(in dir: String) async throws {
        let after = try await Self.contents(files.map(\.path), write: false, in: dir)
        for i in files.indices { files[i].after = after[i] }
        files.removeAll { $0.before == $0.after }
        if indexBefore != nil { indexAfter = await Self.tree(in: dir) }
    }

    func restore(in dir: String, label: String) async throws {
        let now = try await Self.contents(files.map(\.path), write: false, in: dir)
        let changed = zip(files, now).filter { $0.0.after != $0.1 }.map(\.0.path)
        let indexMoved = indexBefore != indexAfter
        let indexNow = indexMoved ? await Self.tree(in: dir) : indexAfter
        if !changed.isEmpty || indexNow != indexAfter {
            let what = changed.isEmpty ? "The staged changes" : changed.count == 1 ? changed[0] : "\(changed.count) files"
            throw UndoRefusal(message: "\(what) changed since \(label), so undoing it would overwrite that. Nothing was changed.", drop: true)
        }
        let fm = FileManager.default
        for f in files {
            let url = URL(fileURLWithPath: dir).appendingPathComponent(f.path)
            if (try? fm.destinationOfSymbolicLink(atPath: url.path)) != nil || fm.fileExists(atPath: url.path) { try fm.removeItem(at: url) }
            switch f.before {
            case nil: break
            case .link(let target)?:
                try fm.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
                try fm.createSymbolicLink(atPath: url.path, withDestinationPath: target)
            case .blob(let sha, let x)?:
                try fm.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
                try await Git.run(["cat-file", "blob", sha], in: dir).write(to: url)
                if x { try fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path) }
            }
        }
        if indexMoved, let t = indexBefore { try await Git.run(["read-tree", "-m", t], in: dir) }   // -m keeps the stat cache of unchanged files
    }

    /// Each path's content; `write` stores blobs. Raw bytes (`--no-filters`): an LFS or CRLF filter must not
    /// turn the saved copy into something else. Directories (submodules) are skipped as absent.
    private static func contents(_ paths: [String], write: Bool, in dir: String) async throws -> [Content?] {
        let fm = FileManager.default
        var result = [Content?](repeating: nil, count: paths.count)
        var regular: [Int] = []
        for (i, p) in paths.enumerated() {
            let full = (dir as NSString).appendingPathComponent(p)
            if let target = try? fm.destinationOfSymbolicLink(atPath: full) { result[i] = .link(target); continue }
            var isDir: ObjCBool = false
            if fm.fileExists(atPath: full, isDirectory: &isDir), !isDir.boolValue { regular.append(i) }
        }
        guard !regular.isEmpty else { return result }
        let out = try await Git.string(["hash-object", "--no-filters"] + (write ? ["-w"] : []) + ["--"] + regular.map { paths[$0] }, in: dir)
        for (i, sha) in zip(regular, out.split(separator: "\n")) {
            let full = (dir as NSString).appendingPathComponent(paths[i])
            result[i] = .blob(String(sha), executable: fm.isExecutableFile(atPath: full))
        }
        return result
    }

    private static func tree(in dir: String) async -> String? {   // nil with conflicts in the index
        (try? await Git.string(["write-tree"], in: dir))?.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

// MARK: - Worktree model

extension WorktreeModel {
    var undo: UndoStack { UndoStack.for(path) }

    /// The public way in for an action without a recipe: call after it succeeded.
    func recordUndo(_ label: String, published: String? = nil, offerRevert: Bool = false, toast: Bool = false,
                    restore: @escaping @MainActor () async throws -> Void) async {
        await undo.record(label, published: published, offerRevert: offerRevert, toast: toast, restore: restore)
    }

    /// Edit › Undo and the toast's Undo button.
    func undoLast() async {
        var refusal: UndoRefusal?
        await perform("Undoing") {
            do { try await undo.undo() } catch let r as UndoRefusal { refusal = r }
        }
        await history.reload()
        guard let r = refusal else { return }
        let a = NSAlert()
        a.messageText = "Can't Undo"
        a.informativeText = r.message
        if let sha = r.revert {
            a.addButton(withTitle: "Revert Commit")
            a.addButton(withTitle: "Cancel")
            guard a.runModal() == .alertFirstButtonReturn else { return }
            await perform("Reverting") { try await Git.run(["revert", "--no-edit", sha], in: path) }
            await history.reload()
        } else { a.runModal() }
    }

    static func filesLabel(_ verb: String, _ fs: [FileChange]) -> String {
        fs.count == 1 ? "\(verb) \((fs[0].path as NSString).lastPathComponent)" : "\(verb) \(fs.count) Files"
    }
}

// MARK: - Menu and toast

/// Edit › Undo / Redo. A text field keeps its own typing undo; anywhere else ⌘Z undoes the worktree's last action.
struct UndoCommands: View {
    let app: AppModel

    var body: some View {
        Button(app.selection.map { UndoStack.for($0).title } ?? "Undo") {
            if NSApp.keyWindow?.firstResponder is NSText { NSApp.sendAction(Selector(("undo:")), to: nil, from: nil); return }
            guard let sel = app.selection, !UndoStack.for(sel).entries.isEmpty else { NSSound.beep(); return }
            let m = app.model(for: sel)
            Task { await m.undoLast() }
        }
        .keyboardShortcut("z")
        // ponytail: no git redo; Redo is the text fields' only. Add one when someone misses it.
        Button("Redo") { NSApp.sendAction(Selector(("redo:")), to: nil, from: nil) }
            .keyboardShortcut("z", modifiers: [.command, .shift])
    }
}

/// "Discard of a.txt · Undo" after a destructive action, "Undone: …" after an undo; gone after a few seconds.
struct UndoToast: View {
    let model: WorktreeModel

    var body: some View {
        let stack = model.undo
        ZStack {
            if let t = stack.toast {
                HStack(spacing: 10) {
                    Image(systemName: t.entry == nil ? "arrow.uturn.backward.circle.fill" : "checkmark.circle.fill")
                        .foregroundStyle(.secondary)
                    Text(t.text).lineLimit(1).truncationMode(.middle)
                    if let id = t.entry, stack.entries.last?.id == id {
                        Button("Undo") { Task { await model.undoLast() } }
                            .buttonStyle(.borderless).fontWeight(.semibold)
                            .help("Undo (⌘Z)")
                    }
                }
                .font(.callout)
                .padding(.horizontal, 14).padding(.vertical, 8)
                .background(.regularMaterial, in: Capsule())
                .overlay(Capsule().strokeBorder(.separator))
                .shadow(color: .black.opacity(0.15), radius: 8, y: 2)
                .padding(.bottom, 18)
                .transition(.move(edge: .bottom).combined(with: .opacity))
                .task(id: t.id) {
                    try? await Task.sleep(for: .seconds(6))
                    if stack.toast?.id == t.id { withAnimation(.easeOut(duration: 0.2)) { stack.toast = nil } }
                }
            }
        }
        .animation(.easeOut(duration: 0.2), value: stack.toast)
    }
}

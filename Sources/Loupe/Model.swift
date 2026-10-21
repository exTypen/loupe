import Foundation
import Observation
import CoreServices
import AppKit

@MainActor @Observable
final class AppModel {
    var repoPaths: [String] = UserDefaults.standard.stringArray(forKey: "repos") ?? [] {
        didSet { UserDefaults.standard.set(repoPaths, forKey: "repos") }
    }
    var worktrees: [String: [Worktree]] = [:]          // repo path → worktrees (main first)
    var selection: String? = UserDefaults.standard.string(forKey: "selection") {   // selected worktree path
        didSet {
            UserDefaults.standard.set(selection, forKey: "selection")
            if let s = selection { recent = [s] + recent.filter { $0 != s }.prefix(29) }
        }
    }
    var recent: [String] = UserDefaults.standard.stringArray(forKey: "recent") ?? [] {   // worktree paths, most recent first
        didSet { UserDefaults.standard.set(recent, forKey: "recent") }
    }
    var collapsed: Set<String> = Set(UserDefaults.standard.stringArray(forKey: "collapsed") ?? []) {   // repo paths
        didSet { UserDefaults.standard.set(Array(collapsed), forKey: "collapsed") }
    }
    var showSwitcher = false
    var accounts: [ForgejoAccount] = (try? JSONDecoder().decode([ForgejoAccount].self, from: UserDefaults.standard.data(forKey: "accounts") ?? Data())) ?? [] {
        didSet { UserDefaults.standard.set(try? JSONEncoder().encode(accounts), forKey: "accounts") }
    }
    var error: String?
    var checksOverlay: (model: ChecksModel, baseURL: String)?   // window-wide CI modal
    var tokens: [String: String] = [:]                          // baseURL → token, read from the Keychain once at launch
    var tokensLoaded = false
    private var models: [String: WorktreeModel] = [:]

    init() {
        Task { await refreshAllWorktrees() }
        Task { await loadTokens() }
    }

    func loadTokens() async {
        let accts = accounts
        let found = await Task.detached(priority: .userInitiated) {
            Dictionary(accts.compactMap { a in a.readToken().map { (a.baseURL, $0) } }, uniquingKeysWith: { a, _ in a })
        }.value
        tokens.merge(found) { _, new in new }
        tokensLoaded = true
    }

    func setToken(_ token: String?, for account: ForgejoAccount) {
        account.saveToken(token)
        tokens[account.baseURL] = token
    }

    func model(for path: String) -> WorktreeModel {
        if let m = models[path] { return m }
        let m = WorktreeModel(path: path, app: self)
        models[path] = m
        return m
    }

    func repo(containing worktreePath: String) -> String? {
        worktrees.first { $0.value.contains { $0.path == worktreePath } }?.key
    }

    func addRepo(_ url: URL) async {
        do {
            // Normalize to the main worktree so adding a linked worktree adds its repo.
            let common = try await Git.string(["rev-parse", "--path-format=absolute", "--git-common-dir"], in: url.path)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let main = common.hasSuffix("/.git") ? String(common.dropLast(5)) : url.path
            if !repoPaths.contains(main) { repoPaths.append(main) }
            await refreshWorktrees(repo: main)
            selection = url.path
        } catch { self.error = "\(url.path) is not a git repository." }
    }

    func removeRepo(_ path: String) {
        repoPaths.removeAll { $0 == path }
        for wt in worktrees[path] ?? [] { models[wt.path]?.stopWatching(); models[wt.path] = nil }
        worktrees[path] = nil
    }

    func refreshAllWorktrees() async {
        await withTaskGroup(of: Void.self) { g in for r in repoPaths { g.addTask { await self.refreshWorktrees(repo: r) } } }
    }

    func refreshWorktrees(repo: String) async {
        guard let list = try? await Git.worktrees(in: repo) else { return }
        worktrees[repo] = list.filter { !$0.isBare }
        for wt in list where !wt.isBare { Task { await model(for: wt.path).refreshStatus() } }
    }

    func addWorktree(repo: String, path: String, branch: String, base: String, newBranch: Bool) async throws {
        var args = ["worktree", "add"]
        if newBranch { args += ["-b", branch, path] + (base.isEmpty ? [] : [base]) } else { args += [path, branch] }
        try await Git.run(args, in: repo)
        await refreshWorktrees(repo: repo)
        selection = path
    }

    func removeWorktree(repo: String, path: String, force: Bool) async throws {
        try await Git.run(["worktree", "remove"] + (force ? ["--force"] : []) + [path], in: repo)
        models[path]?.stopWatching(); models[path] = nil
        if selection == path { selection = repo }
        await refreshWorktrees(repo: repo)
    }

    func account(for remote: RemoteRepo) -> ForgejoAccount? {
        let host = remote.host.split(separator: ":").first.map(String.init)
        return accounts.first { $0.hostKey.split(separator: ":").first.map(String.init) == host }
    }
}

// MARK: - Per-worktree state

@MainActor @Observable
final class WorktreeModel {
    let path: String
    unowned let app: AppModel
    var branch: String?
    var headOID = ""                   // changes on commit/checkout/reset → history reloads
    var ahead = 0
    var behind = 0
    var files: [FileChange] = []
    var selectedFileID: String?            // the file whose diff is shown
    var selectedFileIDs: Set<String> = []   // the file list's selection (⌘/⇧-click, ⌘A); bulk actions use it
    var diff: FileDiff?
    var commitMessage = ""
    var amend = false
    var busy: String?                  // non-nil → an operation is running (label)
    var error: String?
    var remote: RemoteRepo?
    var remoteName = "origin"
    var remoteDetected = false
    var pulls = PullListModel()
    @ObservationIgnored lazy var history = HistoryModel(path: path)

    private var watcher: FSWatcher?
    private var refreshTask: Task<Void, Never>?
    private var diffTask: Task<Void, Never>?

    init(path: String, app: AppModel) {
        self.path = path
        self.app = app
        Task(priority: .high) { await detectRemote() }
        startWatching()
    }

    var staged: [FileChange] { files.filter(\.staged) }
    var unstaged: [FileChange] { files.filter { !$0.staged } }
    var changeCount: Int { Set(files.map(\.path)).count }
    var selectedFile: FileChange? { files.first { $0.id == selectedFileID } }

    // MARK: Status / diff

    func refreshStatus() async {
        do {
            let s = try await Git.status(in: path)
            branch = s.branch; headOID = s.oid; ahead = s.ahead; behind = s.behind
            if s.files != files { files = s.files }
            if selectedFileID == nil || !files.contains(where: { $0.id == selectedFileID }) {
                // Keep the same path selected when it moves between staged/unstaged.
                let p = selectedFileID.map { String($0.dropFirst(2)) }
                selectedFileID = files.first { $0.path == p }?.id ?? files.first?.id
            }
            selectedFileIDs.formIntersection(files.map(\.id))
            if selectedFileIDs.isEmpty, let id = selectedFileID { selectedFileIDs = [id] }
            loadDiff()
        } catch { self.error = error.localizedDescription }
    }

    func loadDiff() {
        diffTask?.cancel()
        guard let f = selectedFile else { diff = nil; return }
        diffTask = Task {
            let d = try? await Git.diff(for: f, in: path)
            guard !Task.isCancelled else { return }
            // Avoid resetting the table (and its scroll/selection) when nothing changed on disk.
            if let d, let old = diff, old.path == d.path, old.lines.map(\.text) == d.lines.map(\.text), old.headerLines == d.headerLines { return }
            diff = d
        }
    }

    func select(_ id: String?) { selectedFileID = id; selectedFileIDs = Set([id].compactMap { $0 }); loadDiff() }
    var selectedFiles: [FileChange] { files.filter { selectedFileIDs.contains($0.id) } }

    // MARK: Index operations

    func perform(_ label: String, _ op: () async throws -> Void) async {
        busy = label
        defer { busy = nil }
        do { try await op() } catch { self.error = error.localizedDescription }
        await refreshStatus()
    }

    func stage(_ fs: [FileChange]) async {
        await perform("Staging") { try await Git.run(["add", "-A", "--"] + fs.map(\.path), in: path) }
    }

    func unstage(_ fs: [FileChange]) async {
        // restore --staged fails on an unborn branch; rm --cached handles the initial commit.
        let hasHead = (try? await Git.run(["rev-parse", "--verify", "HEAD"], in: path)) != nil
        await perform("Unstaging") {
            if hasHead { try await Git.run(["restore", "--staged", "--"] + fs.flatMap { [$0.path] + ($0.origPath.map { [$0] } ?? []) }, in: path) }
            else { try await Git.run(["rm", "--cached", "-r", "-q", "--"] + fs.map(\.path), in: path) }
        }
    }

    func discard(_ fs: [FileChange]) async {
        await perform("Discarding") {
            let untracked = fs.filter { $0.kind == .untracked }.map(\.path)
            let tracked = fs.filter { $0.kind != .untracked }.map(\.path)
            if !tracked.isEmpty { try await Git.run(["restore", "--worktree", "--"] + tracked, in: path) }
            // Untracked files go to the Trash, not oblivion.
            for p in untracked { try FileManager.default.trashItem(at: URL(fileURLWithPath: path).appendingPathComponent(p), resultingItemURL: nil) }
        }
    }

    /// Stage, unstage or discard just these diff lines (indices into the shown diff's `lines`): a hunk or a selection.
    func apply(_ action: HunkAction, lines: Set<Int>) async {
        // The pane may briefly show the previous file's diff while the next loads; never act on a stale one.
        guard let d = diff, let f = selectedFile, d.path == f.path || d.oldPath == f.path else { return }
        if f.kind == .untracked { return await (action == .stage ? stage([f]) : discard([f])) }
        guard let text = d.patch(selecting: lines, reverse: action != .stage) else { return }
        let patch = Data(text.utf8)
        let args: [String]
        switch action {
        case .stage: args = ["apply", "--cached", "--whitespace=nowarn", "-"]
        case .unstage: args = ["apply", "--cached", "--reverse", "--whitespace=nowarn", "-"]
        case .discard: args = ["apply", "--reverse", "--whitespace=nowarn", "-"]
        }
        await perform(action == .stage ? "Staging" : action == .unstage ? "Unstaging" : "Discarding") { try await Git.run(args, in: path, stdin: patch) }
    }

    func commit() async {
        let msg = commitMessage.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !msg.isEmpty else { return }
        await perform("Committing") {
            try await Git.run(["commit", "-F", "-"] + (amend ? ["--amend"] : []), in: path, stdin: Data(msg.utf8))
            commitMessage = ""; amend = false
        }
    }

    func loadLastCommitMessage() async {
        if let m = try? await Git.string(["log", "-1", "--format=%B"], in: path) { commitMessage = m.trimmingCharacters(in: .whitespacesAndNewlines) }
    }

    func remoteOp(_ label: String, _ args: [String]) async {
        await perform(label) { try await Git.run(args, in: path) }
        await app.refreshWorktrees(repo: app.repo(containing: path) ?? path)
    }

    // MARK: Remote / Forgejo

    func detectRemote() async {
        defer { remoteDetected = true }
        guard let out = try? await Git.string(["remote", "-v"], in: path) else { return }
        var candidates: [(String, RemoteRepo)] = []
        for line in out.split(separator: "\n") where line.hasSuffix("(fetch)") {
            let parts = line.split(whereSeparator: { $0 == "\t" || $0 == " " })
            if parts.count >= 2, let r = RemoteRepo(remoteURL: String(parts[1])) { candidates.append((String(parts[0]), r)) }
        }
        // Prefer a remote we have an account for, then "origin".
        let pick = candidates.first { app.account(for: $0.1) != nil } ?? candidates.first { $0.0 == "origin" } ?? candidates.first
        remote = pick?.1
        remoteName = pick?.0 ?? "origin"
    }

    var client: ForgejoClient? {
        guard let r = remote, let a = app.account(for: r), let t = app.tokens[a.baseURL] else { return nil }
        return ForgejoClient(a, token: t)
    }

    /// Fetches refs/pull/N/head and checks it out in a sibling worktree (or selects the existing one).
    func checkoutInWorktree(_ pr: PullRequest) async {
        let branch = "pr/\(pr.number)"
        if let existing = app.worktrees.values.joined().first(where: { $0.branch == branch }) { app.selection = existing.path; return }
        let repo = app.repo(containing: path) ?? path
        let base = (repo as NSString).deletingLastPathComponent
        let dest = (base as NSString).appendingPathComponent("\((repo as NSString).lastPathComponent)-pr-\(pr.number)")
        await perform("Checking out PR #\(pr.number)") {
            try await Git.run(["fetch", remoteName, "+refs/pull/\(pr.number)/head:refs/remotes/\(remoteName)/pr/\(pr.number)"], in: repo)
            try await Git.run(["worktree", "add", "-B", branch, dest, "\(remoteName)/pr/\(pr.number)"], in: repo)
        }
        await app.refreshWorktrees(repo: repo)
        if FileManager.default.fileExists(atPath: dest) { app.selection = dest }
    }

    // MARK: File watching

    func startWatching() {
        Task {
            // A linked worktree's index lives under the main repo's .git/worktrees/<name>, outside `path`.
            let gitDir = (try? await Git.string(["rev-parse", "--path-format=absolute", "--git-dir"], in: path))?.trimmingCharacters(in: .whitespacesAndNewlines)
            let paths = [path] + (gitDir.map { $0.hasPrefix(path) ? [] : [$0] } ?? [])
            watcher = FSWatcher(paths: paths) { [weak self] worktreesChanged in
                Task { @MainActor in
                    guard let self else { return }
                    self.scheduleRefresh()
                    if worktreesChanged { await self.app.refreshWorktrees(repo: self.app.repo(containing: self.path) ?? self.path) }
                }
            }
        }
    }

    func stopWatching() { watcher = nil }

    private func scheduleRefresh() {
        refreshTask?.cancel()
        refreshTask = Task {
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled else { return }
            await refreshStatus()
        }
    }
}

/// FSEvents wrapper; fires on any change under the given paths (coalesced by FSEvents latency).
final class FSWatcher {
    private var stream: FSEventStreamRef?
    private let callback: (_ worktreesChanged: Bool) -> Void

    /// `worktreesChanged`: a directory directly under .git/worktrees/ appeared or vanished, i.e. a worktree was added
    /// or removed (possibly from outside Loupe). Changes deeper inside, like a linked worktree's index, don't count.
    init(paths: [String], callback: @escaping (_ worktreesChanged: Bool) -> Void) {
        self.callback = callback
        var ctx = FSEventStreamContext(version: 0, info: Unmanaged.passUnretained(self).toOpaque(), retain: nil, release: nil, copyDescription: nil)
        let cb: FSEventStreamCallback = { _, info, count, rawPaths, _, _ in
            guard let info else { return }
            let me = Unmanaged<FSWatcher>.fromOpaque(info).takeUnretainedValue()
            let paths = unsafeBitCast(rawPaths, to: NSArray.self) as? [String] ?? []
            // Object-store churn (fetch, gc) and lock files don't change what we display.
            guard paths.contains(where: { !$0.contains("/.git/objects/") && !$0.hasSuffix(".lock") }) else { return }
            me.callback(paths.contains { ($0 as NSString).deletingLastPathComponent.hasSuffix("/.git/worktrees") })
        }
        stream = FSEventStreamCreate(nil, cb, &ctx, paths as CFArray, FSEventStreamEventId(kFSEventStreamEventIdSinceNow), 0.15,
                                     FSEventStreamCreateFlags(kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagFileEvents))
        if let stream {
            FSEventStreamSetDispatchQueue(stream, DispatchQueue.global(qos: .utility))
            FSEventStreamStart(stream)
        }
    }

    deinit {
        if let stream { FSEventStreamStop(stream); FSEventStreamInvalidate(stream); FSEventStreamRelease(stream) }
    }
}

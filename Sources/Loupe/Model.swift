import Foundation
import Observation
import CoreServices
import AppKit

@MainActor @Observable
final class AppModel {
    /// Arc-style spaces: each has its own repos and accounts. Before spaces existed there was one flat list; it
    /// becomes the "default" space, whose accounts keep their old Keychain items (`ForgejoAccount.space == nil`).
    var spaces: [Space] = (try? JSONDecoder().decode([Space].self, from: UserDefaults.standard.data(forKey: "spaces") ?? Data()))
        ?? [Space(id: Space.defaultID, name: "Default", repos: UserDefaults.standard.stringArray(forKey: "repos") ?? [],
                  selection: UserDefaults.standard.string(forKey: "selection"))] {
        didSet { UserDefaults.standard.set(try? JSONEncoder().encode(spaces), forKey: "spaces") }
    }
    var currentSpace: String = UserDefaults.standard.string(forKey: "currentSpace") ?? Space.defaultID {
        didSet { UserDefaults.standard.set(currentSpace, forKey: "currentSpace") }
    }
    private var spaceIndex: Int { spaces.firstIndex { $0.id == currentSpace } ?? 0 }
    var space: Space { spaces[spaceIndex] }
    var repoPaths: [String] {   // the current space's repos, in sidebar order
        get { space.repos }
        set { spaces[spaceIndex].repos = newValue }
    }
    var worktrees: [String: [Worktree]] = [:]          // repo path → worktrees (main first), across all spaces
    var selection: String? {   // selected worktree path
        didSet {
            guard let s = selection else { spaces[spaceIndex].selection = nil; return }
            // Jumping to a worktree in another space (⌘K, PR checkout) takes the sidebar there too.
            let repo = repo(containing: s) ?? s
            if !space.repos.contains(repo), let i = spaces.firstIndex(where: { $0.repos.contains(repo) }) { currentSpace = spaces[i].id }
            spaces[spaceIndex].selection = s
            recent = [s] + recent.filter { $0 != s }.prefix(29)
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
    var tokens: [String: String] = [:]                          // account id → token, read from the Keychain once at launch
    var tokensLoaded = false
    private var models: [String: WorktreeModel] = [:]

    init() {
        selection = space.selection
        Task { await refreshAllWorktrees() }
        Task { await loadTokens() }
    }

    func loadTokens() async {
        let accts = accounts
        let found = await Task.detached(priority: .userInitiated) {
            Dictionary(accts.compactMap { a in a.readToken().map { (a.id, $0) } }, uniquingKeysWith: { a, _ in a })
        }.value
        tokens.merge(found) { _, new in new }
        tokensLoaded = true
    }

    func setToken(_ token: String?, for account: ForgejoAccount) {
        account.saveToken(token)
        tokens[account.id] = token
    }

    func accounts(in space: String) -> [ForgejoAccount] { accounts.filter { $0.spaceID == space } }

    func switchSpace(to id: String) {
        guard id != currentSpace, let s = spaces.first(where: { $0.id == id }) else { return }
        currentSpace = id
        selection = s.selection
    }

    func switchSpace(by offset: Int) {
        let i = spaceIndex + offset
        if spaces.indices.contains(i) { switchSpace(to: spaces[i].id) }
    }

    func addSpace() {
        let s = Space(id: UUID().uuidString, name: "Space \(spaces.count + 1)", repos: [])
        spaces.append(s)
        switchSpace(to: s.id)
    }

    /// Repos go to the neighbouring space; the space's accounts and their tokens are deleted.
    func deleteSpace(_ id: String) {
        guard spaces.count > 1, let i = spaces.firstIndex(where: { $0.id == id }) else { return }
        let target = spaces[i == 0 ? 1 : i - 1].id
        moveRepos(spaces[i].repos, to: target)
        for a in accounts where a.spaceID == id { setToken(nil, for: a) }
        accounts.removeAll { $0.spaceID == id }
        if currentSpace == id { switchSpace(to: target) }
        spaces.removeAll { $0.id == id }
    }

    func moveRepos(_ repos: [String], to id: String) {
        for i in spaces.indices { spaces[i].repos.removeAll(where: repos.contains) }
        if let i = spaces.firstIndex(where: { $0.id == id }) { spaces[i].repos += repos }
        if let s = selection, repos.contains(repo(containing: s) ?? s), id != currentSpace { selection = space.repos.first }
    }

    /// Sidebar drag and drop: puts `repo` just before `target` in the current space.
    func moveRepo(_ repo: String, before target: String) {
        guard repo != target, repoPaths.contains(repo) else { return }
        var r = repoPaths
        r.removeAll { $0 == repo }
        r.insert(repo, at: r.firstIndex(of: target) ?? r.endIndex)
        repoPaths = r
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
            // A repo lives in one space: opening it again here moves it.
            if !repoPaths.contains(main) { moveRepos([main], to: currentSpace) }
            await refreshWorktrees(repo: main)
            selection = url.path
        } catch { self.error = "\(url.path) is not a git repository." }
    }

    func removeRepo(_ path: String) {
        for i in spaces.indices { spaces[i].repos.removeAll { $0 == path } }
        for wt in worktrees[path] ?? [] { models[wt.path]?.stopWatching(); models[wt.path] = nil }
        worktrees[path] = nil
    }

    func refreshAllWorktrees() async {
        await withTaskGroup(of: Void.self) { g in for r in spaces.flatMap(\.repos) { g.addTask { await self.refreshWorktrees(repo: r) } } }
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

    /// The account for `remote` in the space that holds `worktree`'s repo.
    func account(for remote: RemoteRepo, worktree: String) -> ForgejoAccount? {
        let repo = repo(containing: worktree) ?? worktree
        let space = spaces.first { $0.repos.contains(repo) }?.id ?? currentSpace
        let host = remote.host.split(separator: ":").first.map(String.init)
        return accounts.first { $0.spaceID == space && $0.hostKey.split(separator: ":").first.map(String.init) == host }
    }
}

enum GitOperation: Equatable {
    case merge, cherryPick, revert, rebase
    var command: String { switch self { case .merge: "merge"; case .cherryPick: "cherry-pick"; case .revert: "revert"; case .rebase: "rebase" } }
    var verb: String { switch self { case .merge: "Merging"; case .cherryPick: "Cherry-picking"; case .revert: "Reverting"; case .rebase: "Rebasing" } }
}

enum ConflictSide { case current, incoming }

struct Space: Codable, Hashable, Identifiable {
    static let defaultID = "default"
    var id: String
    var name: String
    var repos: [String]
    var selection: String? = nil
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
    var stashCount = 0
    var operation: GitOperation?       // a merge, cherry-pick, revert or rebase waiting to be continued or aborted
    var operationSubject = ""          // the commit it's applying
    @ObservationIgnored private var gitDir: String?
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
    var unstaged: [FileChange] { files.filter { !$0.staged && $0.kind != .conflicted } }
    var conflicted: [FileChange] { files.filter { $0.kind == .conflicted } }
    var listed: [FileChange] { conflicted + staged + unstaged }   // file list order
    var changeCount: Int { Set(files.map(\.path)).count }
    var selectedFile: FileChange? { files.first { $0.id == selectedFileID } }

    // MARK: Status / diff

    func refreshStatus() async {
        do {
            let s = try await Git.status(in: path)
            branch = s.branch; headOID = s.oid; ahead = s.ahead; behind = s.behind
            let stashes = (try? await Git.string(["stash", "list", "--format=%H"], in: path)) ?? ""
            stashCount = stashes.split(separator: "\n").count
            await detectOperation()
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

    private func detectOperation() async {
        if gitDir == nil {
            gitDir = (try? await Git.string(["rev-parse", "--path-format=absolute", "--git-dir"], in: path))?.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        guard let dir = gitDir else { return }
        let fm = FileManager.default
        func has(_ name: String) -> Bool { fm.fileExists(atPath: (dir as NSString).appendingPathComponent(name)) }
        let op: GitOperation? = has("rebase-merge") || has("rebase-apply") ? .rebase
            : has("CHERRY_PICK_HEAD") ? .cherryPick : has("REVERT_HEAD") ? .revert : has("MERGE_HEAD") ? .merge : nil
        if op != operation { operation = op }
        let head = op.flatMap { o -> String? in
            switch o {
            case .cherryPick: "CHERRY_PICK_HEAD"
            case .revert: "REVERT_HEAD"
            case .merge: "MERGE_HEAD"
            case .rebase: has("rebase-merge/stopped-sha") ? "rebase-merge/stopped-sha" : nil
            }
        }
        if let head, let sha = try? String(contentsOfFile: (dir as NSString).appendingPathComponent(head), encoding: .utf8) {
            let s = (try? await Git.string(["log", "-1", "--format=%h %s", sha.trimmingCharacters(in: .whitespacesAndNewlines)], in: path)) ?? ""
            operationSubject = s.trimmingCharacters(in: .whitespacesAndNewlines)
        } else { operationSubject = "" }
    }

    /// `git <op> --continue` with the prepared message, or `--abort`.
    func finishOperation(abort: Bool) async {
        guard let op = operation else { return }
        await perform(abort ? "Aborting" : "Continuing") {
            try await Git.run(["-c", "core.editor=true", op.command, abort ? "--abort" : "--continue"], in: path, env: ["GIT_EDITOR": "true"])
        }
        await history.reload()
    }

    /// Resolves a conflicted file wholesale: one side, or "it's fixed as it is on disk".
    func resolve(_ path: String, take side: ConflictSide?) async {
        await perform("Resolving") {
            if let side {
                // The side may have deleted the file: then resolving means deleting it.
                do { try await Git.run(["checkout", side == .current ? "--ours" : "--theirs", "--", path], in: self.path) }
                catch { try await Git.run(["rm", "-q", "--", path], in: self.path); return }
            }
            try await Git.run(["add", "--", path], in: self.path)
        }
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

    /// `git stash push`: everything, or just `paths`. Untracked files only go in with `untracked`.
    func stash(message: String, untracked: Bool, paths: [String] = []) async {
        let msg = message.trimmingCharacters(in: .whitespacesAndNewlines)
        await perform("Stashing") {
            try await Git.run(["stash", "push"] + (untracked ? ["--include-untracked"] : []) + (msg.isEmpty ? [] : ["-m", msg])
                              + (paths.isEmpty ? [] : ["--"] + paths), in: path)
        }
        await history.reload()
    }

    var generatingMessage = false

    func generateMessage() async {
        generatingMessage = true; defer { generatingMessage = false }
        do { commitMessage = try await CommitAI.message(in: path, amend: amend, hasStaged: !staged.isEmpty) }
        catch { self.error = error.localizedDescription }
    }

    func loadLastCommitMessage() async {
        if let m = try? await Git.string(["log", "-1", "--format=%B"], in: path) { commitMessage = m.trimmingCharacters(in: .whitespacesAndNewlines) }
    }

    func remoteOp(_ label: String, _ args: [String]) async {
        await perform(label) { try await Git.run(args, in: path, env: gitAuth) }
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
        let pick = candidates.first { app.account(for: $0.1, worktree: path) != nil } ?? candidates.first { $0.0 == "origin" } ?? candidates.first
        remote = pick?.1
        remoteName = pick?.0 ?? "origin"
    }

    var account: ForgejoAccount? { remote.flatMap { app.account(for: $0, worktree: path) } }

    var client: ForgejoClient? {
        guard let a = account, let t = app.tokens[a.id] else { return nil }
        return ForgejoClient(a, token: t)
    }

    /// Makes git authenticate to this space's server with the space's token instead of whatever credential helper
    /// the user has (osxkeychain holds one login per host, which is what spaces are meant to get around).
    /// Only HTTPS remotes ask for credentials; SSH remotes keep using the user's keys.
    var gitAuth: [String: String] {
        guard let a = account, let t = app.tokens[a.id] else { return [:] }
        let key = "credential.\(a.baseURL).helper"
        return ["GIT_CONFIG_COUNT": "2",
                "GIT_CONFIG_KEY_0": key, "GIT_CONFIG_VALUE_0": "",   // empty resets the helper list for this URL
                "GIT_CONFIG_KEY_1": key, "GIT_CONFIG_VALUE_1": "!f() { test \"$1\" = get && echo username=\"$LOUPE_GIT_USER\" && echo password=\"$LOUPE_GIT_TOKEN\"; }; f",
                "LOUPE_GIT_USER": a.username, "LOUPE_GIT_TOKEN": t]
    }

    /// Fetches refs/pull/N/head and checks it out in a sibling worktree (or selects the existing one).
    func checkoutInWorktree(_ pr: PullRequest) async {
        let branch = "pr/\(pr.number)"
        if let existing = app.worktrees.values.joined().first(where: { $0.branch == branch }) { app.selection = existing.path; return }
        let repo = app.repo(containing: path) ?? path
        let base = (repo as NSString).deletingLastPathComponent
        let dest = (base as NSString).appendingPathComponent("\((repo as NSString).lastPathComponent)-pr-\(pr.number)")
        await perform("Checking out PR #\(pr.number)") {
            try await Git.run(["fetch", remoteName, "+refs/pull/\(pr.number)/head:refs/remotes/\(remoteName)/pr/\(pr.number)"], in: repo, env: gitAuth)
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

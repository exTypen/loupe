import Foundation
import Observation

@MainActor @Observable
final class PullListModel {
    var state = "open"
    var mine = UserDefaults.standard.bool(forKey: "pullsMine") { didSet { UserDefaults.standard.set(mine, forKey: "pullsMine") } }
    var pulls: [PullRequest] = []
    var loading = false
    var error: String?
    var lastUpdated: Date?
    var loadingMore = false
    private var page = 1
    private var exhausted = false
    var selected: Int?                         // PR number
    var details: [Int: PullRequestModel] = [:]
    @ObservationIgnored private var inFlight: String?   // state of the load running now
    @ObservationIgnored private var again = false
    @ObservationIgnored private var saving: Task<Void, Never>?

    /// Reloads from page 1. The open list is small, so it is fetched completely; closed/all keep just the first page.
    /// A call while one for the same state is in flight runs once more after it instead of alongside it.
    func load(_ client: ForgejoClient, _ repo: RemoteRepo) async {
        // The detail replaces the list, and deselecting reloads it: polling it meanwhile is wasted.
        guard selected == nil else { return }
        guard inFlight != state else { again = true; return }
        loading = true; defer { loading = false }
        repeat {
            again = false
            let st = state; inFlight = st
            await loadOnce(client, repo)
            if inFlight == st { inFlight = nil }
        } while again && selected == nil
    }

    private func loadOnce(_ client: ForgejoClient, _ repo: RemoteRepo) async {
        let st = state
        do {
            var all = try await client.pulls(repo, state: st, page: 1)
            var p = 1
            if st == "open" {
                while all.count == p * ForgejoClient.pullPage { p += 1; all += try await client.pulls(repo, state: st, page: p) }
            }
            guard st == state else { return }   // state switched mid-flight
            pulls = all; page = p; exhausted = all.count < p * ForgejoClient.pullPage
            error = nil; lastUpdated = Date()
            if st == "open" { saveCache(repo) }
        } catch { self.error = error.localizedDescription }
    }

    func loadMore(_ client: ForgejoClient, _ repo: RemoteRepo) async {
        guard !loading, !loadingMore, !exhausted else { return }
        loadingMore = true; defer { loadingMore = false }
        do {
            let next = try await client.pulls(repo, state: state, page: page + 1)
            let seen = Set(pulls.map(\.id))
            pulls += next.filter { !seen.contains($0.id) }
            page += 1; exhausted = next.count < ForgejoClient.pullPage
        } catch { self.error = error.localizedDescription }
    }

    // MARK: Disk cache (open list only) — the window shows the last known list instantly at launch.

    nonisolated private static func cacheURL(_ r: RemoteRepo) -> URL {
        let dir = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("dev.extypen.loupe", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("pulls-\(r.host)-\(r.owner)-\(r.name).json".replacingOccurrences(of: ":", with: "_"))
    }

    func restoreCache(_ r: RemoteRepo) {
        guard state == "open", pulls.isEmpty, let d = try? Data(contentsOf: Self.cacheURL(r)),
              let cached = try? JSONDecoder().decode([PullRequest].self, from: d) else { return }
        pulls = cached
    }

    /// Off the main thread, one write after another so an older list never lands last.
    private func saveCache(_ r: RemoteRepo) {
        let (pulls, prev) = (pulls, saving)
        saving = Task.detached(priority: .utility) {
            await prev?.value
            if let d = try? JSONEncoder().encode(pulls) { try? d.write(to: Self.cacheURL(r), options: .atomic) }
        }
    }

    func detail(for pr: PullRequest, client: ForgejoClient, repo: RemoteRepo, local: LocalCheckout) -> PullRequestModel {
        if let d = details[pr.number] { return d }
        let d = PullRequestModel(pr: pr, client: client, repo: repo, local: local)
        details[pr.number] = d
        return d
    }
}

struct LocalCheckout { var dir: String; var remote: String; var gitAuth: [String: String] = [:] }

@MainActor @Observable
final class PullRequestModel {
    var pr: PullRequest
    let client: ForgejoClient
    let repo: RemoteRepo
    let local: LocalCheckout            // where to fetch and diff when the server won't send the diff
    var files: [FileDiff] = [] { didSet { fileDerived = nil; viewedDerived = nil; viewedUnder = [:]; commentableCache = nil } }   // tree order (dirs first), so next/prev follows the sidebar
    var collapsed: Set<String> = []
    private var index: [String: Int] = [:]
    @ObservationIgnored private var lastDiff: Data?
    @ObservationIgnored private var localDiffSHA: String?   // head the local fallback diff was made at
    private var fingerprints: [String: UInt64] = [:] { didSet { viewedDerived = nil; viewedUnder = [:] } }
    @ObservationIgnored private var reloadQueued = false
    // Derived per-row lookups, rebuilt on first read after their inputs change. Readers go through the observed
    // inputs first, so views still track those.
    @ObservationIgnored private var fileDerived: (filtered: [FileDiff], under: [String: [Int]])?
    @ObservationIgnored private var viewedDerived: Int?
    @ObservationIgnored private var commentDerived: (counts: [String: Int], inline: [String: [InlineComment]])?
    @ObservationIgnored private var commentableCache: (path: String, test: (DiffLine) -> Bool)?
    @ObservationIgnored private var viewedUnder: [String: (done: Int, total: Int)] = [:]
    @ObservationIgnored private var treeFilterCache: (tree: [String], filter: String, paths: [String])?
    var selectedPath: String? {
        didSet {
            // Full mode: whatever opens, from the tree, a jump or a result, opens as a tab next to the current one.
            guard fullMode, let p = selectedPath, !tabs.contains(p) else { return }
            let at = oldValue.flatMap { tabs.firstIndex(of: $0) }.map { $0 + 1 } ?? tabs.count
            tabs.insert(p, at: min(at, tabs.count))
        }
    }
    var tabs: [String] = []                  // Full mode's open files, in tab order
    var reviews: [Review] = []
    var comments: [ReviewComment] = [] { didSet { commentDerived = nil } }
    var conversation: [IssueComment] = []
    var commits: [PRCommit] = []
    var loading = false
    var error: String?
    var filter = "" { didSet { fileDerived = nil; viewedUnder = [:] } }
    @ObservationIgnored lazy var checks = ChecksModel(client: client, repo: repo, sha: pr.head.sha)
    var drafts: [DraftComment] = [] { didSet { persistDrafts(); commentDerived = nil } }
    var viewed: [String: UInt64] = [:] { didSet { viewedDerived = nil; viewedUnder = [:]; UserDefaults.standard.set(viewed.mapValues { String($0) }, forKey: viewedKey) } }

    private var viewedKey: String { "viewed:\(repo.host)/\(repo.owner)/\(repo.name)#\(pr.number)" }
    private var draftsKey: String { "drafts:\(repo.host)/\(repo.owner)/\(repo.name)#\(pr.number)" }

    init(pr: PullRequest, client: ForgejoClient, repo: RemoteRepo, local: LocalCheckout) {
        self.pr = pr; self.client = client; self.repo = repo; self.local = local
        let v = UserDefaults.standard.dictionary(forKey: "viewed:\(repo.host)/\(repo.owner)/\(repo.name)#\(pr.number)") as? [String: String] ?? [:]
        viewed = v.compactMapValues(UInt64.init)
        drafts = (try? JSONDecoder().decode([DraftComment].self, from: UserDefaults.standard.data(forKey: "drafts:\(repo.host)/\(repo.owner)/\(repo.name)#\(pr.number)") ?? Data())) ?? []
    }

    private func persistDrafts() { UserDefaults.standard.set(try? JSONEncoder().encode(drafts), forKey: draftsKey) }

    var selectedFile: FileDiff? { selectedPath.flatMap(file) }
    func file(_ path: String) -> FileDiff? { index[path].map { files[$0] } }
    func files(under dir: String) -> [FileDiff] {
        let d = derivedFiles
        return d.under[dir]?.map { d.filtered[$0] } ?? []
    }

    /// Directory checkbox: mark everything under it viewed, or clear it if it all already is.
    func toggleViewed(dir: String) {
        let fs = files(under: dir)
        let allViewed = fs.allSatisfy(isViewed)
        var v = viewed   // one assignment: each write persists the whole dictionary
        for f in fs { v[f.path] = allViewed ? nil : fingerprints[f.path] }
        viewed = v
    }
    var filteredFiles: [FileDiff] { derivedFiles.filtered }

    /// A folder's viewed progress for the tree, counted once per folder until files, the filter or viewed change.
    func viewedProgress(under dir: String) -> (done: Int, total: Int) {
        let fs = files(under: dir)
        _ = (viewed, fingerprints)   // observed reads
        if let p = viewedUnder[dir] { return p }
        let p = (fs.filter(isViewed).count, fs.count)
        viewedUnder[dir] = p
        return p
    }

    /// Full mode's tree paths with the filter applied, filtered again only when the tree or the filter changes.
    var filteredTree: [String] {
        let (t, q) = (tree, filter)   // observed reads
        if q.isEmpty { return t }
        if let c = treeFilterCache, c.filter == q, c.tree == t { return c.paths }
        let paths = t.filter { $0.localizedCaseInsensitiveContains(q) }
        treeFilterCache = (t, q, paths)
        return paths
    }
    var viewedCount: Int {
        let (fs, _, _) = (files, viewed, fingerprints)   // observed reads
        if let n = viewedDerived { return n }
        let n = fs.filter(isViewed).count
        viewedDerived = n
        return n
    }

    /// The filtered list, and for every directory the indexes into it of the files below it.
    private var derivedFiles: (filtered: [FileDiff], under: [String: [Int]]) {
        let (fs, q) = (files, filter)   // observed reads
        if let d = fileDerived { return d }
        let filtered = q.isEmpty ? fs : fs.filter { $0.path.localizedCaseInsensitiveContains(q) }
        var under: [String: [Int]] = [:]
        for (i, f) in filtered.enumerated() {
            let p = f.path
            for j in p.indices where p[j] == "/" { under[String(p[..<j]), default: []].append(i) }
        }
        fileDerived = (filtered, under)
        return (filtered, under)
    }

    /// Runs one load at a time: a call while one is in flight (poll, app activation, refresh) runs once more after it.
    func load() async {
        guard !loading else { reloadQueued = true; return }
        loading = true; defer { loading = false }
        repeat { reloadQueued = false; await loadOnce() } while reloadQueued
    }

    private func loadOnce() async {
        do {
            async let prReq = client.pull(repo, pr.number)
            async let reviewsReq = client.reviews(repo, pr.number)
            async let convReq = client.issueComments(repo, pr.number)
            async let commitsReq = client.pullCommits(repo, pr.number)
            pr = try await prReq
            if checks.sha != pr.head.sha { checks.sha = pr.head.sha; Task { await checks.refresh() } }   // new push
            let data = try await diffData()
            // Polling: an unchanged diff must not rebuild the table (scroll, selection and highlighting would reset).
            if data != lastDiff { lastDiff = data
            (files, fingerprints) = await Task.detached(priority: .userInitiated) {
                let fs = DiffParser.parse(data).sorted { FileTree.treeOrder($0.path, $1.path) }
                return (fs, Dictionary(fs.map { ($0.path, Self.fingerprint($0)) }, uniquingKeysWith: { a, _ in a }))
            }.value
            index = Dictionary(files.enumerated().map { ($1.path, $0) }, uniquingKeysWith: { a, _ in a })
            }
            if selectedPath == nil || !files.contains(where: { $0.path == selectedPath }) {
                selectedPath = files.first { !isViewed($0) }?.path ?? files.first?.path
            }
            applyDefaultCollapse()
            reviews = try await reviewsReq
            conversation = try await convReq
            if let c = try? await commitsReq { commits = c }   // the timeline does without them rather than fail the load
            comments = try await client.reviewComments(repo, pr.number, reviews: reviews)
            error = nil
        } catch { self.error = error.localizedDescription }
    }

    /// The server's diff, or for a PR too big for GitHub to serve, the same three-dot diff computed by local git.
    private func diffData() async throws -> Data {
        if localDiffSHA == pr.head.sha, let lastDiff { return lastDiff }   // polling: no new push, skip the fetch
        do { return try await client.diff(repo, pr.number) } catch is ForgejoClient.DiffTooLarge {}
        try await fetchHead()
        // Diff from the base sha, not the branch tip: once the PR is merged the tip contains it and `tip...head` is
        // empty. Byte-identical to GitHub's own diff.
        let data = try await Git.run(["diff", "--no-color", "--no-ext-diff", "-M", "\(pr.base.sha)...\(pr.head.sha)"], in: local.dir)
        localDiffSHA = pr.head.sha
        return data
    }

    /// Brings the PR's head and base commits into the local repository (`refs/pull/N/head` works for forks too).
    /// Fetching the base branch brings in pr.base.sha. A no-op when both are already here.
    func fetchHead() async throws {
        let dir = local.dir
        let hasHead = (try? await Git.run(["cat-file", "-e", "\(pr.head.sha)^{commit}"], in: dir)) != nil
        let hasBase = (try? await Git.run(["cat-file", "-e", "\(pr.base.sha)^{commit}"], in: dir)) != nil
        if hasHead && hasBase { return }
        let head = "refs/remotes/\(local.remote)/pr/\(pr.number)", base = "refs/remotes/\(local.remote)/\(pr.base.ref)"
        try await Git.run(["fetch", "--no-tags", local.remote, "+refs/pull/\(pr.number)/head:\(head)", "+refs/heads/\(pr.base.ref):\(base)"], in: dir, env: local.gitAuth)
    }

    // MARK: Full mode — the whole repository at the PR head, whole files, code navigation

    struct Location: Hashable { var path: String; var line: Int }
    enum NavResults {
        case definitions(String, [CodeSymbol])
        case references(String, [CodeMatch], noDefinition: Bool)
    }

    var fullMode = UserDefaults.standard.bool(forKey: "prFullMode") {
        didSet {
            if UserDefaults.standard.bool(forKey: "prFullMode") != fullMode { UserDefaults.standard.set(fullMode, forKey: "prFullMode") }
            if fullMode, let p = selectedPath, !tabs.contains(p) { tabs.append(p) }
            // Back to Compact on a file this PR doesn't change: show the first change instead.
            if !fullMode, selectedPath.flatMap(file) == nil { selectedPath = files.first?.path }
            if !fullMode { navResults = nil }
        }
    }
    var tree: [String] = []                 // every path at the head (and deleted ones), tree order
    var treeError: String?
    var treeLoading = false
    var fullCollapsed: Set<String> = []
    @ObservationIgnored private var treeSHA: String?
    @ObservationIgnored private var treeDirs = Set<String>()
    @ObservationIgnored private var collapseApplied = false

    /// Once both the tree and the PR's files are known: fold every directory this PR doesn't touch.
    /// (Either can arrive first.)
    func applyDefaultCollapse() {
        guard !collapseApplied, !treeDirs.isEmpty, !files.isEmpty else { return }
        collapseApplied = true
        var open = Set<String>()
        for f in files { var d = (f.path as NSString).deletingLastPathComponent; while !d.isEmpty { open.insert(d); d = (d as NSString).deletingLastPathComponent } }
        fullCollapsed = treeDirs.subtracting(open)
    }
    var fullFiles: [String: FileDiff] = [:]  // whole files at the head, with this PR's changes inline
    @ObservationIgnored private var fullSHA: String?
    var reveal: DiffReveal?
    var back: [Location] = []
    var forward: [Location] = []
    var navResults: NavResults?
    var navBusy: String?                     // the word being looked up

    /// `git ls-tree` at the head. Directories start collapsed except where this PR changes something.
    func loadTree() async {
        guard treeSHA != pr.head.sha, !treeLoading else { return }
        treeLoading = true; defer { treeLoading = false }
        do {
            try await fetchHead()
            let out = try await Git.run(["ls-tree", "-r", "-z", "--name-only", pr.head.sha], in: local.dir)
            let changed = files.map(\.path)
            let sha = pr.head.sha
            let (paths, dirs) = await Task.detached(priority: .userInitiated) {
                var set = Set(out.split(separator: 0).map { String(decoding: $0, as: UTF8.self) })
                set.formUnion(changed)   // deleted files stay reachable
                let paths = set.sorted(by: FileTree.treeOrder)
                var dirs = Set<String>()
                for p in paths { var d = (p as NSString).deletingLastPathComponent; while !d.isEmpty, dirs.insert(d).inserted { d = (d as NSString).deletingLastPathComponent } }
                return (paths, dirs)
            }.value
            treeDirs = dirs
            tree = paths; treeSHA = sha; treeError = nil
            applyDefaultCollapse()
        } catch { treeError = error.localizedDescription }
    }

    /// The whole file at the head: changed files as a full-context diff, the rest as plain lines.
    func loadFull(_ path: String) async {
        if fullSHA != pr.head.sha { fullFiles = [:]; fullSHA = pr.head.sha }
        guard fullFiles[path] == nil else { return }
        do {
            try await fetchHead()
            let dir = local.dir, base = pr.base.sha, head = pr.head.sha
            let changed = file(path)
            let f: FileDiff
            if let c = changed {
                if c.isBinary || c.newPath == nil { f = c }   // binary, or deleted: the diff already shows everything
                else {
                    let paths = [c.oldPath, c.newPath].compactMap { $0 }
                    let data = try await Git.run(["diff", "--no-color", "--no-ext-diff", "-M", "-U1000000", "\(base)...\(head)", "--"] + Array(Set(paths)), in: dir)
                    f = await Task.detached { Self.wholeFile(DiffParser.parse(data).first) }.value ?? c
                }
            } else {
                let data = try await Git.run(["show", "\(head):\(path)"], in: dir)
                f = await Task.detached { Self.plainFile(path, data) }.value
            }
            fullFiles[path] = f
        } catch { self.error = error.localizedDescription }
    }

    /// A full-context diff without its single hunk header row: the file reads top to bottom.
    nonisolated static func wholeFile(_ d: FileDiff?) -> FileDiff? {
        guard var d else { return nil }
        d.lines.removeAll { $0.kind == .hunk }
        for i in d.lines.indices { d.lines[i].hunkIndex = 0 }
        d.hunks = [Hunk(header: d.hunks.first?.header ?? "", lines: 0..<d.lines.count)]
        return d
    }

    /// An unchanged file, as context lines.
    nonisolated static func plainFile(_ path: String, _ data: Data) -> FileDiff {
        var f = FileDiff()
        f.oldPath = path; f.newPath = path
        if data.prefix(8000).contains(0) { f.isBinary = true; return f }
        let text = String(decoding: data, as: UTF8.self)
        var n = 0
        text.enumerateSubstringLines { line in
            n += 1
            f.lines.append(DiffLine(kind: .context, text: line, oldLine: n, newLine: n, hunkIndex: 0))
        }
        f.hunks = [Hunk(header: "", lines: 0..<f.lines.count)]
        return f
    }

    /// The file's text at the head (its new side), for the outline.
    func headText(_ path: String) -> String? {
        guard let f = fullFiles[path], !f.isBinary else { return nil }
        return f.lines.filter { $0.kind != .removed && $0.kind != .meta }.map { String($0.text) }.joined(separator: "\n")
    }

    /// Review comments only attach to lines inside the diff's hunks: in a whole file, the rest can't take one.
    func commentable(_ path: String) -> (DiffLine) -> Bool {
        guard let c = file(path) else { return { _ in false } }
        if let cached = commentableCache, cached.path == path { return cached.test }
        let new = Set(c.lines.compactMap { $0.kind == .removed ? nil : $0.newLine })
        let old = Set(c.lines.compactMap { $0.kind == .removed ? $0.oldLine : nil })
        let test: (DiffLine) -> Bool = { l in l.kind == .removed ? old.contains(l.oldLine ?? -1) : new.contains(l.newLine ?? -1) }
        commentableCache = (path, test)
        return test
    }

    func open(_ path: String, line: Int, record: Bool = true) {
        if record, let cur = selectedPath, cur != path || reveal?.line != line {
            back.append(Location(path: cur, line: reveal?.line ?? 1)); forward = []
        }
        selectedPath = path
        reveal = DiffReveal(line: line)
    }

    /// From the Overview: show a file (and a line, when known). A file this PR doesn't change needs Full mode.
    func reveal(_ path: String, line: Int?) {
        if file(path) == nil && !fullMode { fullMode = true }
        if let line { open(path, line: line) } else { selectedPath = path }
    }

    func closeTab(_ path: String) {
        guard let i = tabs.firstIndex(of: path) else { return }
        tabs.remove(at: i)
        if selectedPath == path {
            let next = tabs.isEmpty ? nil : tabs[min(i, tabs.count - 1)]
            selectedPath = next
        }
    }

    func closeOtherTabs(_ path: String) { tabs = tabs.filter { $0 == path }; if selectedPath != path { selectedPath = path } }

    func goBack() {
        guard let to = back.popLast() else { return }
        if let cur = selectedPath { forward.append(Location(path: cur, line: reveal?.line ?? 1)) }
        open(to.path, line: to.line, record: false)
    }

    func goForward() {
        guard let to = forward.popLast() else { return }
        if let cur = selectedPath { back.append(Location(path: cur, line: reveal?.line ?? 1)) }
        open(to.path, line: to.line, record: false)
    }

    /// ⌘-click / Go to Definition / Find References on `word` in `path`.
    func navigate(_ word: String, _ action: CodeAction, from path: String) async {
        navBusy = word; defer { navBusy = nil }
        do { try await fetchHead() } catch { self.error = error.localizedDescription; return }
        let (dir, head) = (local.dir, pr.head.sha)
        if action == .definition {
            let defs = await CodeNavigator.definitions(of: word, at: head, in: dir, from: path)
            if defs.count == 1, let d = defs.first { navResults = nil; open(d.path, line: d.line); return }
            if !defs.isEmpty { navResults = .definitions(word, defs); return }
        }
        navResults = .references(word, await CodeNavigator.references(of: word, at: head, in: dir), noDefinition: action == .definition)
    }

    // MARK: Viewed — keyed by a fingerprint of the file's diff, so a file changed by a new push turns unviewed.

    nonisolated static func fingerprint(_ f: FileDiff) -> UInt64 {
        var h: UInt64 = 0xcbf29ce484222325   // FNV-1a: stable across launches, unlike Hasher
        for l in f.lines { for b in l.text.utf8 { h = (h ^ UInt64(b)) &* 0x100000001b3 }; h = (h ^ UInt64(l.kind.rawValue)) &* 0x100000001b3 }
        return h
    }

    func isViewed(_ f: FileDiff) -> Bool { viewed[f.path] != nil && viewed[f.path] == fingerprints[f.path] }

    func toggleViewed(_ f: FileDiff) {
        if isViewed(f) { viewed[f.path] = nil } else { viewed[f.path] = fingerprints[f.path] }
    }

    /// Mark current viewed and jump to the next unviewed file — the core big-PR loop.
    func markViewedAndAdvance() {
        guard let f = selectedFile else { return }
        if !isViewed(f) { toggleViewed(f) }
        let list = filteredFiles
        guard let idx = list.firstIndex(where: { $0.path == f.path }) else { return }
        let after = list[(idx + 1)...] + list[..<idx]
        selectedPath = after.first { !isViewed($0) }?.path ?? selectedPath
    }

    func move(_ delta: Int) {
        let list = filteredFiles
        guard let idx = list.firstIndex(where: { $0.path == selectedPath }) else { selectedPath = list.first?.path; return }
        selectedPath = list[max(0, min(list.count - 1, idx + delta))].path
    }

    // MARK: Comments

    enum ReviewerState { case approved, changesRequested, pending }

    /// One state per reviewer, as the web sidebar shows it: a push dismisses earlier approvals and re-requested
    /// reviewers appear as REQUEST_REVIEW / requested_reviewers — those are pending, not approved.
    var reviewerStates: [(login: String, state: ReviewerState)] {
        var latest: [String: Review] = [:]
        for r in reviews where r.dismissed != true && r.stale != true && ["APPROVED", "REQUEST_CHANGES", "REQUEST_REVIEW"].contains(r.state) {
            if let u = r.user?.login, (latest[u]?.id ?? -1) < r.id { latest[u] = r }
        }
        var out: [String: ReviewerState] = latest.mapValues { $0.state == "APPROVED" ? .approved : $0.state == "REQUEST_CHANGES" ? .changesRequested : .pending }
        // Forgejo records a re-request as a newer REQUEST_REVIEW review (handled above) and also lists past reviewers in
        // requested_reviewers. GitHub keeps the old approval and lists only open requests, so there an approver in
        // requested_reviewers was re-requested.
        for u in pr.requested_reviewers ?? [] where out[u.login] == nil || (client.github && out[u.login] == .approved) { out[u.login] = .pending }
        return out.map { ($0.key, $0.value) }.sorted { $0.login < $1.login }
    }

    func inlineComments(for path: String) -> [InlineComment] { derivedComments.inline[path] ?? [] }

    func commentCount(for path: String) -> Int { derivedComments.counts[path] ?? 0 }

    private var derivedComments: (counts: [String: Int], inline: [String: [InlineComment]]) {
        let (cs, ds) = (comments, drafts)   // observed reads
        if let d = commentDerived { return d }
        var inline: [String: [InlineComment]] = [:]
        for c in cs {
            inline[c.path, default: []].append(InlineComment(author: c.user?.login ?? "?", body: c.body, newLine: c.position > 0 ? c.position : nil, oldLine: c.original_position > 0 ? c.original_position : nil))
        }
        for d in ds { inline[d.path, default: []].append(InlineComment(author: "You", body: d.body, newLine: d.newLine, oldLine: d.oldLine, draftID: d.id)) }
        let d = (inline.mapValues(\.count), inline)
        commentDerived = d
        return d
    }

    func submit(event: ForgejoClient.ReviewEvent, body: String) async -> Bool {
        do {
            try await client.submitReview(repo, pr.number, event: event, body: body, commit: pr.head.sha, comments: drafts)
            drafts = []
            await load()
            return true
        } catch { self.error = error.localizedDescription; return false }
    }

    func postComment(_ body: String) async {
        do { try await client.addIssueComment(repo, pr.number, body: body); conversation = try await client.issueComments(repo, pr.number) }
        catch { self.error = error.localizedDescription }
    }

    func merge(_ style: ForgejoClient.MergeStyle) async {
        do { try await client.merge(repo, pr.number, style: style); await load() }
        catch { self.error = error.localizedDescription }
    }
}

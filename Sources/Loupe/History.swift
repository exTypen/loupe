import SwiftUI
import Observation

struct Commit: Identifiable, Hashable {
    var sha: String
    var parents: [String]
    var author: String
    var email: String
    var date: Date
    var refs: [String]
    var subject: String
    var selector: String? = nil   // "stash@{2}" for stash entries
    var id: String { sha }
    var short: String { String(sha.prefix(8)) }
}

/// What the History list shows.
enum HistoryScope: Hashable {
    case current, all, branch(String), stashes
    var revs: [String] {
        switch self {
        case .current: ["HEAD"]
        case .all: ["--branches", "--remotes", "--tags", "HEAD"]
        case .branch(let b): [b]
        case .stashes: []
        }
    }
    var title: String {
        switch self {
        case .current: "Current Branch"
        case .all: "All Branches"
        case .branch(let b): b
        case .stashes: "Stashes"
        }
    }
}

extension Git {
    static let logFormat = "%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1e"

    static func log(in dir: String, revs: [String] = ["HEAD"], search: String, skip: Int, limit: Int) async throws -> [Commit] {
        // Topological order keeps a commit below all of its children, which the graph lanes need. With the
        // commit-graph file git keeps, it's as fast as the default order.
        var args = ["log", "--format=\(logFormat)", "--decorate=full", "--topo-order", "--skip=\(skip)", "-n", "\(limit)"]
        if !search.isEmpty { args += ["-i", "--grep=\(search)", "--author=\(search)"] }   // OR'ed by git
        // Unborn branch (fresh repo) → git log fails; that's just an empty history.
        let data = try await run(args + revs + ["--"], in: dir, allowFailure: true)
        return parseLog(data)
    }

    /// `git stash list`, newest first. The reflog selector rides in the refs field.
    static func stashes(in dir: String) async throws -> [Commit] {
        let data = try await run(["stash", "list", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%gd%x1f%s%x1e"], in: dir, allowFailure: true)
        return parseLog(data).map { c in var c = c; c.selector = c.refs.first; c.refs = []; return c }
    }

    static func parseLog(_ data: Data) -> [Commit] {
        String(decoding: data, as: UTF8.self).split(separator: "\u{1e}").compactMap { rec in
            let f = rec.trimmingCharacters(in: .newlines).split(separator: "\u{1f}", omittingEmptySubsequences: false)
            guard f.count >= 7 else { return nil }
            return Commit(sha: String(f[0]), parents: f[1].split(separator: " ").map(String.init), author: String(f[2]), email: String(f[3]),
                          date: Date(timeIntervalSince1970: TimeInterval(f[4]) ?? 0),
                          refs: f[5].isEmpty ? [] : f[5].components(separatedBy: ", "), subject: f[6...].joined(separator: "\u{1f}"))
        }
    }

    /// Diff of a commit against its first parent (merges included), root commits against the empty tree.
    static func commitDiff(_ sha: String, in dir: String) async throws -> [FileDiff] {
        DiffParser.parse(try await run(["show", "--format=", "--no-color", "--no-ext-diff", "-M", "--diff-merges=first-parent", sha], in: dir))
    }

    /// A stash's changes, untracked files included (they live in the stash's third parent).
    static func stashDiff(_ selector: String, in dir: String) async throws -> [FileDiff] {
        DiffParser.parse(try await run(["stash", "show", "-p", "--include-untracked", "--no-color", "--no-ext-diff", "-M", selector], in: dir))
    }
}

// MARK: - Graph

/// One row of the commit graph: which lanes pass by, which end at the commit (its children above) and which
/// leave it (its parents below). Lanes keep their column for as long as they live, so the lines stay vertical.
/// Lane 0 belongs to the main line (HEAD's first parents), so the branch you're on stays put on the left and the
/// others read as forks off it.
struct GraphRow: Hashable {
    var col: Int
    var through: [Int] = []
    var into: [Int] = []
    var out: [Int] = []
    var width: Int { ([col] + through + into + out).max()! + 1 }
}

enum Graph {
    static func layout(_ commits: [Commit], main tip: String? = nil) -> [GraphRow] {
        // The main line: first parents from the tip, as far as the loaded commits go.
        let byID = Dictionary(commits.map { ($0.sha, $0) }, uniquingKeysWith: { a, _ in a })
        var mainLine = Set<String>()
        var cur = tip.flatMap { byID[$0] }
        while let c = cur, mainLine.insert(c.sha).inserted { cur = c.parents.first.flatMap { byID[$0] } }
        let reserved = mainLine.isEmpty ? 0 : 1
        func free(_ lanes: [String?]) -> Int { lanes.indices.dropFirst(reserved).first { lanes[$0] == nil } ?? lanes.count }

        var lanes: [String?] = reserved == 1 ? [nil] : []   // the commit each lane is waiting for
        var rows: [GraphRow] = []
        rows.reserveCapacity(commits.count)
        for c in commits {
            // A main-line commit always sits in lane 0; other lanes waiting for it (newer commits on top of it,
            // like origin/main ahead of main) end there.
            var col = mainLine.contains(c.sha) ? 0 : lanes.firstIndex(of: c.sha) ?? free(lanes)
            if col == lanes.count { lanes.append(nil) }
            var row = GraphRow(col: col)
            for (i, l) in lanes.enumerated() where l != nil {
                if l == c.sha { row.into.append(i); lanes[i] = nil } else { row.through.append(i) }
            }
            for (k, p) in c.parents.enumerated() {
                // Join a lane already headed there, except the main line, which keeps lane 0 (the other lane merges into it at p).
                let keepsMain = k == 0 && col == 0 && reserved == 1
                if !keepsMain, let waiting = lanes.firstIndex(of: p) { row.out.append(waiting); continue }
                let slot = k == 0 && lanes[col] == nil ? col : free(lanes)
                if slot == lanes.count { lanes.append(nil) }
                lanes[slot] = p
                row.out.append(slot)
            }
            col = row.col
            while lanes.count > reserved, lanes.last == .some(nil) { lanes.removeLast() }
            rows.append(row)
        }
        return rows
    }
}

@MainActor @Observable
final class HistoryModel {
    let path: String
    var commits: [Commit] = []
    var graph: [GraphRow] = []          // parallel to commits; empty for stashes
    var graphWidth = 0                   // lanes, capped
    var scope = HistoryScope.current
    var branches: (local: [String], remote: [String]) = ([], [])
    var notInHead: Set<String> = []      // shown commits HEAD doesn't contain: these can be cherry-picked
    var stoppedOnConflicts = false       // a cherry-pick, revert or stash apply left conflicts to resolve
    var search = ""
    var loading = false
    var exhausted = false
    var selectedSHA: String?
    var message = ""
    var files: [FileDiff] = []
    var selectedPath: String?
    var collapsed: Set<String> = []
    var detailLoading = false
    var error: String?
    private static let page = 300
    private var detailTask: Task<Void, Never>?

    init(path: String) { self.path = path }

    var selected: Commit? { commits.first { $0.sha == selectedSHA } }
    var selectedFile: FileDiff? { files.first { $0.path == selectedPath } }

    func reload() async {
        exhausted = false
        await fetch(skip: 0, replace: true)
        if selectedSHA == nil || !commits.contains(where: { $0.sha == selectedSHA }) { select(commits.first?.sha) }
        async let refs = Git.string(["for-each-ref", "--format=%(refname)", "--sort=-committerdate", "refs/heads", "refs/remotes"], in: path)
        let names = ((try? await refs) ?? "").split(separator: "\n").map(String.init).filter { !$0.hasSuffix("/HEAD") }
        branches = (names.filter { $0.hasPrefix("refs/heads/") }.map { String($0.dropFirst(11)) },
                    names.filter { $0.hasPrefix("refs/remotes/") }.map { String($0.dropFirst(13)) })
        if case .branch(let b) = scope, !branches.local.contains(b), !branches.remote.contains(b) { scope = .current; await reload() }
    }

    func setScope(_ s: HistoryScope) {
        guard s != scope else { return }
        scope = s
        commits = []; graph = []; selectedSHA = nil
        Task { await reload() }
    }

    func loadMore() async {
        guard !loading, !exhausted else { return }
        await fetch(skip: commits.count, replace: false)
    }

    private func fetch(skip: Int, replace: Bool) async {
        loading = true; defer { loading = false }
        do {
            if scope == .stashes {
                let all = try await Git.stashes(in: path)
                exhausted = true
                commits = search.isEmpty ? all : all.filter { $0.subject.localizedCaseInsensitiveContains(search) }
                graph = []; graphWidth = 0; notInHead = []
                return
            }
            let batch = try await Git.log(in: path, revs: scope.revs, search: search, skip: skip, limit: Self.page)
            exhausted = batch.count < Self.page
            commits = replace ? batch : commits + batch
            // The main line is HEAD's, or the shown branch's own when HEAD isn't in it.
            let head = scope == .current ? commits.first?.sha : (try? await Git.string(["rev-parse", "HEAD"], in: path))?.trimmingCharacters(in: .whitespacesAndNewlines)
            let tip = head.flatMap { h in commits.contains { $0.sha == h } ? h : nil } ?? commits.first?.sha
            graph = search.isEmpty ? Graph.layout(commits, main: tip) : []   // a filtered list has no lines to draw
            graphWidth = min(graph.map(\.width).max() ?? 0, 12)
            if graphWidth <= 1 { graph = []; graphWidth = 0 }   // one straight line says nothing
            if scope == .current { notInHead = [] } else {
                let out = try await Git.string(["rev-list", "-n", "20000"] + scope.revs.filter { $0 != "HEAD" } + ["^HEAD", "--"], in: path)
                notInHead = Set(out.split(separator: "\n").map(String.init))
            }
        } catch { self.error = error.localizedDescription }
    }

    func select(_ sha: String?) {
        selectedSHA = sha
        detailTask?.cancel()
        guard let sha else { files = []; message = ""; return }
        detailTask = Task {
            detailLoading = true; defer { detailLoading = false }
            let stash = commits.first { $0.sha == sha }?.selector
            async let body = Git.string(["show", "-s", "--format=%B", sha], in: path)
            let path = path
            async let diff = { if let stash { try await Git.stashDiff(stash, in: path) } else { try await Git.commitDiff(sha, in: path) } }()
            do {
                let (b, d) = try await (body, diff)
                guard !Task.isCancelled else { return }
                message = b.trimmingCharacters(in: .whitespacesAndNewlines)
                files = d.sorted { FileTree.treeOrder($0.path, $1.path) }
                if !files.contains(where: { $0.path == selectedPath }) { selectedPath = files.first?.path }
            } catch { if !Task.isCancelled { self.error = error.localizedDescription } }
        }
    }

    // MARK: Actions on the worktree

    /// Runs a git command that may stop on conflicts; conflicts aren't an error, they're work for the Changes tab.
    private func run(_ args: [String]) async {
        do { try await Git.run(args, in: path) }
        catch {
            let unmerged = (try? await Git.string(["diff", "--name-only", "--diff-filter=U"], in: path)) ?? ""
            if unmerged.isEmpty { self.error = error.localizedDescription } else { stoppedOnConflicts = true }
        }
        await reload()
    }

    func cherryPick(_ c: Commit) async { await run(["cherry-pick"] + (c.parents.count > 1 ? ["-m", "1"] : []) + [c.sha]) }
    func revert(_ c: Commit) async { await run(["revert", "--no-edit"] + (c.parents.count > 1 ? ["-m", "1"] : []) + [c.sha]) }
    func applyStash(_ c: Commit, pop: Bool) async { if let s = c.selector { await run(["stash", pop ? "pop" : "apply", s]) } }
    func dropStash(_ c: Commit) async { if let s = c.selector { await run(["stash", "drop", s]) } }

    func move(_ delta: Int) {
        guard let i = commits.firstIndex(where: { $0.sha == selectedSHA }) else { return select(commits.first?.sha) }
        select(commits[max(0, min(commits.count - 1, i + delta))].sha)
    }
}

// MARK: - Views

struct HistoryView: View {
    @Environment(AppModel.self) private var app
    var worktree: WorktreeModel
    @Bindable var model: HistoryModel
    var layout: DiffLayout
    @State private var worktreeBase: String?
    @State private var dropping: Commit?
    @AppStorage("tab") private var tab = "changes"

    var body: some View {
        ResizableSplit(key: "historyListWidth", minWidth: 260, maxWidth: 420, defaultFraction: 340/420) {
            VStack(spacing: 0) {
                HStack(spacing: 6) {
                    TextField(model.scope == .stashes ? "Search stashes" : "Search", text: $model.search).textFieldStyle(.roundedBorder)
                        .help("Commit message or author")
                        .onSubmit { Task { await model.reload() } }
                    scopeMenu
                }
                .padding(8)
                Divider()
                List(selection: Binding(get: { model.selectedSHA }, set: { model.select($0) })) {
                    ForEach(Array(model.commits.enumerated()), id: \.element.id) { i, c in
                        CommitRow(commit: c, isHead: c.sha == worktree.headOID, client: worktree.client, remote: worktree.remote,
                                  graph: model.graph.indices.contains(i) ? model.graph[i] : nil, graphWidth: model.graphWidth)
                            .tag(c.sha)
                            .listRowInsets(EdgeInsets(top: 0, leading: 6, bottom: 0, trailing: 8))   // graph lines run row to row
                            .contextMenu { menu(c) }
                            .onAppear { if c.sha == model.commits.last?.sha { Task { await model.loadMore() } } }
                    }
                    if model.loading && !model.commits.isEmpty { ProgressView().controlSize(.small).frame(maxWidth: .infinity) }
                }
                .listStyle(.inset)
                .frame(maxHeight: .infinity)
                .overlay {
                    if model.commits.isEmpty && !model.loading {
                        if !model.search.isEmpty { ContentUnavailableView("No Matches", systemImage: "magnifyingglass") }
                        else if model.scope == .stashes {
                            ContentUnavailableView("No Stashes", systemImage: "tray", description: Text("Stash changes from the Changes tab to set them aside."))
                        } else { ContentUnavailableView("No Commits", systemImage: "clock.arrow.circlepath") }
                    }
                }
            }
            .frame(maxHeight: .infinity)
        } trailing: {
            CommitDetailView(model: model, remote: worktree.remote, account: worktree.account, layout: layout) { c in AnyView(actions(c)) }
                .frame(minWidth: 420, maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .task(id: worktree.headOID) { await model.reload() }
        .onChange(of: model.search) { _, s in if s.isEmpty { Task { await model.reload() } } }
        .onReceive(NotificationCenter.default.publisher(for: .moveFile)) { n in if let d = n.object as? Int { model.move(d) } }
        .onReceive(NotificationCenter.default.publisher(for: .refresh)) { _ in Task { await model.reload() } }
        // A cherry-pick, revert or stash apply that stopped on conflicts: resolving them happens in Changes.
        .onChange(of: model.stoppedOnConflicts) { _, stopped in if stopped { model.stoppedOnConflicts = false; tab = "changes" } }
        .confirmationDialog("Drop \(dropping?.selector ?? "stash")?", isPresented: Binding(get: { dropping != nil }, set: { if !$0 { dropping = nil } })) {
            Button("Drop Stash", role: .destructive) { if let d = dropping { Task { await model.dropStash(d) } } }
        } message: { Text("The stashed changes are deleted.") }
        .alert("Git", isPresented: Binding(get: { model.error != nil }, set: { if !$0 { model.error = nil } })) {
            Button("OK") { model.error = nil }
        } message: { Text(model.error ?? "") }
        .sheet(item: Binding(get: { worktreeBase.map(IdentifiedString.init) }, set: { worktreeBase = $0?.value })) { b in
            AddWorktreeSheet(repo: app.repo(containing: worktree.path) ?? worktree.path, initialBase: b.value)
        }
    }

    private var scopeMenu: some View {
        Menu {
            Button("Current Branch") { model.setScope(.current) }
            Button("All Branches") { model.setScope(.all) }
            if !model.branches.local.isEmpty {
                Menu("Local Branches") { ForEach(model.branches.local, id: \.self) { b in Button(b) { model.setScope(.branch(b)) } } }
            }
            if !model.branches.remote.isEmpty {
                Menu("Remote Branches") { ForEach(model.branches.remote, id: \.self) { b in Button(b) { model.setScope(.branch(b)) } } }
            }
            Divider()
            Button("Stashes") { model.setScope(.stashes) }
        } label: {
            Label(model.scope.title.count > 18 ? String(model.scope.title.prefix(17)) + "…" : model.scope.title, systemImage: model.scope == .stashes ? "tray" : model.scope == .all ? "point.3.filled.connected.trianglepath.dotted" : "arrow.triangle.branch")
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .help("Which history to show")
    }

    private var branchName: String { worktree.branch ?? "HEAD" }

    /// The one-click actions in the commit header; the context menu offers the same.
    @ViewBuilder private func actions(_ c: Commit) -> some View {
        if c.selector != nil {
            Button("Drop…", role: .destructive) { dropping = c }
            Button("Apply") { Task { await model.applyStash(c, pop: false) } }.help("Apply the changes and keep the stash")
            Button("Pop") { Task { await model.applyStash(c, pop: true) } }.buttonStyle(.borderedProminent).help("Apply the changes and drop the stash")
        } else if model.notInHead.contains(c.sha) {
            Button("Cherry-pick onto \(branchName)") { Task { await model.cherryPick(c) } }.help("Apply this commit on top of \(branchName)")
        } else {
            Button("Revert") { Task { await model.revert(c) } }.help("Make a new commit on \(branchName) that undoes this one")
        }
    }

    @ViewBuilder private func menu(_ c: Commit) -> some View {
        if c.selector != nil {
            Button("Pop") { Task { await model.applyStash(c, pop: true) } }
            Button("Apply") { Task { await model.applyStash(c, pop: false) } }
            Button("Drop…", role: .destructive) { dropping = c }
            Divider()
            Button("Copy Message") { copy(c.subject) }
        } else {
            if model.notInHead.contains(c.sha) { Button("Cherry-pick onto \(branchName)") { Task { await model.cherryPick(c) } } }
            else { Button("Revert Commit") { Task { await model.revert(c) } } }
            Divider()
            Button("Copy SHA") { copy(c.sha) }
            Button("Copy Subject") { copy(c.subject) }
            Button("New Worktree from Here…") { worktreeBase = c.sha }
            if let r = worktree.remote, let a = worktree.account {
                Button("Open in Browser") { if let u = URL(string: "\(a.baseURL)/\(r.owner)/\(r.name)/commit/\(c.sha)") { NSWorkspace.shared.open(u) } }
            }
        }
    }

    private func copy(_ s: String) { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(s, forType: .string) }
}

struct CommitRow: View {
    var commit: Commit
    var isHead: Bool
    var client: ForgejoClient? = nil
    var remote: RemoteRepo? = nil
    var graph: GraphRow? = nil
    var graphWidth = 0

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            if let graph { GraphCell(row: graph, lanes: min(graph.width, graphWidth), nodeY: 21) }
            Avatar(user: FUser(login: commit.author, avatar_url: AuthorAvatars.shared.url(for: commit.email)), size: 26)
                .padding(.top, 8)
                .task { AuthorAvatars.shared.request(email: commit.email, sha: commit.sha, client: client, repo: remote) }
            details.padding(.vertical, 6)
        }
        .help("\(commit.subject)\n\(commit.author) <\(commit.email)>\n\(commit.date.formatted(date: .abbreviated, time: .shortened))")
    }

    private var details: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 4) {
                if isHead { Circle().fill(Color.accentColor).frame(width: 6, height: 6).help("HEAD") }
                Text(commit.subject).lineLimit(1).fontWeight(isHead ? .semibold : .regular)
            }
            HStack(spacing: 6) {
                Text(commit.short).font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(1).fixedSize()
                Text(commit.author).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                Spacer(minLength: 4)
                Text(commit.date, format: .relative(presentation: .named)).font(.caption).foregroundStyle(.tertiary).lineLimit(1)
            }
            if !commit.refs.isEmpty {
                HStack(spacing: 4) { ForEach(commit.refs, id: \.self) { RefBadge(ref: $0) } }.lineLimit(1)
            }
        }
    }
}

/// Draws one row's slice of the commit graph; rows stack with no gap, so slices join into lines.
struct GraphCell: View {
    var row: GraphRow
    var lanes: Int
    var nodeY: CGFloat
    static let lane: CGFloat = 11
    static let palette: [Color] = [.green, .purple, .pink, .teal, .yellow, .red, .indigo, .mint, .cyan, .brown, .orange]   // lane 0, the main line, is the accent

    var body: some View {
        Canvas { ctx, size in
            func x(_ i: Int) -> CGFloat { CGFloat(i) * Self.lane + Self.lane / 2 }
            func color(_ i: Int) -> Color { i == 0 ? .accentColor : Self.palette[(i - 1) % Self.palette.count] }
            let h = size.height, y = nodeY, c = x(row.col)
            let style = StrokeStyle(lineWidth: 1.6, lineCap: .round)
            for i in row.through where i < lanes {
                ctx.stroke(Path { $0.move(to: .init(x: x(i), y: 0)); $0.addLine(to: .init(x: x(i), y: h)) }, with: .color(color(i)), style: style)
            }
            guard row.col < lanes else { return }   // past the lane cap: lines only
            for i in row.into where i < lanes {
                ctx.stroke(Path { $0.move(to: .init(x: x(i), y: 0)); $0.addQuadCurve(to: .init(x: c, y: y), control: .init(x: x(i), y: y)) },
                           with: .color(color(i)), style: style)
            }
            for j in row.out where j < lanes {
                ctx.stroke(Path { $0.move(to: .init(x: c, y: y)); $0.addQuadCurve(to: .init(x: x(j), y: h), control: .init(x: x(j), y: y)) },
                           with: .color(color(j)), style: style)
            }
            let dot = CGRect(x: c - 4, y: y - 4, width: 8, height: 8)
            ctx.fill(Path(ellipseIn: dot), with: .color(color(row.col)))
            ctx.stroke(Path(ellipseIn: dot.insetBy(dx: -1, dy: -1)), with: .color(Color(nsColor: .windowBackgroundColor)), lineWidth: 1.5)
        }
        .frame(width: CGFloat(lanes) * Self.lane)
    }
}

struct RefBadge: View {
    var ref: String   // from --decorate=full: "HEAD -> refs/heads/x", "refs/remotes/origin/x", "tag: refs/tags/v1", "HEAD"
    var body: some View {
        let (name, icon, tint): (String, String, Color) = {
            if ref.hasPrefix("HEAD -> ") { return (String(ref.dropFirst(8)).replacingOccurrences(of: "refs/heads/", with: ""), "arrow.triangle.branch", .accentColor) }
            if ref.hasPrefix("tag: ") { return (String(ref.dropFirst(5)).replacingOccurrences(of: "refs/tags/", with: ""), "tag", .orange) }
            if ref.hasPrefix("refs/remotes/") { return (String(ref.dropFirst(13)), "cloud", .secondary) }
            if ref.hasPrefix("refs/heads/") { return (String(ref.dropFirst(11)), "arrow.triangle.branch", .green) }
            return (ref, "smallcircle.filled.circle", .accentColor)   // detached HEAD, refs/pull/*, stash…
        }()
        Label(name, systemImage: icon)
            .labelStyle(.titleAndIcon)
            .font(.caption2.weight(.medium))
            .lineLimit(1)
            .padding(.horizontal, 5).padding(.vertical, 1)
            .foregroundStyle(tint)
            .background(tint.opacity(0.12), in: Capsule())
    }
}

struct CommitDetailView: View {
    @Bindable var model: HistoryModel
    var remote: RemoteRepo?
    var account: ForgejoAccount?
    var layout: DiffLayout
    var actions: (Commit) -> AnyView

    var body: some View {
        if let c = model.selected {
            VStack(spacing: 0) {
                header(c)
                Divider()
                ResizableSplit(key: "commitTreeWidth", minWidth: 180, maxWidth: 300, defaultFraction: 1) {
                    FileTreeList(paths: model.files.map(\.path), selection: $model.selectedPath, collapsed: $model.collapsed) { path in
                        if let f = model.files.first(where: { $0.path == path }) {
                            TreeFileRow(path: f.path, badge: f.badge, additions: f.additions, deletions: f.deletions, isBinary: f.isBinary)
                        }
                    } dirAccessory: { _ in EmptyView() }
                    .frame(maxHeight: .infinity)
                    .overlay { if model.files.isEmpty && !model.detailLoading { ContentUnavailableView("No File Changes", systemImage: "doc") } }
                } trailing: {

                    Group {
                        if let f = model.selectedFile {
                            VStack(spacing: 0) {
                                DiffHeader(path: f.path, oldPath: f.oldPath != f.newPath ? f.oldPath : nil, additions: f.additions, deletions: f.deletions,
                                           file: f.newPath == nil || c.selector != nil ? nil : FileTarget(worktree: model.path, path: f.path, rev: c.sha)) { EmptyView() }
                                Divider()
                                if f.isBinary { ContentUnavailableView("Binary File", systemImage: "doc.zipper") }
                                else if f.lines.isEmpty { ContentUnavailableView("No Content Changes", systemImage: "doc", description: Text("Mode change or rename only.")) }
                                else { DiffTableView(file: f, layout: layout) }
                            }
                        } else if model.detailLoading {
                            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
                        } else {
                            Color.clear
                        }
                    }
                    .frame(minWidth: 300, maxWidth: .infinity, maxHeight: .infinity)
                }
            }
        } else {
            ContentUnavailableView("Select a Commit", systemImage: "clock.arrow.circlepath")
        }
    }

    private func header(_ c: Commit) -> some View {
        let lines = model.message.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false)
        let body = lines.count > 1 ? lines[1].trimmingCharacters(in: .whitespacesAndNewlines) : ""
        let adds = model.files.reduce(0) { $0 + $1.additions }, dels = model.files.reduce(0) { $0 + $1.deletions }
        return VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text(c.subject).font(.title3.weight(.semibold)).textSelection(.enabled).lineLimit(3)
                Spacer(minLength: 12)
                HStack(spacing: 6) { actions(c) }.controlSize(.small).fixedSize()
            }
            if !body.isEmpty {
                ScrollView { Text(body).font(.callout).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading) }
                    .frame(maxHeight: 120)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: 10) {
                Avatar(user: FUser(login: c.author, avatar_url: AuthorAvatars.shared.url(for: c.email)), size: 18)
                Text(c.author).fontWeight(.medium)
                Text(c.date.formatted(date: .abbreviated, time: .shortened)).foregroundStyle(.secondary)
                Button { copy(c.sha) } label: { Label(c.short, systemImage: "doc.on.doc").font(.callout.monospaced()) }
                    .buttonStyle(.borderless).help("Copy full SHA")
                if !c.parents.isEmpty {
                    Text(c.parents.count > 1 ? "merge of" : "parent").foregroundStyle(.tertiary)
                    ForEach(c.parents, id: \.self) { p in
                        Button(String(p.prefix(8))) { if model.commits.contains(where: { $0.sha == p }) { model.select(p) } }
                            .buttonStyle(.link).font(.callout.monospaced())
                            .help(model.commits.contains(where: { $0.sha == p }) ? "Go to parent" : "Parent not loaded yet — scroll the list to load more")
                    }
                }
                Spacer()
                Text(model.files.count == 1 ? "1 file" : "\(String(model.files.count)) files").foregroundStyle(.secondary)
                Text(verbatim: "+\(String(adds))").foregroundStyle(.green).monospacedDigit()
                Text(verbatim: "−\(String(dels))").foregroundStyle(.red).monospacedDigit()
                if model.detailLoading { ProgressView().controlSize(.small) }
            }
            .font(.callout)
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.bar)
    }

    private func copy(_ s: String) { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(s, forType: .string) }
}

/// Author email → Forgejo avatar URL. Git only knows names and emails, so each distinct email is resolved once
/// through one of its commits and remembered across launches ("" = no matching account, use initials).
@MainActor @Observable
final class AuthorAvatars {
    static let shared = AuthorAvatars()
    private(set) var urls: [String: String] = UserDefaults.standard.dictionary(forKey: "authorAvatars") as? [String: String] ?? [:]
    @ObservationIgnored private var queue: [(email: String, sha: String, client: ForgejoClient, repo: RemoteRepo)] = []
    @ObservationIgnored private var pending = Set<String>()
    @ObservationIgnored private var running = false

    func url(for email: String) -> String? { urls[email.lowercased()].flatMap { $0.isEmpty ? nil : $0 } }

    func request(email: String, sha: String, client: ForgejoClient?, repo: RemoteRepo?) {
        let key = email.lowercased()
        guard let client, let repo, urls[key] == nil, !pending.contains(key) else { return }
        pending.insert(key)
        queue.append((key, sha, client, repo))
        if !running { Task { await drain() } }
    }

    /// One request at a time: a history page can show dozens of authors at once.
    private func drain() async {
        running = true; defer { running = false }
        while !queue.isEmpty {
            let job = queue.removeFirst()
            if let user = try? await job.client.commitAuthor(job.repo, sha: job.sha) {
                urls[job.email] = user.avatar_url ?? ""
            } else {
                urls[job.email] = ""
            }
            pending.remove(job.email)
            UserDefaults.standard.set(urls, forKey: "authorAvatars")
        }
    }
}

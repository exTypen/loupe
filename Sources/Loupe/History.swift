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
    var id: String { sha }
    var short: String { String(sha.prefix(8)) }
}

extension Git {
    static func log(in dir: String, search: String, skip: Int, limit: Int) async throws -> [Commit] {
        var args = ["log", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1e", "--decorate=full", "--skip=\(skip)", "-n", "\(limit)"]
        if !search.isEmpty { args += ["-i", "--grep=\(search)", "--author=\(search)"] }   // OR'ed by git
        // Unborn branch (fresh repo) → git log fails; that's just an empty history.
        let data = try await run(args, in: dir, allowFailure: true)
        return parseLog(data)
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
}

@MainActor @Observable
final class HistoryModel {
    let path: String
    var commits: [Commit] = []
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
    }

    func loadMore() async {
        guard !loading, !exhausted else { return }
        await fetch(skip: commits.count, replace: false)
    }

    private func fetch(skip: Int, replace: Bool) async {
        loading = true; defer { loading = false }
        do {
            let batch = try await Git.log(in: path, search: search, skip: skip, limit: Self.page)
            exhausted = batch.count < Self.page
            commits = replace ? batch : commits + batch
        } catch { self.error = error.localizedDescription }
    }

    func select(_ sha: String?) {
        selectedSHA = sha
        detailTask?.cancel()
        guard let sha else { files = []; message = ""; return }
        detailTask = Task {
            detailLoading = true; defer { detailLoading = false }
            async let body = Git.string(["show", "-s", "--format=%B", sha], in: path)
            async let diff = Git.commitDiff(sha, in: path)
            do {
                let (b, d) = try await (body, diff)
                guard !Task.isCancelled else { return }
                message = b.trimmingCharacters(in: .whitespacesAndNewlines)
                files = d.sorted { FileTree.treeOrder($0.path, $1.path) }
                if !files.contains(where: { $0.path == selectedPath }) { selectedPath = files.first?.path }
            } catch { if !Task.isCancelled { self.error = error.localizedDescription } }
        }
    }

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

    var body: some View {
        HSplitView {
            VStack(spacing: 0) {
                TextField("Message or author", text: $model.search).textFieldStyle(.roundedBorder)
                    .onSubmit { Task { await model.reload() } }
                    .padding(8)
                Divider()
                List(selection: Binding(get: { model.selectedSHA }, set: { model.select($0) })) {
                    ForEach(model.commits) { c in
                        CommitRow(commit: c, isHead: c.sha == worktree.headOID, client: worktree.client, remote: worktree.remote)
                            .tag(c.sha)
                            .contextMenu { menu(c) }
                            .onAppear { if c.sha == model.commits.last?.sha { Task { await model.loadMore() } } }
                    }
                    if model.loading && !model.commits.isEmpty { ProgressView().controlSize(.small).frame(maxWidth: .infinity) }
                }
                .listStyle(.inset)
                .frame(maxHeight: .infinity)
                .overlay {
                    if model.commits.isEmpty && !model.loading {
                        ContentUnavailableView(model.search.isEmpty ? "No Commits" : "No Matches", systemImage: "clock.arrow.circlepath")
                    }
                }
            }
            .frame(minWidth: 260, idealWidth: 340, maxWidth: 420, maxHeight: .infinity)

            CommitDetailView(model: model, remote: worktree.remote, account: worktree.account, layout: layout)
                .frame(minWidth: 420, maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .task(id: worktree.headOID) { await model.reload() }
        .onChange(of: model.search) { _, s in if s.isEmpty { Task { await model.reload() } } }
        .onReceive(NotificationCenter.default.publisher(for: .moveFile)) { n in if let d = n.object as? Int { model.move(d) } }
        .onReceive(NotificationCenter.default.publisher(for: .refresh)) { _ in Task { await model.reload() } }
        .alert("Git", isPresented: Binding(get: { model.error != nil }, set: { if !$0 { model.error = nil } })) {
            Button("OK") { model.error = nil }
        } message: { Text(model.error ?? "") }
        .sheet(item: Binding(get: { worktreeBase.map(IdentifiedString.init) }, set: { worktreeBase = $0?.value })) { b in
            AddWorktreeSheet(repo: app.repo(containing: worktree.path) ?? worktree.path, initialBase: b.value)
        }
    }

    @ViewBuilder private func menu(_ c: Commit) -> some View {
        Button("Copy SHA") { copy(c.sha) }
        Button("Copy Subject") { copy(c.subject) }
        Button("New Worktree from Here…") { worktreeBase = c.sha }
        if let r = worktree.remote, let a = worktree.account {
            Button("Open in Browser") { if let u = URL(string: "\(a.baseURL)/\(r.owner)/\(r.name)/commit/\(c.sha)") { NSWorkspace.shared.open(u) } }
        }
    }

    private func copy(_ s: String) { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(s, forType: .string) }
}

struct CommitRow: View {
    var commit: Commit
    var isHead: Bool
    var client: ForgejoClient? = nil
    var remote: RemoteRepo? = nil

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            Avatar(user: FUser(login: commit.author, avatar_url: AuthorAvatars.shared.url(for: commit.email)), size: 26)
                .padding(.top, 2)
                .task { AuthorAvatars.shared.request(email: commit.email, sha: commit.sha, client: client, repo: remote) }
            details
        }
        .padding(.vertical, 2)
        .help("\(commit.subject)\n\(commit.author) <\(commit.email)>\n\(commit.date.formatted(date: .abbreviated, time: .shortened))")
    }

    private var details: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 4) {
                if isHead { Circle().fill(Color.accentColor).frame(width: 6, height: 6).help("HEAD") }
                Text(commit.subject).lineLimit(1).fontWeight(isHead ? .semibold : .regular)
            }
            HStack(spacing: 6) {
                Text(commit.short).font(.caption.monospaced()).foregroundStyle(.secondary)
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

    var body: some View {
        if let c = model.selected {
            VStack(spacing: 0) {
                header(c)
                Divider()
                HSplitView {
                    FileTreeList(paths: model.files.map(\.path), selection: $model.selectedPath, collapsed: $model.collapsed) { path in
                        if let f = model.files.first(where: { $0.path == path }) {
                            TreeFileRow(path: f.path, badge: f.badge, additions: f.additions, deletions: f.deletions, isBinary: f.isBinary)
                        }
                    } dirAccessory: { _ in EmptyView() }
                    .frame(minWidth: 180, idealWidth: 240, maxWidth: 300, maxHeight: .infinity)
                    .overlay { if model.files.isEmpty && !model.detailLoading { ContentUnavailableView("No File Changes", systemImage: "doc") } }

                    Group {
                        if let f = model.selectedFile {
                            VStack(spacing: 0) {
                                DiffHeader(path: f.path, oldPath: f.oldPath != f.newPath ? f.oldPath : nil, additions: f.additions, deletions: f.deletions) { EmptyView() }
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
            Text(c.subject).font(.title3.weight(.semibold)).textSelection(.enabled).lineLimit(3)
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

import SwiftUI

// MARK: - API

extension ForgejoClient {
    /// The body of POST /repos/{o}/{r}/pulls. GitHub takes `draft`; Forgejo and Gitea mark a draft by its title.
    struct NewPull: Encodable, Equatable {
        var head: String
        var base: String
        var title: String
        var body: String
        var draft: Bool?
    }

    static func newPull(head: String, base: String, title: String, body: String, draft: Bool, github: Bool) -> NewPull {
        let t = title.trimmingCharacters(in: .whitespacesAndNewlines)
        if github { return NewPull(head: head, base: base, title: t, body: body, draft: draft ? true : nil) }
        let marked = ["wip:", "[wip]"].contains { t.lowercased().hasPrefix($0) }
        return NewPull(head: head, base: base, title: draft && !marked ? "WIP: " + t : t, body: body)
    }

    struct PullExists: LocalizedError { var errorDescription: String? { "A pull request for this branch already exists." } }

    /// Forgejo answers 409, GitHub 422 with the reason in `errors[].message` (its `message` is just "Validation Failed").
    static func createError(status: Int, data: Data, server: String) -> Error {
        let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        let reasons = (json?["errors"] as? [[String: Any]])?.compactMap { $0["message"] as? String } ?? []
        let text = (reasons.isEmpty ? [json?["message"] as? String ?? String(decoding: data.prefix(300), as: UTF8.self)] : reasons).joined(separator: " ")
        if (status == 409 || status == 422) && text.lowercased().contains("already exists") { return PullExists() }
        return GitError(message: "\(server) \(status): \(text)")
    }

    func createPull(_ r: RemoteRepo, _ p: NewPull) async throws -> PullRequest {
        let (data, resp) = try await URLSession.shared.data(for: try request("POST", "/repos/\(r.owner)/\(r.name)/pulls", body: p))
        guard let http = resp as? HTTPURLResponse else { throw GitError(message: "No HTTP response") }
        guard (200..<300).contains(http.statusCode) else { throw Self.createError(status: http.statusCode, data: data, server: github ? "GitHub" : "Forgejo") }
        return try JSONDecoder().decode(PullRequest.self, from: data)
    }

    /// The open pull request whose head is `branch` in this repository, if any.
    func openPull(_ r: RemoteRepo, head branch: String) async throws -> PullRequest? {
        if github {
            let list: [PullRequest] = try await get("/repos/\(r.owner)/\(r.name)/pulls", query: ["state": "open", "head": "\(r.owner):\(branch)"])
            return list.first
        }
        // Forgejo's list has no head filter; the open list is the short one. A fork's PR can share the branch name
        // (patch-1), so only a head in this same repository counts.
        let list: [HeadOnly] = try await getAll("/repos/\(r.owner)/\(r.name)/pulls", query: ["state": "open"])
        guard let n = Self.sameRepoPull(list, head: branch) else { return nil }
        return try await pull(r, n)
    }

    struct HeadOnly: Decodable {
        struct Ref: Decodable { var ref: String; var repo_id: Int? }
        var number: Int; var head: Ref; var base: Ref
    }

    static func sameRepoPull(_ list: [HeadOnly], head branch: String) -> Int? {
        list.first { $0.head.ref == branch && $0.head.repo_id == $0.base.repo_id }?.number
    }

    func defaultBranch(_ r: RemoteRepo) async throws -> String {
        struct R: Decodable { var default_branch: String }
        let repo: R = try await get("/repos/\(r.owner)/\(r.name)")
        return repo.default_branch
    }
}

// MARK: - Defaults (pure, tested)

enum PullDraft {
    struct Commit: Hashable, Identifiable {
        var sha: String
        var subject: String
        var body: String
        var id: String { sha }
    }

    /// `git log --format=%H%x1f%s%x1f%b%x1e`
    static let logFormat = "--format=%H%x1f%s%x1f%b%x1e"

    static func parseLog(_ s: String) -> [Commit] {
        s.split(separator: "\u{1e}").compactMap { rec in
            let f = rec.split(separator: "\u{1f}", maxSplits: 2, omittingEmptySubsequences: false)
            let sha = f[0].trimmingCharacters(in: .whitespacesAndNewlines)
            guard !sha.isEmpty, f.count >= 2 else { return nil }
            return Commit(sha: sha, subject: String(f[1]), body: f.count > 2 ? f[2].trimmingCharacters(in: .whitespacesAndNewlines) : "")
        }
    }

    /// One commit: its subject. More: the branch name read as words ("feat/add-login_page" → "Add login page").
    static func title(commits: [Commit], branch: String) -> String {
        if commits.count == 1 { return commits[0].subject }
        return humanize(branch)
    }

    static func humanize(_ branch: String) -> String {
        let last = branch.split(separator: "/").last.map(String.init) ?? branch
        let words = last.replacingOccurrences(of: "-", with: " ").replacingOccurrences(of: "_", with: " ")
            .split(separator: " ").joined(separator: " ")
        return words.prefix(1).uppercased() + words.dropFirst()
    }

    /// The template the forge itself would use: its own folder first, then the others', then docs/ and the root.
    static func templatePaths(github: Bool) -> [String] {
        let dirs = github ? [".github", ".forgejo", ".gitea"] : [".forgejo", ".gitea", ".github"]
        return (dirs + ["docs", ""]).flatMap { d in
            ["pull_request_template.md", "PULL_REQUEST_TEMPLATE.md"].map { d.isEmpty ? $0 : "\(d)/\($0)" }
        }
    }

    static func template(in dir: String, github: Bool) -> String? {
        for p in templatePaths(github: github) {
            if let s = try? String(contentsOfFile: (dir as NSString).appendingPathComponent(p), encoding: .utf8) {
                let t = s.trimmingCharacters(in: .whitespacesAndNewlines)
                if !t.isEmpty { return t }
            }
        }
        return nil
    }

    /// The template when there is one, else a lone commit's description.
    static func body(commits: [Commit], template: String?) -> String {
        template ?? (commits.count == 1 ? commits[0].body : "")
    }

    static func prompt(commits: [Commit], base: String, branch: String, stat: String, diff: String, template: String?) -> String {
        let limit = 60_000
        let patch = diff.utf8.count > limit ? String(diff.prefix(limit)) + "\n[diff truncated]" : diff
        let log = commits.map { c in c.subject + (c.body.isEmpty ? "" : "\n" + c.body.split(separator: "\n").map { "    " + $0 }.joined(separator: "\n")) }
            .joined(separator: "\n")
        let extra = CommitAI.extraInstructions.trimmingCharacters(in: .whitespacesAndNewlines)
        return """
        You write pull request titles and descriptions. Below are the commits of branch \(branch), which is to be merged \
        into \(base), then the change.

        Write a title of at most 72 characters that says what the pull request does, in the style of the commit subjects. \
        Then a description in Markdown for a reviewer: what changed and why, and anything they should look at closely. \
        Be brief; don't list every file.\(template.map { "\nFill in this pull request template, keeping its headings and checkboxes:\n\($0)\n" } ?? "")\(extra.isEmpty ? "" : "\n\(extra)")

        Reply with only the title on the first line, a blank line, then the description. No quotes, no code fences around \
        the reply, no preamble. Don't run tools or look at files; everything you need is here.

        Commits:
        \(log)

        Summary:
        \(stat)
        Diff:
        \(patch)
        """
    }

    /// First line: the title (a leading "# " or "Title:" dropped); the rest: the description.
    static func splitReply(_ s: String) -> (title: String, body: String) {
        let parts = s.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false)
        var t = String(parts.first ?? "").trimmingCharacters(in: .whitespaces)
        for p in ["# ", "Title:", "title:"] where t.hasPrefix(p) { t = String(t.dropFirst(p.count)).trimmingCharacters(in: .whitespaces) }
        if t.count > 1, t.first == "\"", t.last == "\"" { t = String(t.dropFirst().dropLast()) }
        return (t, parts.count > 1 ? parts[1].trimmingCharacters(in: .whitespacesAndNewlines) : "")
    }
}

extension Notification.Name {
    static let branchPushed = Notification.Name("loupe.branchPushed")   // object: the worktree path
}

// MARK: - Sheet model

@MainActor @Observable
final class PullCreateModel {
    let worktree: WorktreeModel
    let branch: String
    var bases: [String] = []
    var base = ""
    var title = ""
    var body = ""
    var draft = false
    var commits: [PullDraft.Commit] = []   // the newest 100
    var total = 0                           // all of them
    var template: String?
    var needsPush = false
    var loading = true
    var working: String?            // "Pushing", "Creating", "Writing"
    var error: String?
    var existing: PullRequest?      // the server says one is open already: offer it
    @ObservationIgnored private var autoTitle = ""
    @ObservationIgnored private var autoBody = ""

    init(worktree: WorktreeModel) {
        self.worktree = worktree
        branch = worktree.branch ?? ""
    }

    private var path: String { worktree.path }
    private var remoteName: String { worktree.remoteName }

    func load() async {
        defer { loading = false }
        guard let client = worktree.client, let remote = worktree.remote else { error = "No account for this repository's remote."; return }
        guard !branch.isEmpty else { error = "HEAD is detached; switch to a branch first."; return }
        template = PullDraft.template(in: path, github: client.github)
        let out = (try? await Git.string(["for-each-ref", "--format=%(refname:lstrip=3)", "--sort=-committerdate", "refs/remotes/\(remoteName)"], in: path)) ?? ""
        var names = out.split(separator: "\n").map(String.init).filter { $0 != "HEAD" && $0 != branch }
        let def: String? = if let d = try? await client.defaultBranch(remote) { d }
            else { await Git.integrationTarget(remote: remoteName, in: path).map { $0.name.hasPrefix(remoteName + "/") ? String($0.name.dropFirst(remoteName.count + 1)) : $0.name } }
        if let def, def != branch, !names.contains(def) { names.insert(def, at: 0) }
        bases = Array(names.prefix(200))
        base = def.flatMap { names.contains($0) ? $0 : nil } ?? names.first ?? ""
        existing = try? await client.openPull(remote, head: branch)
        await baseChanged()
    }

    /// Commits, title and body follow the base until the user edits them.
    func baseChanged() async {
        let sel = base, dir = path, range = "\(remoteName)/\(sel)..HEAD"
        // Capped: an unrelated base (gh-pages) would otherwise list, and parse on the main actor, the whole history.
        async let log = Git.string(["log", "-n", "100", PullDraft.logFormat, range], in: dir)
        async let count = Git.string(["rev-list", "--count", range], in: dir)
        let parsed = PullDraft.parseLog((try? await log) ?? ""), n = Int(((try? await count) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)) ?? 0
        guard sel == base else { return }   // the base changed again meanwhile; that call fills in
        commits = parsed; total = max(n, parsed.count)
        let t = PullDraft.title(commits: commits, branch: branch), b = PullDraft.body(commits: commits, template: template)
        if title == autoTitle { title = t }
        if body == autoBody { body = b }
        autoTitle = t; autoBody = b
        // Already on the remote (or behind it): nothing to push.
        let push = (try? await Git.run(["merge-base", "--is-ancestor", "HEAD", "\(remoteName)/\(branch)"], in: path)) == nil
        if sel == base { needsPush = push }
    }

    func writeWithAI() async {
        working = "Writing"; defer { working = nil }
        error = nil
        let range = "\(remoteName)/\(base)...HEAD"
        let args = ["diff", "--no-color", "--no-ext-diff", "-M", range]
        do {
            async let diff = Git.string(args + ["-U2"], in: path)
            async let stat = Git.string(args + ["--stat"], in: path)
            let prompt = PullDraft.prompt(commits: commits, base: base, branch: branch, stat: try await stat, diff: try await diff, template: template)
            let reply = PullDraft.splitReply(try await CommitAI.answer(prompt, in: path))
            if !reply.title.isEmpty { title = reply.title }
            body = reply.body
        } catch { self.error = error.localizedDescription }
    }

    /// Pushes the branch when the remote doesn't have its head yet (the toolbar's push, with the space's credentials),
    /// then opens the pull request and shows it.
    func create() async {
        guard let client = worktree.client, let remote = worktree.remote else { return }
        error = nil; existing = nil
        do {
            if needsPush {
                working = "Pushing"
                try await Git.run(["push", "--set-upstream", remoteName, "HEAD"], in: path, env: worktree.gitAuth)
                needsPush = false
                await worktree.refreshStatus()
            }
            working = "Creating"
            let pr = try await client.createPull(remote, ForgejoClient.newPull(head: branch, base: base, title: title, body: body, draft: draft, github: client.github))
            working = nil
            await show(pr)
        } catch is ForgejoClient.PullExists {
            working = nil
            existing = try? await client.openPull(remote, head: branch)
            error = ForgejoClient.PullExists().errorDescription
        } catch {
            working = nil
            self.error = error.localizedDescription
        }
    }

    /// Closes the sheet and opens the pull request in the Pull Requests tab, the list refreshed so it's in it.
    func show(_ pr: PullRequest) async {
        let app = worktree.app, list = worktree.pulls
        app.creatingPull = nil
        app.selection = path
        UserDefaults.standard.set("pulls", forKey: "tab")
        list.selected = nil
        if let c = worktree.client, let r = worktree.remote { await list.load(c, r) }
        if !list.pulls.contains(where: { $0.number == pr.number }) { list.pulls.insert(pr, at: 0) }
        list.selected = pr.number
    }
}

// MARK: - Sheet

struct PullCreateSheet: View {
    @State private var model: PullCreateModel
    @Environment(AppModel.self) private var app
    @AppStorage("aiHarness") private var aiHarness = CommitAI.defaultHarness   // re-render when Settings changes it

    init(worktree: WorktreeModel) { _model = State(initialValue: PullCreateModel(worktree: worktree)) }

    var body: some View {
        @Bindable var m = model
        Form {
            Section {
                LabeledContent("Branch") {
                    HStack(spacing: 6) {
                        Text(m.branch).font(.body.monospaced()).lineLimit(1).truncationMode(.middle).help(m.branch)
                        if m.needsPush { Text("pushed first").font(.caption).foregroundStyle(.secondary).fixedSize() }
                    }
                }
                Picker("Into", selection: $m.base) {
                    ForEach(m.bases, id: \.self) { Text($0).tag($0) }
                }
                .onChange(of: m.base) { if !model.loading { Task { await model.baseChanged() } } }
                HStack(spacing: 6) {
                    TextField("Title", text: $m.title)
                    if let h = CommitAI.selected {
                        if m.working == "Writing" { ProgressView().controlSize(.small).frame(width: 20) }
                        else {
                            Button { Task { await model.writeWithAI() } } label: {
                                Image(systemName: "sparkles").foregroundStyle(m.commits.isEmpty ? Color.secondary : Color.accentColor)
                            }
                            .buttonStyle(.borderless)
                            .disabled(m.commits.isEmpty || m.working != nil)
                            .help("Write the title and description with \(h.name), from the commits and their diff")
                        }
                    }
                }
                TextEditor(text: $m.body)
                    .font(.body)
                    .frame(height: 160)
                    .overlay(alignment: .topLeading) {
                        if m.body.isEmpty { Text("Description").foregroundStyle(.tertiary).padding(.leading, 5).allowsHitTesting(false) }
                    }
                Toggle("Draft", isOn: $m.draft)
                    .help(model.worktree.client?.github == true ? "Open it as a draft pull request" : "Open it as a draft: the title starts with “WIP: ”")
            }
            Section(m.total == 1 ? "1 commit" : "\(m.total) commits") {
                if m.commits.isEmpty && !m.loading {
                    Text("No commits on \(m.branch) that aren't on \(m.base).").foregroundStyle(.secondary)
                }
                ForEach(m.commits) { c in
                    HStack(spacing: 8) {
                        Text(c.sha.prefix(7)).font(.caption.monospaced()).foregroundStyle(.secondary)
                        Text(c.subject).lineLimit(1).truncationMode(.tail)
                    }
                    .help(c.body.isEmpty ? c.subject : c.subject + "\n\n" + c.body)
                }
                if m.total > m.commits.count { Text("and \(m.total - m.commits.count) more").foregroundStyle(.secondary) }
            }
            if m.error != nil || m.existing != nil {
                Section {
                    if let e = m.error { Text(e).foregroundStyle(.red).font(.callout).textSelection(.enabled) }
                    if let pr = m.existing {
                        HStack {
                            Text(verbatim: "#\(pr.number) \(pr.title)").lineLimit(1)
                            Spacer()
                            Button("Open Pull Request") { Task { await model.show(pr) } }
                        }
                    }
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 560)
        .frame(minHeight: 520)
        .overlay { if m.loading { ProgressView() } }
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button("Cancel") { app.creatingPull = nil } }
            ToolbarItem(placement: .confirmationAction) {
                HStack(spacing: 8) {
                    if let w = m.working, w != "Writing" { ProgressView().controlSize(.small) }
                    Button(m.needsPush ? "Push and Create" : "Create Pull Request") { Task { await model.create() } }
                        .keyboardShortcut(.defaultAction)
                        .disabled(m.loading || m.working != nil || m.base.isEmpty || m.commits.isEmpty
                                  || m.title.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
        .navigationTitle("New Pull Request")
        .task { await model.load() }
    }
}

// MARK: - Banner (Changes)

/// Over the Changes list: the branch has work that isn't on the default branch and no open pull request.
struct PullCreateBanner: View {
    var model: WorktreeModel
    @State private var ahead = 0
    @State private var target = ""
    @State private var pushes = 0
    @AppStorage("prBannerDismissed") private var dismissed = ""   // "<path>\t<branch>" lines

    private var key: String { "\(model.path)\t\(model.branch ?? "")" }

    var body: some View {
        Group {
            if ahead > 0, !dismissed.split(separator: "\n").contains(Substring(key)) {
                VStack(spacing: 0) {
                    HStack(spacing: 10) {
                        Image(systemName: "arrow.triangle.pull").foregroundStyle(Color.accentColor)
                        Text("\(ahead == 1 ? "1 commit" : "\(ahead) commits") on \(model.branch ?? "") that \(ahead == 1 ? "isn't" : "aren't") on \(target)")
                            .lineLimit(1).truncationMode(.middle)
                            .help("\(model.branch ?? "") → \(target)")
                        Spacer()
                        Button("Create Pull Request…") { model.app.creatingPull = model.path }
                            .buttonStyle(.borderedProminent)
                        Button { dismissed += (dismissed.isEmpty ? "" : "\n") + key } label: { Image(systemName: "xmark") }
                            .buttonStyle(.borderless).help("Don't offer a pull request for this branch again")
                    }
                    .controlSize(.small)
                    .padding(.horizontal, 12).padding(.vertical, 8)
                    .background(Color.accentColor.opacity(0.08))
                    Divider()
                }
            }
        }
        .task(id: "\(model.headOID) \(model.branch ?? "") \(pushes) \(model.client != nil)") { await check() }
        .onReceive(NotificationCenter.default.publisher(for: .branchPushed)) { n in if n.object as? String == model.path { pushes += 1 } }
        .onChange(of: model.app.creatingPull) { old, new in if old == model.path && new == nil { pushes += 1 } }
    }

    private func check() async {
        guard let branch = model.branch, let client = model.client, let remote = model.remote,
              let (n, t) = await model.aheadOfDefault(), !model.isMerged else { ahead = 0; return }
        // Offline or no access: say nothing rather than offer something that will fail.
        do { if try await client.openPull(remote, head: branch) != nil { ahead = 0; return } } catch { ahead = 0; return }
        target = t
        ahead = n
    }
}

extension WorktreeModel {
    /// Commits on the branch that aren't on the default branch, and that branch's name; nil when there are none
    /// or the branch is the default one.
    func aheadOfDefault() async -> (count: Int, target: String)? {
        guard let branch, let t = await Git.integrationTarget(remote: remoteName, in: path),
              t.name != branch, t.name != "\(remoteName)/\(branch)" else { return nil }
        let n = Int((try? await Git.string(["rev-list", "--count", "\(t.name)..HEAD"], in: path))?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? "") ?? 0
        return n > 0 ? (n, t.name.hasPrefix(remoteName + "/") ? String(t.name.dropFirst(remoteName.count + 1)) : t.name) : nil
    }
}

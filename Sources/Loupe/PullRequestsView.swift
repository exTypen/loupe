import SwiftUI

struct PullRequestsView: View {
    @Environment(AppModel.self) private var app
    var model: WorktreeModel
    var layout: DiffLayout

    var body: some View {
        if let remote = model.remote, let client = model.client {
            PullBrowser(model: model, list: model.pulls, client: client, remote: remote, me: app.account(for: remote)?.username, layout: layout)
        } else if !model.remoteDetected || !app.tokensLoaded {
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let remote = model.remote {
            ContentUnavailableView {
                Label("Connect \(remote.host)", systemImage: "person.crop.circle.badge.plus")
            } description: {
                Text("Add a \(remote.host == "github.com" ? "GitHub" : "Forgejo") account for \(remote.host) to review pull requests for \(remote.owner)/\(remote.name).")
            } actions: {
                SettingsLink { Text("Open Settings…") }.buttonStyle(.borderedProminent)
            }
            .onChange(of: app.accounts) { Task { await model.detectRemote() } }
        } else {
            ContentUnavailableView("No Remote", systemImage: "network.slash", description: Text("This repository has no remote that looks like a Forgejo or GitHub repo."))
        }
    }
}

struct PullBrowser: View {
    var model: WorktreeModel
    @Bindable var list: PullListModel
    var client: ForgejoClient
    var remote: RemoteRepo
    var me: String?
    var layout: DiffLayout
    @State private var search = ""

    @Environment(AppModel.self) private var app
    private var worktreeBranches: Set<String> {
        Set((app.worktrees[app.repo(containing: model.path) ?? ""] ?? []).compactMap(\.branch))
    }

    private var visible: [PullRequest] {
        list.pulls.filter { pr in
            (!list.mine || pr.user.login.caseInsensitiveCompare(me ?? "") == .orderedSame)
                && (search.isEmpty || pr.title.localizedCaseInsensitiveContains(search) || "#\(pr.number)".contains(search) || pr.user.login.localizedCaseInsensitiveContains(search))
        }
    }

    var body: some View {
        Group {
            // A big PR needs every pixel: the detail replaces the list instead of sitting beside it.
            if let n = list.selected, let pr = list.pulls.first(where: { $0.number == n }) ?? list.details[n]?.pr {
                PullDetailView(pull: list.detail(for: pr, client: client, repo: remote, local: LocalCheckout(dir: model.path, remote: model.remoteName)), worktree: model, layout: layout) { list.selected = nil }
                    .id(n)
            } else {
                VStack(spacing: 0) {
                    HStack {
                        Picker("State", selection: $list.state) {
                            Text("Open").tag("open"); Text("Closed").tag("closed"); Text("All").tag("all")
                        }
                        .pickerStyle(.segmented).labelsHidden().fixedSize()
                        Toggle(isOn: $list.mine) { Label("Opened by me", systemImage: "person") }
                            .toggleStyle(.button)
                            .disabled(me == nil)
                            .help(me.map { "Only pull requests opened by \($0)" } ?? "Add a Forgejo account to filter by author")
                        TextField("Filter pull requests", text: $search).textFieldStyle(.roundedBorder).frame(maxWidth: 320)
                        Spacer()
                        if list.loading { ProgressView().controlSize(.small) }
                        else if let t = list.lastUpdated {
                            TimelineView(.periodic(from: .now, by: 5)) { _ in
                                Text("Updated \(t, format: .relative(presentation: .numeric))").font(.caption).foregroundStyle(.tertiary)
                            }
                        }
                        Button { Task { await list.load(client, remote) } } label: { Image(systemName: "arrow.clockwise") }
                            .buttonStyle(.borderless).help("Refresh (⌘R)")
                        Text("\(remote.owner)/\(remote.name)").foregroundStyle(.secondary)
                    }
                    .padding(10)
                    Divider()
                    List(selection: $list.selected) {
                        // PRs from the selected worktree's branch first, then other local worktrees', then the rest.
                        let current = visible.filter { $0.head.ref == model.branch }
                        let local = visible.filter { $0.head.ref != model.branch && worktreeBranches.contains($0.head.ref) }
                        let rest = visible.filter { $0.head.ref != model.branch && !worktreeBranches.contains($0.head.ref) }
                        if !current.isEmpty {
                            Section("This Branch") { ForEach(current) { PullRow(pr: $0, isLocal: true).tag($0.number) } }
                        }
                        if !local.isEmpty {
                            Section("Other Worktrees") { ForEach(local) { PullRow(pr: $0, isLocal: true).tag($0.number) } }
                        }
                        Section(current.isEmpty && local.isEmpty ? "" : "All") {
                            ForEach(rest) { pr in
                                PullRow(pr: pr).tag(pr.number)
                                    .onAppear { if pr.id == list.pulls.last?.id { Task { await list.loadMore(client, remote) } } }
                            }
                            if list.loadingMore { ProgressView().controlSize(.small).frame(maxWidth: .infinity) }
                        }
                    }
                    .listStyle(.inset)
                    .overlay {
                        if list.loading && list.pulls.isEmpty { ProgressView() }
                        else if let e = list.error { ContentUnavailableView("Couldn't Load", systemImage: "exclamationmark.triangle", description: Text(e)) }
                        else if visible.isEmpty { ContentUnavailableView(list.mine ? "No Pull Requests by You" : "No Pull Requests", systemImage: "arrow.triangle.pull") }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        // Keep the list in sync with the server: poll while visible, and refresh on returning to the app or the list.
        .task(id: list.state) {
            list.restoreCache(remote)   // instant list from the last session; the fetch below replaces it
            await list.load(client, remote)
            // Only the open list auto-refreshes: closed/all are paged and refreshing would throw away scrolled pages.
            while !Task.isCancelled, list.state == "open" {
                try? await Task.sleep(for: .seconds(60))
                await list.load(client, remote)
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in Task { await list.load(client, remote) } }
        .onChange(of: list.selected) { _, sel in if sel == nil { Task { await list.load(client, remote) } } }
        .onReceive(NotificationCenter.default.publisher(for: .refresh)) { _ in Task { await list.load(client, remote) } }
    }
}

struct PullRow: View {
    var pr: PullRequest
    var isLocal = false

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Avatar(user: pr.user, size: 28).padding(.top, 1)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 5) {
                    Image(systemName: pr.merged == true ? "arrow.triangle.merge" : pr.state == "closed" ? "xmark.circle" : "arrow.triangle.pull")
                        .foregroundStyle(pr.merged == true ? .purple : pr.state == "closed" ? .red : .green)
                    Text(pr.title).lineLimit(2).fontWeight(.medium)
                }
                HStack(spacing: 6) {
                    Text(verbatim: "#\(pr.number)").monospacedDigit()
                    Text(pr.user.login).fontWeight(.medium)
                    if pr.draft == true { Text("Draft").padding(.horizontal, 4).background(.quaternary, in: Capsule()) }
                    Text(pr.head.ref).font(.caption.monospaced()).foregroundStyle(isLocal ? Color.accentColor : Color.secondary).lineLimit(1).truncationMode(.middle)
                    if isLocal { Image(systemName: "arrow.triangle.branch").foregroundStyle(Color.accentColor).help("Checked out in a local worktree") }
                    Text("→ \(pr.base.ref)").font(.caption.monospaced()).foregroundStyle(.tertiary).lineLimit(1)
                    Spacer()
                    if let c = pr.comments, c > 0 { Label(String(c), systemImage: "bubble.left").labelStyle(.titleAndIcon).fixedSize() }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 4)
    }
}

/// One in-flight request per avatar URL and an in-memory cache. AsyncImage re-requests per row and
/// gets cancelled when List re-renders, which left most rows on the fallback.
@MainActor final class AvatarCache {
    static let shared = AvatarCache()
    private var images: [URL: NSImage] = [:]
    private var tasks: [URL: Task<NSImage?, Never>] = [:]

    func cached(_ url: URL) -> NSImage? { images[url] }

    func load(_ url: URL) async -> NSImage? {
        if let img = images[url] { return img }
        if let t = tasks[url] { return await t.value }
        let t = Task<NSImage?, Never> {
            guard let (data, _) = try? await URLSession.shared.data(from: url) else { return nil }
            return NSImage(data: data)
        }
        tasks[url] = t
        let img = await t.value
        tasks[url] = nil
        if let img { images[url] = img }
        return img
    }
}

/// Forgejo avatar with an initials fallback while loading or when it can't be fetched.
struct Avatar: View {
    var user: FUser
    var size: CGFloat = 20
    @State private var image: NSImage?

    private var url: URL? { user.avatar_url.flatMap(URL.init(string:)) }

    var body: some View {
        Group {
            if let img = image ?? url.flatMap(AvatarCache.shared.cached) { Image(nsImage: img).resizable().scaledToFill() }
            else { initials }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .overlay(Circle().strokeBorder(.separator, lineWidth: 0.5))
        .help(user.login)
        .task(id: url) { if let url, image == nil { image = await AvatarCache.shared.load(url) } }
    }

    private var initials: some View {
        let hue = Double(user.login.unicodeScalars.reduce(0) { ($0 &* 31 &+ Int($1.value)) & 0xffff } % 360) / 360
        return ZStack {
            Color(hue: hue, saturation: 0.45, brightness: 0.75)
            Text(String(user.login.prefix(1)).uppercased()).font(.system(size: size * 0.45, weight: .semibold)).foregroundStyle(.white)
        }
    }
}

// MARK: - Detail

struct PullDetailView: View {
    @Environment(AppModel.self) private var app
    @Bindable var pull: PullRequestModel
    var worktree: WorktreeModel
    var layout: DiffLayout
    var back: () -> Void
    @State private var tab = "files"
    @State private var showSubmit = false

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if tab == "files" { files } else { ConversationView(pull: pull) }
        }
        // Always refresh on open (cached content stays on screen meanwhile), then keep polling while it's open:
        // new pushes dismiss approvals and re-request reviewers, and the header must reflect that.
        .task {
            while !Task.isCancelled {
                await pull.load()
                try? await Task.sleep(for: .seconds(60))
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in Task { await pull.load() } }
        .onReceive(NotificationCenter.default.publisher(for: .refresh)) { _ in Task { await pull.load() } }
        .onReceive(NotificationCenter.default.publisher(for: .moveFile)) { n in if let d = n.object as? Int { pull.move(d) } }
        .onReceive(NotificationCenter.default.publisher(for: .markViewed)) { _ in pull.markViewedAndAdvance() }
        .alert("Forgejo", isPresented: Binding(get: { pull.error != nil }, set: { if !$0 { pull.error = nil } })) {
            Button("OK") { pull.error = nil }
        } message: { Text(pull.error ?? "") }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Button(action: back) { Label("Pull Requests", systemImage: "chevron.left") }
                    .buttonStyle(.borderless).keyboardShortcut("[", modifiers: .command)
                Text(pull.pr.title).font(.title3.weight(.semibold)).lineLimit(1).textSelection(.enabled)
                Text(verbatim: "#\(pull.pr.number)").font(.title3).foregroundStyle(.secondary)
                Spacer()
                if pull.loading { ProgressView().controlSize(.small) }
            }
            HStack(spacing: 10) {
                Avatar(user: pull.pr.user, size: 18)
                Text(pull.pr.user.login).fontWeight(.medium).lineLimit(1).fixedSize()
                Text("\(pull.pr.head.ref) → \(pull.pr.base.ref)").font(.callout.monospaced()).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                if let a = pull.pr.additions, let d = pull.pr.deletions {
                    Text(verbatim: "+\(String(a))").foregroundStyle(.green).monospacedDigit().fixedSize()
                    Text(verbatim: "−\(String(d))").foregroundStyle(.red).monospacedDigit().fixedSize()
                }
                reviewBadges
                Spacer()
                ChecksBadge(model: pull.checks) {
                    withAnimation(.easeOut(duration: 0.15)) { app.checksOverlay = (pull.checks, pull.client.baseURL) }
                }
                Picker("", selection: $tab) {
                    Text("Files \(pull.files.count)").tag("files")
                    Text("Conversation").tag("conversation")
                }
                .pickerStyle(.segmented).labelsHidden().fixedSize()
                Menu {
                    Button("Check Out in New Worktree") { Task { await worktree.checkoutInWorktree(pull.pr) } }
                    Button("Open in Browser") { if let u = URL(string: pull.pr.html_url) { NSWorkspace.shared.open(u) } }
                    Button("Copy Link") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(pull.pr.html_url, forType: .string) }
                    if pull.pr.state == "open" {
                        Divider()
                        Menu("Merge") {
                            ForEach(ForgejoClient.MergeStyle.allCases, id: \.self) { s in
                                Button(s.rawValue.capitalized) { confirmMerge(s) }
                            }
                        }
                        .disabled(pull.pr.mergeable == false)
                    }
                } label: { Image(systemName: "ellipsis.circle") }
                .menuStyle(.borderlessButton).fixedSize()
                Button { showSubmit = true } label: {
                    Text(pull.drafts.isEmpty ? "Review" : "Review (\(pull.drafts.count))")
                }
                .buttonStyle(.borderedProminent)
                .fixedSize()
                .popover(isPresented: $showSubmit, arrowEdge: .bottom) { SubmitReviewView(pull: pull) { showSubmit = false } }
            }
            .controlSize(.small)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(.bar)
    }

    @ViewBuilder private var reviewBadges: some View {
        ForEach(pull.reviewerStates, id: \.login) { r in
            switch r.state {
            case .approved: Label(r.login, systemImage: "checkmark.circle.fill").foregroundStyle(.green).help("\(r.login) approved")
            case .changesRequested: Label(r.login, systemImage: "exclamationmark.circle.fill").foregroundStyle(.red).help("\(r.login) requested changes")
            case .pending: Label(r.login, systemImage: "circle.fill").foregroundStyle(.orange).help("Waiting for \(r.login)'s review")
            }
        }
        .font(.caption)
    }

    private func confirmMerge(_ s: ForgejoClient.MergeStyle) {
        let a = NSAlert()
        a.messageText = "\(s.rawValue.capitalized) #\(pull.pr.number) into \(pull.pr.base.ref)?"
        a.addButton(withTitle: s.rawValue.capitalized)
        a.addButton(withTitle: "Cancel")
        if a.runModal() == .alertFirstButtonReturn { Task { await pull.merge(s) } }
    }

    private var files: some View {
        ResizableSplit(key: "prTreeWidth", minWidth: 250, maxWidth: 460) {
            VStack(spacing: 0) {
                HStack {
                    TextField("Filter files", text: $pull.filter).textFieldStyle(.roundedBorder)
                }
                .padding(8)
                let total = max(1, pull.files.count)
                VStack(alignment: .leading, spacing: 3) {
                    ProgressView(value: Double(pull.viewedCount), total: Double(total))
                    Text("\(pull.viewedCount) of \(pull.files.count) files viewed").font(.caption).foregroundStyle(.secondary)
                }
                .padding(.horizontal, 10).padding(.bottom, 6)
                FileTreeList(paths: pull.filteredFiles.map(\.path), selection: $pull.selectedPath,
                             collapsed: pull.filter.isEmpty ? $pull.collapsed : .constant([]),
                             dirMenu: { dir in
                                 let fs = pull.files(under: dir)
                                 return (fs.allSatisfy(pull.isViewed) && !fs.isEmpty ? "Mark Folder as Not Viewed" : "Mark Folder as Viewed", { pull.toggleViewed(dir: dir) })
                             }) { path in
                    if let f = pull.file(path) {
                        HStack(spacing: 6) {
                            TreeFileRow(path: f.path, badge: f.badge, additions: f.additions, deletions: f.deletions, isBinary: f.isBinary, dimmed: pull.isViewed(f))
                            let c = pull.commentCount(for: f.path)
                            if c > 0 { Label(String(c), systemImage: "bubble.left.fill").font(.caption2).foregroundStyle(.secondary).labelStyle(.titleAndIcon).fixedSize() }
                        }
                    }
                } dirAccessory: { dir in
                    let fs = pull.files(under: dir)
                    let done = fs.filter(pull.isViewed).count
                    // Viewed is toggled from the file header; the tree only reports progress.
                    Text(verbatim: "\(done)/\(fs.count)").font(.caption2.monospacedDigit())
                        .foregroundStyle(done == fs.count && !fs.isEmpty ? Color.accentColor : Color.secondary.opacity(0.6))
                }
            }
            .frame(maxHeight: .infinity)
        } trailing: {
            Group {
                if let f = pull.selectedFile {
                    VStack(spacing: 0) {
                        DiffHeader(path: f.path, oldPath: f.oldPath != f.newPath ? f.oldPath : nil, additions: f.additions, deletions: f.deletions) {
                            Toggle("Viewed", isOn: Binding(get: { pull.isViewed(f) }, set: { _ in pull.toggleViewed(f) })).toggleStyle(.checkbox)
                        }
                        Divider()
                        if f.isBinary { ContentUnavailableView("Binary File", systemImage: "doc.zipper") }
                        else if f.lines.isEmpty { ContentUnavailableView("No Content Changes", systemImage: "doc", description: Text("Mode change or rename only.")) }
                        else {
                            DiffTableView(file: f, layout: layout, comments: pull.inlineComments(for: f.path),
                                          onAddComment: { n, o, body in pull.drafts.append(DraftComment(path: f.path, newLine: n, oldLine: o, body: body)) },
                                          onDeleteDraft: { id in pull.drafts.removeAll { $0.id == id } })
                        }
                    }
                } else if pull.loading {
                    ProgressView("Loading diff…").frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    ContentUnavailableView("No File Selected", systemImage: "doc.text.magnifyingglass")
                }
            }
            .frame(minWidth: 360, maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

extension FileDiff {
    var badge: FileChange.Kind {
        if oldPath == nil { return .added }
        if newPath == nil { return .deleted }
        if oldPath != newPath { return .renamed }
        return .modified
    }
}

struct SubmitReviewView: View {
    var pull: PullRequestModel
    var done: () -> Void
    @State private var body_ = ""
    @State private var event = ForgejoClient.ReviewEvent.comment
    @State private var sending = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Finish your review").font(.headline)
            TextEditor(text: $body_)
                .frame(width: 400, height: 120)
                .scrollContentBackground(.hidden)
                .padding(4)
                .background(.background.secondary, in: RoundedRectangle(cornerRadius: 6))
            Picker("", selection: $event) {
                Text("Comment").tag(ForgejoClient.ReviewEvent.comment)
                Text("Approve").tag(ForgejoClient.ReviewEvent.approve)
                Text("Request Changes").tag(ForgejoClient.ReviewEvent.requestChanges)
            }
            .pickerStyle(.radioGroup)
            .labelsHidden()
            HStack {
                Text(pull.drafts.isEmpty ? "No pending comments" : "\(pull.drafts.count) pending comment(s)").font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("Cancel", action: done).keyboardShortcut(.cancelAction)
                Button("Submit Review") {
                    sending = true
                    Task { if await pull.submit(event: event, body: body_) { done() }; sending = false }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.return, modifiers: .command)
                .disabled(sending || (event != .approve && body_.isEmpty && pull.drafts.isEmpty))
            }
        }
        .padding(14)
    }
}

struct ConversationView: View {
    var pull: PullRequestModel
    @State private var reply = ""

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    if let b = pull.pr.body, !b.isEmpty { bubble(pull.pr.user.login, b, date: nil, tint: .accentColor) }
                    ForEach(timeline, id: \.id) { item in bubble(item.author, item.body, date: item.date, tint: item.tint, label: item.label) }
                }
                .padding(16)
            }
            Divider()
            HStack(alignment: .bottom) {
                TextField("Leave a comment", text: $reply, axis: .vertical).lineLimit(1...6).textFieldStyle(.roundedBorder)
                Button("Comment") { let r = reply; reply = ""; Task { await pull.postComment(r) } }
                    .disabled(reply.trimmingCharacters(in: .whitespaces).isEmpty)
                    .keyboardShortcut(.return, modifiers: .command)
            }
            .padding(10)
        }
    }

    private struct Item { var id: String; var author: String; var body: String; var date: String; var tint: Color; var label: String? }

    private var timeline: [Item] {
        let c = pull.conversation.map { Item(id: "c\($0.id)", author: $0.user?.login ?? "?", body: $0.body, date: $0.created_at, tint: .secondary, label: nil) }
        let r = pull.reviews.filter { !$0.body.isEmpty || $0.state != "COMMENT" }.map {
            Item(id: "r\($0.id)", author: $0.user?.login ?? "?", body: $0.body, date: $0.submitted_at ?? "",
                 tint: $0.state == "APPROVED" ? .green : $0.state == "REQUEST_CHANGES" ? .orange : .secondary,
                 label: $0.state == "APPROVED" ? "approved" : $0.state == "REQUEST_CHANGES" ? "requested changes" : "reviewed")
        }
        return (c + r).sorted { $0.date < $1.date }
    }

    private func bubble(_ author: String, _ text: String, date: String?, tint: Color, label: String? = nil) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                Text(author).fontWeight(.semibold)
                if let label { Text(label).foregroundStyle(tint) }
                Spacer()
                if let date { Text(date.prefix(16).replacingOccurrences(of: "T", with: " ")).font(.caption).foregroundStyle(.tertiary) }
            }
            if !text.isEmpty {
                Text((try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(text))
                    .textSelection(.enabled)
            }
        }
        .padding(12)
        .frame(maxWidth: 760, alignment: .leading)
        .background(tint.opacity(0.06), in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(tint.opacity(0.25)))
    }
}

// MARK: - Settings

struct SettingsView: View {
    @Environment(AppModel.self) private var app
    @State private var github = false
    @State private var url = ""
    @State private var token = ""
    @State private var status: String?
    @State private var checking = false

    var body: some View {
        Form {
            Section("Accounts") {
                if app.accounts.isEmpty { Text("No accounts yet.").foregroundStyle(.secondary) }
                ForEach(app.accounts) { a in
                    HStack {
                        Image(systemName: a.isGitHub ? "cat" : "server.rack")
                        VStack(alignment: .leading) {
                            Text(a.username).fontWeight(.medium)
                            Text(a.baseURL).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Remove", role: .destructive) { app.setToken(nil, for: a); app.accounts.removeAll { $0.id == a.id } }
                    }
                }
            }
            Section("Add Account") {
                Picker("Server", selection: $github) {
                    Text("Forgejo / Gitea").tag(false)
                    Text("GitHub").tag(true)
                }
                .pickerStyle(.segmented)
                .onChange(of: github) { url = github ? "https://github.com" : ""; token = ""; status = nil }
                TextField("Server URL", text: $url, prompt: Text(github ? "https://github.com" : "https://git.example.com"))
                SecureField("Access Token", text: $token,
                            prompt: Text(github ? "Settings → Developer settings → Personal access tokens" : "Settings → Applications → Generate token"))
                HStack {
                    if let status { Text(status).font(.caption).foregroundStyle(.secondary) }
                    Spacer()
                    if github {
                        Button("Use GitHub CLI Token") { useGHToken() }
                            .help("Reads the token `gh auth login` stored for this host")
                    }
                    Button("Add") { add() }.disabled(url.isEmpty || token.isEmpty || checking)
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 480, height: 420)
    }

    private func add() {
        var base = url.trimmingCharacters(in: .whitespacesAndNewlines)
        if !base.contains("://") { base = "https://" + base }
        while base.hasSuffix("/") { base.removeLast() }
        checking = true
        status = "Checking…"
        Task {
            defer { checking = false }
            do {
                let me = try await ForgejoClient(baseURL: base, token: token, github: github).me()
                let acct = ForgejoAccount(baseURL: base, username: me.login, github: github ? true : nil)
                app.setToken(token, for: acct)
                app.accounts.removeAll { $0.id == acct.id }
                app.accounts.append(acct)
                url = github ? "https://github.com" : ""; token = ""; status = "Signed in as \(me.login)."
            } catch { status = error.localizedDescription }
        }
    }

    private func useGHToken() {
        let host = URL(string: url.contains("://") ? url : "https://" + url)?.host ?? "github.com"
        Task {
            if let t = await Self.ghToken(host: host) { token = t; status = "Token read from gh." }
            else { status = "No gh login for \(host). Run `gh auth login` first." }
        }
    }

    /// `gh auth token`; a GUI app's PATH has no Homebrew, so look in the usual places.
    static func ghToken(host: String) async -> String? {
        await Task.detached {
            guard let gh = ["/opt/homebrew/bin/gh", "/usr/local/bin/gh", "/usr/bin/gh"].first(where: FileManager.default.isExecutableFile) else { return nil }
            let p = Process(), out = Pipe()
            p.executableURL = URL(fileURLWithPath: gh)
            p.arguments = ["auth", "token", "--hostname", host]
            p.standardOutput = out
            p.standardError = FileHandle.nullDevice
            guard (try? p.run()) != nil else { return nil }
            let data = out.fileHandleForReading.readDataToEndOfFile()
            p.waitUntilExit()
            let t = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
            return p.terminationStatus == 0 && !t.isEmpty ? t : nil
        }.value
    }
}

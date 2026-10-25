import SwiftUI
import ImageIO

struct PullRequestsView: View {
    @Environment(AppModel.self) private var app
    var model: WorktreeModel
    var layout: DiffLayout

    var body: some View {
        if let remote = model.remote, let client = model.client {
            PullBrowser(model: model, list: model.pulls, client: client, remote: remote, me: model.account?.username, layout: layout)
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
                PullDetailView(pull: list.detail(for: pr, client: client, repo: remote, local: LocalCheckout(dir: model.path, remote: model.remoteName, gitAuth: model.gitAuth)), worktree: model, layout: layout) { list.selected = nil }
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
        // Decoded off the main thread at a size no avatar is shown above (32 pt at 2×, with headroom): a 460 px
        // original would otherwise sit decoded in memory at ~850 KB per person.
        let t = Task<NSImage?, Never>.detached {
            guard let (data, _) = try? await URLSession.shared.data(from: url),
                  let src = CGImageSourceCreateWithData(data as CFData, nil),
                  let cg = CGImageSourceCreateThumbnailAtIndex(src, 0, [kCGImageSourceCreateThumbnailFromImageAlways: true,
                                                                        kCGImageSourceCreateThumbnailWithTransform: true,
                                                                        kCGImageSourceThumbnailMaxPixelSize: 128] as CFDictionary)
            else { return nil }
            return NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
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
    @State private var loaded: (url: URL, image: NSImage)?   // keyed by URL: a reused view (a detail header) changes users

    private var url: URL? { user.avatar_url.flatMap(URL.init(string:)) }

    var body: some View {
        Group {
            if let img = loaded.flatMap({ $0.url == url ? $0.image : nil }) ?? url.flatMap(AvatarCache.shared.cached) {
                Image(nsImage: img).resizable().scaledToFill()
            }
            else { initials }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .overlay(Circle().strokeBorder(.separator, lineWidth: 0.5))
        .help(user.login)
        .task(id: url) {
            guard let url, loaded?.url != url, AvatarCache.shared.cached(url) == nil else { return }
            if let img = await AvatarCache.shared.load(url) { loaded = (url, img) }
        }
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

    var body: some View {
        VStack(spacing: 0) {
            PullHeader(pull: pull, worktree: worktree, tab: $tab, back: back)
            Divider()
            switch tab {
            case "overview": PROverview(pull: pull) { path, line in tab = "files"; pull.reveal(path, line: line) }
            case "conversation": ConversationView(pull: pull) { path, line in tab = "files"; pull.reveal(path, line: line) }
            default: PullFilesPane(pull: pull, layout: layout)
            }
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
        .background(FilesChrome(pull: pull, tab: tab))
        .alert("Forgejo", isPresented: Binding(get: { pull.error != nil }, set: { if !$0 { pull.error = nil } })) {
            Button("OK") { pull.error = nil }
        } message: { Text(pull.error ?? "") }
    }
}

/// The pull request's header: title, links and actions, then state and the tabs.
private struct PullHeader: View {
    @Environment(AppModel.self) private var app
    var pull: PullRequestModel
    var worktree: WorktreeModel
    @Binding var tab: String
    var back: () -> Void
    @State private var showSubmit = false

    var body: some View { header }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            // Who and what, then the actions on the pull request.
            HStack(alignment: .firstTextBaseline) {
                Button(action: back) { Label("Pull Requests", systemImage: "chevron.left") }
                    .buttonStyle(.borderless).shortcut(.back)
                Text(pull.pr.title).font(.title3.weight(.semibold)).lineLimit(1).textSelection(.enabled).help(pull.pr.title)
                // The pull request's own links live on its number.
                Menu {
                    Button { if let u = URL(string: pull.pr.html_url) { NSWorkspace.shared.open(u) } } label: { Label("Open in Browser", systemImage: "safari") }
                    Button { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(pull.pr.html_url, forType: .string) } label: { Label("Copy Link", systemImage: "link") }
                } label: {
                    HStack(spacing: 3) {
                        Text(verbatim: "#\(pull.pr.number)").font(.title3)
                        Image(systemName: "chevron.down").font(.caption2.weight(.semibold))
                    }
                    .foregroundStyle(.secondary)
                    .contentShape(Rectangle())
                }
                .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
                .help("Open in Browser or Copy Link")
                Spacer()
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small).opacity(pull.loading ? 1 : 0)
                    Button { Task { await worktree.checkoutInWorktree(pull.pr) } } label: { Label("Check Out", systemImage: "arrow.triangle.branch") }
                        .help("Check Out in New Worktree")
                    if pull.pr.state == "open" {
                        Menu {
                            ForEach(ForgejoClient.MergeStyle.allCases, id: \.self) { s in
                                Button(s.rawValue.capitalized) { confirmMerge(s) }
                            }
                        } label: { Label("Merge", systemImage: "arrow.triangle.merge") }
                        .fixedSize()
                        .disabled(pull.pr.mergeable == false)
                        .help(pull.pr.mergeable == false ? "Can't be merged: conflicts with \(pull.pr.base.ref), or a branch protection rule isn't met" : "Merge into \(pull.pr.base.ref)")
                    }
                    Button { showSubmit = true } label: {
                        Text(pull.drafts.isEmpty ? "Review" : "Review (\(pull.drafts.count))")
                    }
                    .buttonStyle(.borderedProminent)
                    .popover(isPresented: $showSubmit, arrowEdge: .bottom) { SubmitReviewView(pull: pull) { showSubmit = false } }
                }
                .controlSize(.small)
                .fixedSize()
            }
            // Its state, then where to look.
            HStack(spacing: 10) {
                Avatar(user: pull.pr.user, size: 18)
                Text(pull.pr.user.login).fontWeight(.medium).lineLimit(1).fixedSize()
                Text("\(pull.pr.head.ref) → \(pull.pr.base.ref)").font(.callout.monospaced()).foregroundStyle(.secondary)
                    .lineLimit(1).truncationMode(.middle).layoutPriority(1).help("\(pull.pr.head.ref) → \(pull.pr.base.ref)")
                if let a = pull.pr.additions, let d = pull.pr.deletions {
                    Text(verbatim: "+\(String(a))").foregroundStyle(.green).monospacedDigit().fixedSize()
                    Text(verbatim: "−\(String(d))").foregroundStyle(.red).monospacedDigit().fixedSize()
                }
                reviewBadges
                ChecksBadge(model: pull.checks) {
                    withAnimation(.easeOut(duration: 0.15)) { app.checksOverlay = (pull.checks, pull.client.baseURL) }
                }
                .fixedSize()
                Spacer()
                Picker("", selection: $tab) {
                    Text("Overview").tag("overview")
                    Text("Files \(pull.files.count)").tag("files")
                    Text("Conversation").tag("conversation")
                }
                .pickerStyle(.segmented).labelsHidden().fixedSize()
            }
            .controlSize(.small)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(.bar)
    }

    /// One mark per state, with the count; the names are in the tooltip.
    @ViewBuilder private var reviewBadges: some View {
        let states = pull.reviewerStates
        ForEach([(PullRequestModel.ReviewerState.approved, "approved", "checkmark.circle.fill", Color.green),
                 (.changesRequested, "requested changes", "exclamationmark.circle.fill", .red),
                 (.pending, "pending", "circle.fill", .orange)], id: \.1) { state, label, icon, color in
            let names = states.filter { $0.state == state }.map(\.login)
            if !names.isEmpty {
                Label { Text("\(names.count) \(label)").foregroundStyle(.secondary) } icon: { Image(systemName: icon).foregroundStyle(color) }
                    .fixedSize()
                    .help(names.joined(separator: ", "))
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
}

/// The Files tab: the tree and the open file. Each part is its own view, so a selection or a poll re-renders only
/// the part that reads it.
private struct PullFilesPane: View {
    @Bindable var pull: PullRequestModel
    var layout: DiffLayout

    var body: some View {
        ResizableSplit(key: "prTreeWidth", minWidth: 250, maxWidth: 460) {
            VStack(spacing: 0) {
                TextField("Filter files", text: $pull.filter).textFieldStyle(.roundedBorder)
                    .padding(8)
                let total = max(1, pull.files.count)
                VStack(alignment: .leading, spacing: 3) {
                    ProgressView(value: Double(pull.viewedCount), total: Double(total))
                    Text("\(pull.viewedCount) of \(pull.files.count) files viewed").font(.caption).foregroundStyle(.secondary)
                }
                .padding(.horizontal, 10).padding(.bottom, 6)
                if pull.fullMode && pull.tree.isEmpty {
                    Group {
                        if let e = pull.treeError { ContentUnavailableView("Can't Load the Repository", systemImage: "exclamationmark.triangle", description: Text(e)) }
                        else { ProgressView("Fetching the pull request…").controlSize(.small) }
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    PullTree(pull: pull)
                }
            }
            .frame(maxHeight: .infinity)
        } trailing: {
            PullFilePane(pull: pull, layout: layout)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .task(id: pull.fullMode) { if pull.fullMode { await pull.loadTree() } }
        .onReceive(NotificationCenter.default.publisher(for: UserDefaults.didChangeNotification)) { _ in
            let on = UserDefaults.standard.bool(forKey: "prFullMode")   // the Review menu's Compact / Full Mode
            if on != pull.fullMode { pull.fullMode = on }
        }
        .onChange(of: pull.pr.head.sha) { if pull.fullMode { Task { await pull.loadTree() } } }
    }
}

private struct PullTree: View {
    @Bindable var pull: PullRequestModel

    var body: some View {
        let paths = pull.fullMode
            ? pull.filteredTree
            : pull.filteredFiles.map(\.path)
        let collapsed = pull.fullMode ? $pull.fullCollapsed : $pull.collapsed
        return FileTreeList(paths: paths, selection: $pull.selectedPath,
                            collapsed: pull.filter.isEmpty ? collapsed : .constant([]),
                            dirMenu: { dir in
                                let p = pull.viewedProgress(under: dir)
                                return (p.total > 0 && p.done == p.total ? "Mark Folder as Not Viewed" : "Mark Folder as Viewed", { pull.toggleViewed(dir: dir) })
                            }) { path in
            if let f = pull.file(path) {
                HStack(spacing: 6) {
                    TreeFileRow(path: f.path, badge: f.badge, additions: f.additions, deletions: f.deletions, isBinary: f.isBinary, dimmed: pull.isViewed(f))
                    let c = pull.commentCount(for: f.path)
                    if c > 0 { Label(String(c), systemImage: "bubble.left.fill").font(.caption2).foregroundStyle(.secondary).labelStyle(.titleAndIcon).fixedSize() }
                }
            } else {
                // Full mode: a file this PR doesn't touch.
                HStack(spacing: 6) {
                    FileIcon(path: path)
                    Text((path as NSString).lastPathComponent).lineLimit(1).truncationMode(.middle).foregroundStyle(.secondary)
                    Spacer(minLength: 0)
                }
                .help(path)
            }
        } dirAccessory: { dir in
            let p = pull.viewedProgress(under: dir)
            if p.total > 0 {
                // Viewed is toggled from the file header; the tree only reports progress.
                Text(verbatim: "\(p.done)/\(p.total)").font(.caption2.monospacedDigit())
                    .foregroundStyle(p.done == p.total ? Color.accentColor : Color.secondary.opacity(0.6))
            }
        }
    }
}

private struct PullFilePane: View {
    var pull: PullRequestModel
    var layout: DiffLayout

    var body: some View {
            VStack(spacing: 0) {
                if pull.fullMode && !pull.tabs.isEmpty {
                    FileTabBar(pull: pull)
                    Divider()
                }
                Group {
                    if pull.fullMode, let path = pull.selectedPath { fullFile(path) }
                    else if let f = pull.selectedFile { compactFile(f) }
                    else if pull.loading { ProgressView("Loading diff…").frame(maxWidth: .infinity, maxHeight: .infinity) }
                    else { ContentUnavailableView("No File Selected", systemImage: "doc.text.magnifyingglass") }
                }
                .frame(maxHeight: .infinity)
                if let r = pull.navResults {
                    Divider()
                    NavResultsPanel(results: r, pull: pull).frame(height: 220)
                }
            }
            .frame(minWidth: 360, maxWidth: .infinity, maxHeight: .infinity)
    }

    @ViewBuilder private func compactFile(_ f: FileDiff) -> some View {
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
                              onDeleteDraft: { id in pull.drafts.removeAll { $0.id == id } },
                              reveal: pull.reveal)
            }
        }
    }

    @ViewBuilder private func fullFile(_ path: String) -> some View {
        let changed = pull.file(path)
        VStack(spacing: 0) {
            DiffHeader(path: path, oldPath: changed.flatMap { $0.oldPath != $0.newPath ? $0.oldPath : nil },
                       additions: changed?.additions ?? 0, deletions: changed?.deletions ?? 0, showCounts: changed != nil) {
                FileNavControls(pull: pull, path: path)
                if let f = changed {
                    Toggle("Viewed", isOn: Binding(get: { pull.isViewed(f) }, set: { _ in pull.toggleViewed(f) })).toggleStyle(.checkbox)
                } else {
                    Text("Unchanged").font(.caption).foregroundStyle(.secondary)
                }
            }
            Divider()
            if let f = pull.fullFiles[path] {
                if f.isBinary { ContentUnavailableView("Binary File", systemImage: "doc.zipper") }
                else if f.lines.isEmpty { ContentUnavailableView("Empty File", systemImage: "doc") }
                else {
                    DiffTableView(file: f, layout: changed == nil ? .unified : layout, comments: changed == nil ? [] : pull.inlineComments(for: path),
                                  onAddComment: changed == nil ? nil : { n, o, body in pull.drafts.append(DraftComment(path: path, newLine: n, oldLine: o, body: body)) },
                                  onDeleteDraft: { id in pull.drafts.removeAll { $0.id == id } },
                                  canComment: pull.commentable(path),
                                  onNavigate: { word, action in Task { await pull.navigate(word, action, from: path) } },
                                  reveal: pull.reveal, changeRuler: true)
                }
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .task(id: path + pull.pr.head.sha) { await pull.loadFull(path) }
    }
}

/// What the Files tab tells the window: which toolbar controls apply, and that ⌘W closes a tab. Separate so the
/// selection it reads doesn't re-render the whole detail view.
private struct FilesChrome: View {
    var pull: PullRequestModel
    var tab: String
    var body: some View {
        Color.clear
            .preference(key: DiffControlsKey.self, value: tab == "files"
                        ? DiffControls(layout: pull.fullMode ? pull.selectedPath != nil : pull.selectedFile != nil, fullMode: pull) : DiffControls())
            // ⌘W closes the open tab rather than the window; with no tab open it does nothing, so it never quits the app.
            .background(CommandWInterceptor(action: tab == "files" && pull.fullMode ? { if let p = pull.selectedPath { pull.closeTab(p) } } : nil))
    }
}

/// The PR's structural summary: from the merge base to the head, like the diff itself.
struct PROverview: View {
    var pull: PullRequestModel
    var onOpen: (String, Int?) -> Void
    @State private var base: String?
    @State private var error: String?

    var body: some View {
        Group {
            if let base {
                ChangeOverview(dir: pull.local.dir, a: .rev(base), b: .rev(pull.pr.head.sha),
                               changes: pull.files.map { ChangedPath(old: $0.oldPath, new: $0.newPath) },
                               key: "\(base) \(pull.pr.head.sha) \(pull.files.count)", onOpen: onOpen)
            } else if let error {
                ContentUnavailableView("Can't Read the Pull Request", systemImage: "exclamationmark.triangle", description: Text(error))
            } else {
                ProgressView("Fetching the pull request…").frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .task(id: pull.pr.head.sha) {
            do {
                try await pull.fetchHead()
                base = try await Git.string(["merge-base", pull.pr.base.sha, pull.pr.head.sha], in: pull.local.dir)
                    .trimmingCharacters(in: .whitespacesAndNewlines)
            } catch { self.error = error.localizedDescription }
        }
    }
}

/// Full mode's open files. Changed files carry their status colour; the selected tab is the one shown.
struct FileTabBar: View {
    var pull: PullRequestModel
    @State private var hovered: String?

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 0) {
                    ForEach(pull.tabs, id: \.self) { path in tab(path).id(path) }
                }
            }
            .onChange(of: pull.selectedPath) { _, p in if let p { withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo(p) } } }
        }
        .frame(height: 30)
        .background(.bar)
    }

    private func tab(_ path: String) -> some View {
        let active = pull.selectedPath == path
        let name = (path as NSString).lastPathComponent
        // Same file name twice (mod.rs, index.ts): add the parent directory.
        let twin = pull.tabs.contains { $0 != path && ($0 as NSString).lastPathComponent == name }
        let parent = ((path as NSString).deletingLastPathComponent as NSString).lastPathComponent
        let changed = pull.file(path)
        return HStack(spacing: 5) {
            FileIcon(path: path)
            Text(name).lineLimit(1).foregroundStyle(active ? .primary : .secondary)
            if twin, !parent.isEmpty { Text(parent).font(.caption).foregroundStyle(.tertiary).lineLimit(1) }
            if let c = changed { Circle().fill(c.badge.color).frame(width: 6, height: 6).help("Changed in this pull request") }
            Button { pull.closeTab(path) } label: { Image(systemName: "xmark").font(.system(size: 9, weight: .semibold)) }
                .buttonStyle(.borderless)
                .opacity(active || hovered == path ? 1 : 0)
                .help("Close Tab")
        }
        .font(.callout)
        .padding(.horizontal, 10)
        .frame(height: 30)
        .background(active ? Color(nsColor: .textBackgroundColor) : Color.clear)
        .overlay(alignment: .trailing) { Rectangle().fill(Color(nsColor: .separatorColor)).frame(width: 1) }
        .overlay(alignment: .top) { if active { Rectangle().fill(Color.accentColor).frame(height: 2) } }
        .contentShape(Rectangle())
        .onTapGesture { pull.selectedPath = path }
        .onHover { hovered = $0 ? path : (hovered == path ? nil : hovered) }
        .help(path)
        .contextMenu {
            Button("Close Tab") { pull.closeTab(path) }
            Button("Close Other Tabs") { pull.closeOtherTabs(path) }.disabled(pull.tabs.count < 2)
            Button("Close All Tabs") { pull.tabs = []; pull.selectedPath = nil }
            Divider()
            Button("Copy Path") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(path, forType: .string) }
        }
    }
}

/// Back / forward through jumps, the file's outline, and what's being looked up.
struct FileNavControls: View {
    var pull: PullRequestModel
    var path: String
    @State private var outline: [CodeSymbol] = []

    var body: some View {
        HStack(spacing: 2) {
            if let w = pull.navBusy {
                ProgressView().controlSize(.mini)
                Text(w).font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(1)
            }
            Button { pull.goBack() } label: { Image(systemName: "chevron.left") }
                .disabled(pull.back.isEmpty).help("Back").shortcut(.navigateBack)
            Button { pull.goForward() } label: { Image(systemName: "chevron.right") }
                .disabled(pull.forward.isEmpty).help("Forward").shortcut(.navigateForward)
            if !outline.isEmpty {
                Menu {
                    ForEach(outline, id: \.self) { s in
                        Button { pull.open(path, line: s.line) } label: { Text("\(s.name)  \(s.kind.rawValue) · \(s.line)") }
                    }
                } label: { Image(systemName: "list.bullet.indent") }
                .menuStyle(.borderlessButton).fixedSize()
                .help("Outline: jump to a definition in this file")
            }
        }
        .buttonStyle(.borderless)
        .task(id: [path, pull.fullFiles[path] == nil ? "0" : "1"]) {
            guard let text = pull.headText(path) else { outline = []; return }
            let p = path
            outline = await Task.detached(priority: .utility) { CodeIndex.outline(text, path: p) }.value
        }
    }
}

/// Under the file: the definitions to choose from, or every reference.
struct NavResultsPanel: View {
    var results: PullRequestModel.NavResults
    var pull: PullRequestModel

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                switch results {
                case .definitions(let w, let d): Text("\(d.count) definitions of ").foregroundStyle(.secondary) + Text(w).font(.body.monospaced())
                case .references(let w, let r, let noDef):
                    let n = r.count >= 2000 ? "2000+ references" : r.count == 1 ? "1 reference" : "\(r.count) references"
                    Text(noDef ? "No definition found in this repository · " : "").foregroundStyle(.secondary)
                        + Text("\(n) to ").foregroundStyle(.secondary) + Text(w).font(.body.monospaced())
                }
                Spacer()
                Button { pull.navResults = nil } label: { Image(systemName: "xmark") }.buttonStyle(.borderless).keyboardShortcut(.cancelAction)
            }
            .font(.callout)
            .padding(.horizontal, 12).padding(.vertical, 6)
            .background(.bar)
            Divider()
            List {
                switch results {
                case .definitions(_, let defs):
                    ForEach(defs, id: \.self) { d in
                        row(path: d.path, line: d.line, text: "\(d.kind.rawValue) \(d.name)")
                    }
                case .references(_, let refs, _):
                    ForEach(refs, id: \.self) { r in row(path: r.path, line: r.line, text: r.text) }
                }
            }
            .listStyle(.plain)
        }
    }

    private func row(path: String, line: Int, text: String) -> some View {
        Button { pull.open(path, line: line) } label: {
            HStack(spacing: 8) {
                FileIcon(path: path)
                Text("\(path):\(line)").font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
                    .frame(width: 260, alignment: .leading)
                Text(text).font(.callout.monospaced()).lineLimit(1)
                Spacer(minLength: 0)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
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

/// While `action` is set, ⌘W in this view's window runs it instead of closing the window.
struct CommandWInterceptor: NSViewRepresentable {
    var action: (() -> Void)?
    func makeNSView(context: Context) -> Probe { Probe() }
    func updateNSView(_ v: Probe, context: Context) { v.action = action }

    final class Probe: NSView {
        var action: (() -> Void)?
        private var monitor: Any?
        override func viewDidMoveToWindow() {
            if let monitor { NSEvent.removeMonitor(monitor) }
            monitor = nil
            guard window != nil else { return }
            monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
                guard e.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command,
                      e.charactersIgnoringModifiers?.lowercased() == "w" else { return e }
                let windowNumber = e.windowNumber
                // Local monitors run on the main thread.
                let handled = MainActor.assumeIsolated { () -> Bool in
                    guard let self, let action = self.action, self.window?.windowNumber == windowNumber else { return false }
                    action()
                    return true
                }
                return handled ? nil : e
            }
        }
    }
}

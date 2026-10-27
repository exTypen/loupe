import SwiftUI

struct WorktreeView: View {
    @Environment(AppModel.self) private var app
    var model: WorktreeModel
    @AppStorage("tab") private var tab = "changes"
    @State private var showBranches = false
    @State private var controls = DiffControls()
    @AppStorage("prFullMode") private var fullMode = false

    var body: some View {
        Group {
            switch tab {
            case "history": HistoryView(worktree: model, model: model.history)
            case "pulls": PullRequestsView(model: model)
            default: ChangesView(model: model)
            }
        }
        .navigationTitle(model.headOID.isEmpty ? "" : model.branch ?? "detached HEAD")
        .navigationSubtitle((model.path as NSString).abbreviatingWithTildeInPath)
        .toolbar(removing: .title)   // drawn by the branch button below instead
        .toolbar {
            // Fetch, pull and push act on the branch: beside it, on the left.
            ToolbarItemGroup(placement: .navigation) {
                // No separate spinner (it shifted the buttons); the running operation animates its own icon.
                Button { Task { await model.remoteOp("Fetching", ["fetch", "--all", "--prune"]) } } label: {
                    Label("Fetch", systemImage: "arrow.triangle.2.circlepath").symbolEffect(.rotate, isActive: model.busy == "Fetching")
                }
                .disabled(model.busy != nil)
                    .help("Fetch all remotes")
                Button { Task { await model.remoteOp("Pulling", ["pull", "--ff-only"]) } } label: {
                    Label("Pull", systemImage: "arrow.down").symbolEffect(.pulse, isActive: model.busy == "Pulling")
                }
                .disabled(model.busy != nil)
                    .help(model.behind > 0 ? "Pull \(model.behind) commits (fast-forward only)" : "Pull (fast-forward only)")
                Button { Task { await model.remoteOp("Pushing", ["push", "--set-upstream", model.remoteName, "HEAD"]) } } label: {
                    Label("Push", systemImage: "arrow.up").symbolEffect(.pulse, isActive: model.busy == "Pushing")
                }
                .disabled(model.busy != nil)
                    .help(model.ahead > 0 ? "Push \(model.ahead) commits" : "Push")
            }
            ToolbarItem(placement: .navigation) {
                Button { showBranches.toggle() } label: {
                    VStack(alignment: .leading, spacing: 0) {
                        HStack(spacing: 4) {
                            Text(model.headOID.isEmpty ? "" : model.branch ?? "detached HEAD").font(.headline)
                            Image(systemName: "chevron.down").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                        }
                        Text((model.path as NSString).abbreviatingWithTildeInPath).font(.caption).foregroundStyle(.secondary)
                    }
                    .padding(.horizontal, 12)   // keep the two lines inside the toolbar's glass capsule
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Switch branch")
                .popover(isPresented: $showBranches, arrowEdge: .bottom) { BranchPicker(model: model) }
            }
            ToolbarItem(placement: .principal) {
                Picker("View", selection: $tab) {
                    Label("Changes", systemImage: "square.and.pencil").tag("changes")
                    Label("History", systemImage: "clock").tag("history")
                    Label("Pull Requests", systemImage: "arrow.triangle.pull").tag("pulls")
                }
                .pickerStyle(.segmented)
                .labelStyle(.titleOnly)
            }
            // Compact/Full while a diff shows; the view switch stays put at the right edge, whatever comes and goes.
            ToolbarItemGroup {
                if controls.full {
                    Picker("Mode", selection: $fullMode) {
                        Image(systemName: "rectangle.compress.vertical").help("Compact: the changes with a few lines around them").tag(false)
                        Image(systemName: "rectangle.expand.vertical").help("Full: whole files with the changes inline (⌥⌘F)").tag(true)
                    }
                    .pickerStyle(.segmented)
                }
            }
            ToolbarItemGroup {
                if let v = controls.view {
                    Picker("View", selection: Binding(get: { v.current }, set: v.set)) {
                        ForEach(v.modes, id: \.self) { m in
                            switch m {
                            case .overview: Image(systemName: "square.grid.2x2").help("Overview: what the change does").tag(m)
                            case .files: Image(systemName: "doc.text").help("Files: the diff").tag(m)
                            case .conversation: Image(systemName: "bubble.left.and.bubble.right").help("Conversation").tag(m)
                            }
                        }
                    }
                    .pickerStyle(.segmented)
                }
            }
        }
        .alert("Git", isPresented: Binding(get: { model.error != nil }, set: { if !$0 { model.error = nil } })) {
            Button("OK") { model.error = nil }
        } message: { Text(model.error ?? "") }
        .onReceive(NotificationCenter.default.publisher(for: .refresh)) { _ in Task { await model.refreshStatus() } }
        .onPreferenceChange(DiffControlsKey.self) { controls = $0 }
    }
}

/// Which of the toolbar's diff controls apply to what's on screen. Views showing a diff set it.
struct DiffControls: Equatable {
    var full = false           // a diff is showing: Compact/Full applies (the mode itself is the "prFullMode" default)
    var view: ViewSwitch?
}

/// Overview / Files (/ Conversation) for whatever change is on screen: the modes it has, the one showing, and how to switch.
struct ViewSwitch: Equatable {
    enum Mode: String { case overview, files, conversation }
    var modes: [Mode]
    var current: Mode
    var set: (Mode) -> Void
    static func == (a: Self, b: Self) -> Bool { a.modes == b.modes && a.current == b.current }
}

struct DiffControlsKey: PreferenceKey {
    static var defaultValue: DiffControls { DiffControls() }
    static func reduce(value: inout DiffControls, nextValue: () -> DiffControls) {
        let next = nextValue()
        value.full = value.full || next.full
        value.view = value.view ?? next.view
    }
}

// MARK: - Branch picker

/// Title-bar popover: filter, pick, `git switch`. Remote branches with a local counterpart are hidden.
struct BranchPicker: View {
    var model: WorktreeModel
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @State private var locals: [String] = []
    @State private var remotes: [String] = []
    @State private var checkedOut: [String: String] = [:]   // local branch → path of the worktree that has it
    @State private var error: String?
    @State private var working = false

    private func matching(_ list: [String]) -> [String] { query.isEmpty ? list : list.filter { $0.localizedCaseInsensitiveContains(query) } }

    var body: some View {
        let (l, r) = (matching(locals), matching(remotes))
        VStack(spacing: 0) {
            TextField("Switch to branch…", text: $query).textFieldStyle(.roundedBorder).padding(8)
                .onSubmit { if let b = (l + r).first(where: { $0 != model.branch }) { switchTo(b) } }
            if let error { Text(error).font(.callout).foregroundStyle(.red).textSelection(.enabled).padding(.horizontal, 8).padding(.bottom, 6) }
            Divider()
            List {
                if !l.isEmpty { Section("Local") { ForEach(l, id: \.self) { row($0, remote: false) } } }
                if !r.isEmpty { Section("Remote") { ForEach(r, id: \.self) { row($0, remote: true) } } }
            }
            .listStyle(.plain)
            .disabled(working)
        }
        .frame(width: 340, height: 420)
        .task {
            // lstrip=2, not refname:short: short names refs/remotes/origin/HEAD plain "origin".
            func refs(_ ns: String) async -> [(name: String, worktree: String)] {
                let out = (try? await Git.string(["for-each-ref", "--format=%(refname:lstrip=2)%09%(worktreepath)", "--sort=-committerdate", ns], in: model.path)) ?? ""
                return out.split(separator: "\n").map { l in
                    let f = l.split(separator: "\t", maxSplits: 1, omittingEmptySubsequences: false)
                    return (String(f[0]), f.count > 1 ? String(f[1]) : "")
                }
            }
            let heads = await refs("refs/heads")
            locals = heads.map(\.name)
            checkedOut = Dictionary(heads.filter { !$0.worktree.isEmpty }.map { ($0.name, $0.worktree) }, uniquingKeysWith: { a, _ in a })
            remotes = await refs("refs/remotes").map(\.name).filter { b in
                !b.hasSuffix("/HEAD") && !locals.contains(String(b.split(separator: "/", maxSplits: 1).last ?? ""))
            }
        }
    }

    private func row(_ b: String, remote: Bool) -> some View {
        Button { switchTo(b) } label: {
            HStack(spacing: 6) {
                Image(systemName: remote ? "cloud" : "arrow.triangle.branch").foregroundStyle(.secondary).frame(width: 16)
                Text(b).lineLimit(1).truncationMode(.middle)
                Spacer()
                if b == model.branch { Image(systemName: "checkmark").foregroundStyle(.tint) }
                else if let wt = checkedOut[b] {
                    Text((wt as NSString).lastPathComponent).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        .help("Checked out in \(wt): selecting it opens that worktree")
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(b == model.branch)
    }

    private func switchTo(_ b: String) {
        working = true
        Task {
            defer { working = false }
            let app = model.app, repo = app.repo(containing: model.path) ?? model.path
            // git allows a branch in one worktree at a time: go to the worktree that has it instead.
            if let wt = checkedOut[b] {
                if app.worktrees[repo]?.contains(where: { $0.path == wt }) != true { await app.refreshWorktrees(repo: repo) }
                app.selection = wt
                dismiss()
                return
            }
            do {
                // "origin/x" with no local x (those are hidden) → new local x tracking it.
                let args = remotes.contains(b) ? ["switch", "-c", String(b.split(separator: "/", maxSplits: 1)[1]), "--track", b] : ["switch", b]
                // Conflicting uncommitted changes, or a branch checked out in another worktree: git refuses and says why.
                try await Git.run(args, in: model.path)
                await app.refreshWorktrees(repo: repo)
                dismiss()
            } catch { self.error = error.localizedDescription }
        }
    }
}

// MARK: - Changes

struct ChangesView: View {
    var model: WorktreeModel
    @State private var confirmDiscard: [FileChange]?
    @State private var overview = false
    @AppStorage("prFullMode") private var fullMode = false
    @AppStorage("diffSide") private var side = DiffSide.diff

    var body: some View {
        VStack(spacing: 0) {
        if model.operation != nil || !model.conflicted.isEmpty {
            OperationBanner(model: model)
            Divider()
        }
        ResizableSplit(key: "changesListWidth", minWidth: 220, maxWidth: 380, defaultFraction: 300/380) {
            VStack(spacing: 0) {
                if fullMode { WorktreeTree(model: model, browser: model.browser) }
                else { FileListView(model: model, confirmDiscard: $confirmDiscard) }
                if model.stashCount > 0 { StashesLink(model: model) }
                Divider()
                CommitBox(model: model)
            }
            .frame(maxHeight: .infinity)
        } trailing: {

            Group {
                if overview && !model.files.isEmpty {
                    let changes = Dictionary(grouping: model.files, by: \.path).values.map { fs -> ChangedPath in
                        let f = fs.first { !$0.staged } ?? fs[0]
                        let gone = fs.contains { $0.kind == .deleted }, new = fs.contains { $0.kind == .added || $0.kind == .untracked }
                        return ChangedPath(old: new ? nil : (f.origPath ?? f.path), new: gone ? nil : f.path)
                    }
                    ChangeOverview(dir: model.path, a: model.headOID.isEmpty || model.headOID.hasPrefix("(") ? .empty : .rev("HEAD"), b: .worktree,
                                   changes: changes.sorted { $0.path < $1.path }, key: "\(model.headOID) \(model.statusVersion)") { path, _ in
                        overview = false
                        if let f = model.files.first(where: { $0.path == path && !$0.staged }) ?? model.files.first(where: { $0.path == path }) { model.select(f.id) }
                    }
                }
                else if fullMode { WorktreeFilePane(model: model, browser: model.browser, confirmDiscard: $confirmDiscard) }
                else if let f = model.selectedFile, f.kind == .conflicted { ConflictPane(model: model, path: f.path).id(f.path) }
                else {
                    DiffPane(model: model, confirmDiscard: $confirmDiscard)
                }
            }
            .frame(maxHeight: .infinity)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                if !overview, let r = model.browser.navResults {
                    VStack(spacing: 0) { Divider(); NavResultsPanel(results: r, browser: model.browser).frame(height: 220) }
                }
            }
            .frame(minWidth: 360, maxWidth: .infinity, maxHeight: .infinity)
            .preference(key: DiffControlsKey.self, value: model.files.isEmpty ? DiffControls()
                        : DiffControls(full: !overview && (fullMode || model.selectedFile != nil),
                                       view: ViewSwitch(modes: [.overview, .files], current: overview ? .overview : .files) { overview = $0 == .overview }))
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .onChange(of: model.selectedFileID) {
            overview = false   // picking a file shows its diff
            if !fullMode, let f = model.selectedFile { model.browser.follow(f.path) }
        }
        .onChange(of: fullMode) {
            model.loadDiff()
            // The file showing carries over between the change list and the worktree.
            if fullMode, let f = model.selectedFile { model.browser.selectedPath = f.path }
            if !fullMode, let p = model.browser.selectedPath, let c = model.browser.change(p) { model.select(c.id) }
        }
        // A jump (⌘-click, a result, back / forward) in Compact: a changed file shows in the list, any other needs Full.
        .onChange(of: model.browser.selectedPath) {
            guard !fullMode, let p = model.browser.selectedPath else { return }
            if let c = model.browser.change(p) { if model.selectedFile?.path != p { model.select(c.id) } } else { fullMode = true }
        }
        .onChange(of: side) { model.loadDiff() }
        .confirmationDialog("Discard changes?", isPresented: Binding(get: { confirmDiscard != nil }, set: { if !$0 { confirmDiscard = nil } })) {
            Button("Discard \(confirmDiscard?.count ?? 0) file(s)", role: .destructive) {
                if let fs = confirmDiscard { Task { await model.discard(fs) } }
            }
        } message: { Text("Tracked changes are reverted; untracked files are moved to the Trash.") }
        .onReceive(NotificationCenter.default.publisher(for: .moveFile)) { n in
            let list = model.listed
            guard let d = n.object as? Int, let i = list.firstIndex(where: { $0.id == model.selectedFileID }) else { return }
            model.select(list[max(0, min(list.count - 1, i + d))].id)
        }
    }
}

struct FileListView: View {
    var model: WorktreeModel
    @Binding var confirmDiscard: [FileChange]?
    @AppStorage("filePanel") private var panel = FilePanel.none
    @State private var collapsed: Set<String> = []   // "<section>/<directory>"

    var body: some View {
        // The diff follows the row just added to the selection (plain click, ⌘-click), else stays put.
        let selection = Binding(get: { model.selectedFileIDs }, set: { ids in
            let order = model.listed
            let added = ids.subtracting(model.selectedFileIDs)
            let shown = order.first { added.contains($0.id) }?.id
                ?? (ids.contains(model.selectedFileID ?? "") ? model.selectedFileID : order.first { ids.contains($0.id) }?.id)
            model.select(shown)
            model.selectedFileIDs = ids
        })
        List(selection: selection) {
            if !model.conflicted.isEmpty {
                Section {
                    tree(model.conflicted, section: "conflicts")
                } header: {
                    sectionHeader("Conflicts", count: model.conflicted.count, action: nil) {}
                }
            }
            if !model.staged.isEmpty {
                Section {
                    tree(model.staged, section: "staged")
                } header: {
                    sectionHeader("Staged", count: model.staged.count, action: "Unstage All") { Task { await model.unstage(model.staged) } }
                }
            }
            Section {
                tree(model.unstaged, section: "changes")
            } header: {
                sectionHeader("Changes", count: model.unstaged.count, action: model.unstaged.isEmpty ? nil : "Stage All") { Task { await model.stage(model.unstaged) } }
            }
        }
        .listStyle(.inset)
        .frame(maxHeight: .infinity)
        .overlay {
            if model.files.isEmpty {
                ContentUnavailableView("Working Tree Clean", systemImage: "checkmark.circle", description: Text(model.branch.map { "Nothing to commit on \($0)." } ?? ""))
            }
        }
        .contextMenu(forSelectionType: String.self) { ids in menu(model.files.filter { ids.contains($0.id) }) }
        .onKeyPress(.space) {
            // Mixed selection: stage what isn't staged yet; all staged: unstage.
            let fs = model.selectedFiles
            guard !fs.isEmpty else { return .ignored }
            let unstaged = fs.filter { !$0.staged }
            Task { await unstaged.isEmpty ? model.unstage(fs) : model.stage(unstaged) }
            return .handled
        }
        .onDeleteCommand {
            let fs = model.selectedFiles.filter { !$0.staged }
            if !fs.isEmpty { confirmDiscard = fs }
        }
    }

    private func sectionHeader(_ title: String, count: Int, action: String?, perform: @escaping () -> Void) -> some View {
        HStack {
            Text(title)
            Text("\(count)").foregroundStyle(.secondary).monospacedDigit()
            Spacer()
            if let action { Button(action, action: perform).buttonStyle(.borderless).font(.caption) }
        }
    }

    /// A section's files under their folders. A folder isn't selectable: a click folds or unfolds it.
    @ViewBuilder private func tree(_ files: [FileChange], section: String) -> some View {
        let byPath = Dictionary(files.map { ($0.path, $0) }, uniquingKeysWith: { a, _ in a })
        let closed = Set(collapsed.compactMap { $0.hasPrefix(section + "/") ? String($0.dropFirst(section.count + 1)) : nil })
        ForEach(FileTree.rows(files.map(\.path).sorted(by: FileTree.treeOrder), collapsed: closed)) { r in
            if let dir = r.dirPath {
                let key = section + "/" + dir
                HStack(spacing: 4) {
                    Image(systemName: "chevron.right").font(.caption2.weight(.semibold))
                        .rotationEffect(.degrees(collapsed.contains(key) ? 0 : 90)).foregroundStyle(.secondary).frame(width: 12)
                    Image(systemName: collapsed.contains(key) ? "folder" : "folder.fill").foregroundStyle(.secondary)
                    Text(r.name).lineLimit(1).truncationMode(.head).foregroundStyle(.secondary)
                    Spacer(minLength: 4)
                }
                .treeRow(depth: r.depth)
                .contentShape(Rectangle())
                .onTapGesture { if collapsed.contains(key) { collapsed.remove(key) } else { collapsed.insert(key) } }
                .selectionDisabled()
                .tag(section + ":" + r.id)
            } else if let p = r.filePath, let f = byPath[p] {
                HStack(spacing: 4) {
                    Color.clear.frame(width: 12)
                    FileRow(path: f.path, badge: f.kind, nameOnly: true)
                }
                .treeRow(depth: r.depth)
                .tag(f.id)
            }
        }
    }

    /// Right-click menu for the selection (or the clicked row when it's outside the selection).
    @ViewBuilder private func menu(_ fs: [FileChange]) -> some View {
        let unstaged = fs.filter { !$0.staged }, staged = fs.filter(\.staged)
        let n = { (c: [FileChange]) in c.count == 1 ? "" : " \(c.count) Files" }
        let urls = fs.map { URL(fileURLWithPath: model.path).appendingPathComponent($0.path) }
        if !unstaged.isEmpty { Button("Stage\(n(unstaged))") { Task { await model.stage(unstaged) } } }
        if !staged.isEmpty { Button("Unstage\(n(staged))") { Task { await model.unstage(staged) } } }
        if !unstaged.isEmpty { Button("Discard Changes\(n(unstaged))…", role: .destructive) { confirmDiscard = unstaged } }
        if !fs.isEmpty {
            let paths = Array(Set(fs.map(\.path))).sorted()
            Button("Stash\(paths.count == 1 ? "" : " \(paths.count) Files")") {
                Task { await model.stash(message: "", untracked: fs.contains { $0.kind == .untracked }, paths: paths) }
            }
        }
        if fs.count == 1, let f = fs.first, f.kind != .untracked, f.kind != .added {
            Divider()
            Button("Blame") { model.select(f.id); panel = .blame }
            Button("File History") { model.select(f.id); panel = .history }
        }
        if !fs.isEmpty {
            Divider()
            Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting(urls) }
            Button("Open") { urls.forEach { NSWorkspace.shared.open($0) } }
            Button("Copy Path\(fs.count == 1 ? "" : "s")") {
                NSPasteboard.general.clearContents(); NSPasteboard.general.setString(fs.map(\.path).joined(separator: "\n"), forType: .string)
            }
        }
    }
}

struct FileRow: View {
    var path: String
    var badge: FileChange.Kind?
    var nameOnly = false   // in a tree, where the folders are rows
    var additions: Int? = nil
    var deletions: Int? = nil

    var body: some View {
        HStack(spacing: 7) {
            FileIcon(path: path)
            let dir = nameOnly ? "" : (path as NSString).deletingLastPathComponent
            // GitHub Desktop style: one line, directory dimmed, file name emphasized; the head truncates first.
            (Text(dir.isEmpty ? "" : dir + "/").foregroundStyle(.secondary) + Text((path as NSString).lastPathComponent))
                .lineLimit(1)
                .truncationMode(.head)
            Spacer(minLength: 4)
            if let additions, let deletions {
                Group {
                    Text(verbatim: "+\(String(additions))").foregroundStyle(.green)
                    Text(verbatim: "−\(String(deletions))").foregroundStyle(.red)
                }
                .font(.caption.monospacedDigit()).fixedSize()
            }
            if let badge { StatusIcon(kind: badge) }
        }
        .padding(.vertical, 2)
        .help(path)
    }
}

extension FileChange.Kind {
    var color: Color {
        switch self {
        case .added, .untracked: .green
        case .deleted: .red
        case .renamed, .copied: .purple
        case .conflicted: .orange
        default: .blue
        }
    }
}

struct DiffPane: View {
    var model: WorktreeModel
    @AppStorage("prFullMode") private var fullMode = false
    @Binding var confirmDiscard: [FileChange]?

    var body: some View {
        // Keep showing the previous diff until the next one is parsed: swapping in a spinner for ~50 ms reads as a flash.
        if let f = model.selectedFile, let d = model.diff {
            VStack(spacing: 0) {
                DiffHeader(path: d.path, oldPath: d.oldPath != d.newPath ? d.oldPath : nil, additions: d.additions, deletions: d.deletions,
                           file: f.kind == .untracked || f.kind == .added ? nil : FileTarget(worktree: model.path, path: f.path)) {
                    if f.staged { Button("Unstage File") { Task { await model.unstage([f]) } } }
                    else {
                        Button("Discard…") { confirmDiscard = [f] }
                        Button("Stage File") { Task { await model.stage([f]) } }.buttonStyle(.borderedProminent)
                    }
                }
                Divider()
                WithFilePanel(file: f.kind == .untracked || f.kind == .added ? nil : FileTarget(worktree: model.path, path: f.path), version: d.uid) {
                if d.isBinary { ContentUnavailableView("Binary File", systemImage: "doc.zipper") }
                else {
                    DiffTableView(file: d,
                                  hunkActions: f.staged ? [.unstage] : f.kind == .untracked ? [] : [.discard, .stage],
                                  onHunkAction: { a, lines in
                                      if a == .discard {
                                          let alert = NSAlert()
                                          alert.messageText = "Discard these changes?"
                                          alert.informativeText = "The change is removed from the working tree. This can't be undone."
                                          alert.addButton(withTitle: "Discard").hasDestructiveAction = true
                                          alert.addButton(withTitle: "Cancel")
                                          guard alert.runModal() == .alertFirstButtonReturn else { return }
                                      }
                                      Task { await model.apply(a, lines: lines) }
                                  },
                                  onNavigate: { word, action in Task { await model.browser.navigate(word, action, from: f.path) } },
                                  // A jump's line, once this is the file it landed in.
                                  reveal: model.browser.selectedPath == d.path ? model.browser.reveal : nil,
                                  changeRuler: fullMode)
                }
                }
            }
        } else if model.selectedFile != nil {
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if model.files.isEmpty {
            ContentUnavailableView("No Changes", systemImage: "checkmark.seal", description: Text("Edits in this worktree show up here as you make them."))
        } else {
            ContentUnavailableView("No File Selected", systemImage: "doc.text.magnifyingglass")
        }
    }
}

struct DiffHeader<Actions: View>: View {
    var path: String
    var oldPath: String?
    var additions: Int
    var deletions: Int
    var file: FileTarget? = nil   // offers Blame and History for it
    var showCounts = true
    @ViewBuilder var actions: Actions
    @AppStorage("diffSide") private var side = DiffSide.diff
    @AppStorage("filePanel") private var panel = FilePanel.none

    var body: some View {
        HStack(spacing: 8) {
            FileIcon(path: path)
            if let oldPath { Text(oldPath).foregroundStyle(.secondary).lineLimit(1); Image(systemName: "arrow.right").font(.caption) }
            Text(path).fontWeight(.medium).lineLimit(1).truncationMode(.head).textSelection(.enabled)
            if showCounts {
                Text(verbatim: "+\(String(additions))").foregroundStyle(.green).monospacedDigit()
                Text(verbatim: "−\(String(deletions))").foregroundStyle(.red).monospacedDigit()
            }
            Spacer()
            // Before / After only mean something when the file has both: a new or deleted file is one side.
            if additions > 0 && deletions > 0 {
                Picker("Show", selection: $side) {
                    Text("Diff").tag(DiffSide.diff)
                    Text("Before").tag(DiffSide.old)
                    Text("After").tag(DiffSide.new)
                }
                .pickerStyle(.segmented).labelsHidden().fixedSize().controlSize(.small)
                .help("The changes, the file before them, or the file after them")
            }
            if let file {
                Group {
                    // One panel at a time: a second click closes it, the other button switches to the other.
                    panelButton(.blame, "person.text.rectangle", "Blame: who last changed each line")
                    panelButton(.history, "clock.arrow.circlepath", "File History")
                }
                .buttonStyle(.borderless)
                if !(Actions.self == EmptyView.self) { Divider().frame(height: 14) }
            }
            actions.controlSize(.small)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .background(.bar, ignoresSafeAreaEdges: [])
    }

    private func panelButton(_ p: FilePanel, _ icon: String, _ help: String) -> some View {
        Button { panel = panel == p ? .none : p } label: {
            Image(systemName: icon)
                .foregroundStyle(panel == p ? Color.accentColor : .secondary)
                .padding(3)
                .background(panel == p ? Color.accentColor.opacity(0.15) : .clear, in: RoundedRectangle(cornerRadius: 4))
        }
        .help(help)
    }
}

/// Under the file list: stashes are set-aside changes, so Changes points at them; History shows and applies them.
struct StashesLink: View {
    var model: WorktreeModel
    @AppStorage("tab") private var tab = "changes"

    var body: some View {
        Button {
            model.history.setScope(.stashes)
            tab = "history"
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "tray.full").foregroundStyle(.secondary)
                Text(model.stashCount == 1 ? "1 stash" : "\(model.stashCount) stashes")
                Spacer()
                Image(systemName: "chevron.right").font(.caption.weight(.semibold)).foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 12).padding(.vertical, 7)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("Show stashes in History")
    }
}

struct CommitBox: View {
    @Bindable var model: WorktreeModel
    @State private var stashing = false
    @State private var stashMessage = ""
    @State private var stashUntracked = true
    @AppStorage("aiHarness") private var aiHarness = CommitAI.defaultHarness   // re-render when Settings changes it

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                TextField("Summary", text: title).textFieldStyle(.roundedBorder)
                    .onSubmit { if canCommit { Task { await model.commit() } } }
                if let h = CommitAI.selected {
                    if model.generatingMessage { ProgressView().controlSize(.small).frame(width: 20) }
                    else {
                        let usable = !model.files.isEmpty || model.amend
                        Button { Task { await model.generateMessage() } } label: {
                            Image(systemName: "sparkles").foregroundStyle(usable ? Color.accentColor : Color.secondary)
                        }
                            .buttonStyle(.borderless)
                            .disabled(!usable)
                            .help("\(model.commitMessage.isEmpty ? "Write" : "Rewrite") the message with \(h.name), from the staged changes (or all of them when nothing is staged)")
                    }
                }
            }
            TextEditor(text: description)
                .font(.body)
                .scrollContentBackground(.hidden)
                .frame(height: 56)
                .overlay(alignment: .topLeading) {
                    if description.wrappedValue.isEmpty {
                        Text("Description").foregroundStyle(.tertiary).padding(.leading, 5).allowsHitTesting(false)
                    }
                }
            HStack {
                Toggle("Amend", isOn: $model.amend)
                    .toggleStyle(.checkbox)
                    .onChange(of: model.amend) { _, on in if on && model.commitMessage.isEmpty { Task { await model.loadLastCommitMessage() } } }
                Spacer()
                Button("Stash…") { stashing = true }
                    .disabled(model.files.isEmpty)
                    .help("Set all changes aside without committing them")
                    .popover(isPresented: $stashing, arrowEdge: .top) {
                        VStack(alignment: .leading, spacing: 10) {
                            TextField("Message (optional)", text: $stashMessage).textFieldStyle(.roundedBorder).frame(width: 260)
                                .onSubmit { stash() }
                            Toggle("Include untracked files", isOn: $stashUntracked).toggleStyle(.checkbox)
                            HStack { Spacer(); Button("Stash") { stash() }.buttonStyle(.borderedProminent).keyboardShortcut(.defaultAction) }
                        }
                        .padding(14)
                    }
                Button {
                    Task { await model.commit() }
                } label: {
                    Text(model.amend ? "Amend" : model.staged.isEmpty ? "Commit" : "Commit \(model.staged.count)")
                        .frame(minWidth: 70)
                }
                .buttonStyle(.borderedProminent)
                .shortcut(.commit)
                .disabled(!canCommit)
            }
        }
        .padding(10)
        .background(.background.secondary)
    }

    private var canCommit: Bool { !title.wrappedValue.trimmingCharacters(in: .whitespaces).isEmpty && (!model.staged.isEmpty || model.amend) }

    /// The message is one string (what git takes, what the AI writes); the fields edit its first line and the rest.
    private var title: Binding<String> {
        Binding(get: { String(model.commitMessage.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false).first ?? "") },
                set: { t in model.commitMessage = Self.join(t.replacingOccurrences(of: "\n", with: " "), description.wrappedValue) })
    }
    private var description: Binding<String> {
        Binding(get: {
            let parts = model.commitMessage.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false)
            return parts.count > 1 ? String(parts[1].trimmingPrefix(while: \.isNewline)) : ""
        }, set: { d in model.commitMessage = Self.join(title.wrappedValue, d) })
    }
    private static func join(_ t: String, _ d: String) -> String { d.isEmpty ? t : t + "\n\n" + d }

    private func stash() {
        let (m, u) = (stashMessage, stashUntracked)
        stashing = false; stashMessage = ""
        Task { await model.stash(message: m, untracked: u) }
    }
}

// MARK: - Full mode: the worktree as an IDE

/// Every file in the worktree; changed ones carry their status.
struct WorktreeTree: View {
    var model: WorktreeModel
    @Bindable var browser: WorktreeBrowser

    var body: some View {
        Group {
            if browser.tree.isEmpty { ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity) }
            else {
                FileTreeList(paths: browser.tree, selection: $browser.selectedPath, collapsed: $browser.collapsed) { path in
                    let c = browser.change(path)
                    HStack(spacing: 6) {
                        FileIcon(path: path)
                        Text((path as NSString).lastPathComponent).lineLimit(1).truncationMode(.middle)
                            .foregroundStyle(c == nil ? .secondary : .primary)
                        Spacer(minLength: 4)
                        if let c { StatusIcon(kind: c.kind) }
                    }
                    .help(path)
                } dirAccessory: { _ in EmptyView() }
            }
        }
        .frame(maxHeight: .infinity)
        .task(id: "\(model.statusVersion) \(model.changeCount)") { await browser.loadTree() }
    }
}

/// The open file in tabs: the whole file, its changes against HEAD inline, with code navigation.
struct WorktreeFilePane: View {
    var model: WorktreeModel
    var browser: WorktreeBrowser
    @Binding var confirmDiscard: [FileChange]?

    var body: some View {
        VStack(spacing: 0) {
            if !browser.tabs.isEmpty {
                FileTabBar(browser: browser)
                Divider()
            }
            if let path = browser.selectedPath { file(path) }
            else { ContentUnavailableView("No File Open", systemImage: "doc.text.magnifyingglass", description: Text("Open one from the tree, or ⌘-click a name to go to its definition.")) }
        }
        // ⌘W closes the open tab rather than the window; with no tab open it does nothing.
        .background(CommandWInterceptor(action: { if let p = browser.selectedPath { browser.closeTab(p) } }))
        // Opening in Full mode with nothing open: the change the list had selected.
        .task(id: model.selectedFileID) { if browser.tabs.isEmpty, let f = model.selectedFile { browser.selectedPath = f.path } }
    }

    @ViewBuilder private func file(_ path: String) -> some View {
        let c = browser.change(path), f = browser.files[path]
        VStack(spacing: 0) {
            DiffHeader(path: path, additions: f?.additions ?? 0, deletions: f?.deletions ?? 0,
                       file: c?.kind == .untracked || c?.kind == .added ? nil : FileTarget(worktree: model.path, path: path),
                       showCounts: c != nil) {
                FileNavControls(browser: browser, path: path)
                if let c {
                    if c.staged { Button("Unstage File") { Task { await model.unstage([c]) } } }
                    else {
                        Button("Discard…") { confirmDiscard = [c] }
                        Button("Stage File") { Task { await model.stage([c]) } }.buttonStyle(.borderedProminent)
                    }
                }
            }
            Divider()
            WithFilePanel(file: c?.kind == .untracked || c?.kind == .added ? nil : FileTarget(worktree: model.path, path: path), version: f?.uid) {
                if let f {
                    if f.isBinary { ContentUnavailableView("Binary File", systemImage: "doc.zipper") }
                    else if f.lines.isEmpty { ContentUnavailableView("Empty File", systemImage: "doc") }
                    else {
                        DiffTableView(file: f, onNavigate: { word, action in Task { await browser.navigate(word, action, from: path) } },
                                      reveal: browser.reveal, changeRuler: true)
                    }
                } else {
                    ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
        }
        .task(id: path + " " + String(model.statusVersion)) { await browser.load(path) }
    }
}

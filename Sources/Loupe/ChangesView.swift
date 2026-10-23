import SwiftUI

struct WorktreeView: View {
    @Environment(AppModel.self) private var app
    var model: WorktreeModel
    @AppStorage("tab") private var tab = "changes"
    @AppStorage("layout") private var layout = DiffLayout.unified
    @State private var showBranches = false

    var body: some View {
        Group {
            switch tab {
            case "history": HistoryView(worktree: model, model: model.history, layout: layout)
            case "pulls": PullRequestsView(model: model, layout: layout)
            default: ChangesView(model: model, layout: layout)
            }
        }
        .navigationTitle(model.headOID.isEmpty ? "" : model.branch ?? "detached HEAD")
        .navigationSubtitle((model.path as NSString).abbreviatingWithTildeInPath)
        .toolbar(removing: .title)   // drawn by the branch button below instead
        .toolbar {
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
            ToolbarItemGroup {
                Picker("Layout", selection: $layout) {
                    Image(systemName: "rectangle").help("Unified").tag(DiffLayout.unified)
                    Image(systemName: "rectangle.split.2x1").help("Split").tag(DiffLayout.split)
                }
                .pickerStyle(.segmented)
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
        }
        .alert("Git", isPresented: Binding(get: { model.error != nil }, set: { if !$0 { model.error = nil } })) {
            Button("OK") { model.error = nil }
        } message: { Text(model.error ?? "") }
        .onReceive(NotificationCenter.default.publisher(for: .refresh)) { _ in Task { await model.refreshStatus() } }
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
    var layout: DiffLayout
    @State private var confirmDiscard: [FileChange]?

    var body: some View {
        VStack(spacing: 0) {
        if model.operation != nil || !model.conflicted.isEmpty {
            OperationBanner(model: model)
            Divider()
        }
        ResizableSplit(key: "changesListWidth", minWidth: 220, maxWidth: 380, defaultFraction: 300/380) {
            VStack(spacing: 0) {
                FileListView(model: model, confirmDiscard: $confirmDiscard)
                if model.stashCount > 0 { StashesLink(model: model) }
                Divider()
                CommitBox(model: model)
            }
            .frame(maxHeight: .infinity)
        } trailing: {

            Group {
                if let f = model.selectedFile, f.kind == .conflicted { ConflictPane(model: model, path: f.path).id(f.path) }
                else { DiffPane(model: model, layout: layout, confirmDiscard: $confirmDiscard) }
            }
            .frame(minWidth: 360, maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
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
    @Environment(\.openWindow) private var openWindow

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
                    ForEach(model.conflicted) { f in row(f) }
                } header: {
                    sectionHeader("Conflicts", count: model.conflicted.count, action: nil) {}
                }
            }
            if !model.staged.isEmpty {
                Section {
                    ForEach(model.staged) { f in row(f) }
                } header: {
                    sectionHeader("Staged", count: model.staged.count, action: "Unstage All") { Task { await model.unstage(model.staged) } }
                }
            }
            Section {
                ForEach(model.unstaged) { f in row(f) }
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

    private func row(_ f: FileChange) -> some View {
        FileRow(path: f.path, badge: f.kind).tag(f.id)
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
            Button("Blame") { openWindow(id: "file", value: FileTarget(worktree: model.path, path: f.path)) }
            Button("File History") { openWindow(id: "file", value: FileTarget(worktree: model.path, path: f.path, mode: "history")) }
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
    var additions: Int? = nil
    var deletions: Int? = nil

    var body: some View {
        HStack(spacing: 7) {
            FileIcon(path: path)
            let dir = (path as NSString).deletingLastPathComponent
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
    var layout: DiffLayout
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
                if d.isBinary { ContentUnavailableView("Binary File", systemImage: "doc.zipper") }
                else {
                    DiffTableView(file: d, layout: layout,
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
                                  })
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
    @ViewBuilder var actions: Actions
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "doc.text").foregroundStyle(.secondary)
            if let oldPath { Text(oldPath).foregroundStyle(.secondary).lineLimit(1); Image(systemName: "arrow.right").font(.caption) }
            Text(path).fontWeight(.medium).lineLimit(1).truncationMode(.head).textSelection(.enabled)
            Text(verbatim: "+\(String(additions))").foregroundStyle(.green).monospacedDigit()
            Text(verbatim: "−\(String(deletions))").foregroundStyle(.red).monospacedDigit()
            Spacer()
            if let file {
                Group {
                    Button { openWindow(id: "file", value: file) } label: { Image(systemName: "person.text.rectangle") }
                        .help("Blame: who last changed each line")
                    Button { var f = file; f.mode = "history"; openWindow(id: "file", value: f) } label: { Image(systemName: "clock.arrow.circlepath") }
                        .help("File History")
                }
                .buttonStyle(.borderless)
                if !(Actions.self == EmptyView.self) { Divider().frame(height: 14) }
            }
            actions.controlSize(.small)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .background(.bar)
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

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
            func refs(_ ns: String) async -> [String] {
                let out = (try? await Git.string(["for-each-ref", "--format=%(refname:short)", "--sort=-committerdate", ns], in: model.path)) ?? ""
                return out.split(separator: "\n").map(String.init)
            }
            locals = await refs("refs/heads")
            remotes = await refs("refs/remotes").filter { b in
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
            do {
                // "origin/x" with no local x (those are hidden) → new local x tracking it.
                let args = remotes.contains(b) ? ["switch", "-c", String(b.split(separator: "/", maxSplits: 1)[1]), "--track", b] : ["switch", b]
                // Conflicting uncommitted changes, or a branch checked out in another worktree: git refuses and says why.
                try await Git.run(args, in: model.path)
                await model.app.refreshWorktrees(repo: model.app.repo(containing: model.path) ?? model.path)
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
        HSplitView {
            VStack(spacing: 0) {
                FileListView(model: model, confirmDiscard: $confirmDiscard)
                Divider()
                CommitBox(model: model)
            }
            .frame(minWidth: 220, idealWidth: 300, maxWidth: 380, maxHeight: .infinity)

            DiffPane(model: model, layout: layout, confirmDiscard: $confirmDiscard)
                .frame(minWidth: 360, maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .confirmationDialog("Discard changes?", isPresented: Binding(get: { confirmDiscard != nil }, set: { if !$0 { confirmDiscard = nil } })) {
            Button("Discard \(confirmDiscard?.count ?? 0) file(s)", role: .destructive) {
                if let fs = confirmDiscard { Task { await model.discard(fs) } }
            }
        } message: { Text("Tracked changes are reverted; untracked files are moved to the Trash.") }
        .onReceive(NotificationCenter.default.publisher(for: .moveFile)) { n in
            let list = model.staged + model.unstaged
            guard let d = n.object as? Int, let i = list.firstIndex(where: { $0.id == model.selectedFileID }) else { return }
            model.select(list[max(0, min(list.count - 1, i + d))].id)
        }
    }
}

struct FileListView: View {
    var model: WorktreeModel
    @Binding var confirmDiscard: [FileChange]?

    var body: some View {
        let selection = Binding(get: { model.selectedFileID }, set: { model.select($0) })
        List(selection: selection) {
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
        .onKeyPress(.space) {
            guard let f = model.selectedFile else { return .ignored }
            Task { await f.staged ? model.unstage([f]) : model.stage([f]) }
            return .handled
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
        FileRow(path: f.path, badge: f.kind)
            .tag(f.id)
            .contextMenu {
                if f.staged { Button("Unstage") { Task { await model.unstage([f]) } } }
                else { Button("Stage") { Task { await model.stage([f]) } } }
                if !f.staged { Button("Discard Changes…", role: .destructive) { confirmDiscard = [f] } }
                Divider()
                Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: model.path).appendingPathComponent(f.path)]) }
                Button("Open") { NSWorkspace.shared.open(URL(fileURLWithPath: model.path).appendingPathComponent(f.path)) }
                Button("Copy Path") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(f.path, forType: .string) }
            }
            .onTapGesture(count: 2) { Task { await f.staged ? model.unstage([f]) : model.stage([f]) } }
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
                DiffHeader(path: d.path, oldPath: d.oldPath != d.newPath ? d.oldPath : nil, additions: d.additions, deletions: d.deletions) {
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
                                  onHunkAction: { i, a in
                                      if a == .discard {
                                          let alert = NSAlert()
                                          alert.messageText = "Discard this hunk?"
                                          alert.informativeText = "The change is removed from the working tree. This can't be undone."
                                          alert.addButton(withTitle: "Discard").hasDestructiveAction = true
                                          alert.addButton(withTitle: "Cancel")
                                          guard alert.runModal() == .alertFirstButtonReturn else { return }
                                      }
                                      Task { await model.hunk(i, a) }
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
    @ViewBuilder var actions: Actions

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "doc.text").foregroundStyle(.secondary)
            if let oldPath { Text(oldPath).foregroundStyle(.secondary).lineLimit(1); Image(systemName: "arrow.right").font(.caption) }
            Text(path).fontWeight(.medium).lineLimit(1).truncationMode(.head).textSelection(.enabled)
            Text(verbatim: "+\(String(additions))").foregroundStyle(.green).monospacedDigit()
            Text(verbatim: "−\(String(deletions))").foregroundStyle(.red).monospacedDigit()
            Spacer()
            actions.controlSize(.small)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .background(.bar)
    }
}

struct CommitBox: View {
    @Bindable var model: WorktreeModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            TextEditor(text: $model.commitMessage)
                .font(.body)
                .scrollContentBackground(.hidden)
                .frame(height: 76)
                .overlay(alignment: .topLeading) {
                    if model.commitMessage.isEmpty {
                        Text("Commit message").foregroundStyle(.tertiary).padding(.leading, 5).allowsHitTesting(false)
                    }
                }
            HStack {
                Toggle("Amend", isOn: $model.amend)
                    .toggleStyle(.checkbox)
                    .onChange(of: model.amend) { _, on in if on && model.commitMessage.isEmpty { Task { await model.loadLastCommitMessage() } } }
                Spacer()
                Button {
                    Task { await model.commit() }
                } label: {
                    Text(model.amend ? "Amend" : model.staged.isEmpty ? "Commit" : "Commit \(model.staged.count)")
                        .frame(minWidth: 70)
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.return, modifiers: .command)
                .disabled(model.commitMessage.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || (model.staged.isEmpty && !model.amend))
            }
        }
        .padding(10)
        .background(.background.secondary)
    }
}

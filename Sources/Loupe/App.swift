import SwiftUI
import AppKit

@main
struct LoupeApp: App {
    @NSApplicationDelegateAdaptor private var delegate: AppDelegate
    @State private var app = AppModel()

    init() {
        // Dev aid: `Loupe --api /repos/o/r/pulls/1/reviews` prints a Forgejo response using the app's own Keychain
        // access (the terminal's `security` tool gets an ACL prompt for Loupe's item).
        if let i = CommandLine.arguments.firstIndex(of: "--api"), i + 1 < CommandLine.arguments.count {
            let path = CommandLine.arguments[i + 1]
            let accts = (try? JSONDecoder().decode([ForgejoAccount].self, from: UserDefaults.standard.data(forKey: "accounts") ?? Data())) ?? []
            guard let a = accts.first, let t = a.readToken(), let url = URL(string: a.baseURL + "/api/v1" + path) else { print("no account/token"); exit(1) }
            var r = URLRequest(url: url); r.setValue("token \(t)", forHTTPHeaderField: "Authorization")
            let sem = DispatchSemaphore(value: 0)
            URLSession.shared.dataTask(with: r) { d, _, _ in FileHandle.standardOutput.write(d ?? Data()); sem.signal() }.resume()
            sem.wait(); exit(0)
        }
    }

    var body: some Scene {
        // WindowGroup, not Window: a single `Window` scene was intermittently not presented at launch.
        WindowGroup("Loupe", id: "main") {
            ContentView()
                .environment(app)
                .frame(minWidth: 860, minHeight: 560)
        }
        .defaultSize(width: 1440, height: 900)
        .defaultLaunchBehavior(.presented)
        .restorationBehavior(.disabled)
        .windowToolbarStyle(.unified)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("Open Repository…") { openRepository(app) }.keyboardShortcut("o")
            }
            CommandGroup(after: .sidebar) {
                Button("Go to Worktree…") { app.showSwitcher = true }.keyboardShortcut("k")
            }
            CommandMenu("Review") {
                Button("Next File") { NotificationCenter.default.post(name: .moveFile, object: 1) }.keyboardShortcut(.downArrow, modifiers: .option)
                Button("Previous File") { NotificationCenter.default.post(name: .moveFile, object: -1) }.keyboardShortcut(.upArrow, modifiers: .option)
                Button("Mark Viewed & Next") { NotificationCenter.default.post(name: .markViewed, object: nil) }.keyboardShortcut("v", modifiers: [.command, .shift])
                Divider()
                Button("Refresh") { NotificationCenter.default.post(name: .refresh, object: nil) }.keyboardShortcut("r")
            }
        }

        Settings {
            SettingsView().environment(app)
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ n: Notification) {
        // Avatars come through AsyncImage → URLSession.shared; the default cache is too small to keep a PR list's worth.
        URLCache.shared = URLCache(memoryCapacity: 32 << 20, diskCapacity: 128 << 20)
        // Launched as a bare SwiftPM binary there's no bundle telling macOS we're a GUI app.
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()
        // Launched while another app is frontmost (Dock click during typing, `open -g`), macOS 14+ refuses the
        // activation and SwiftUI leaves the window un-ordered: the app runs with nothing on screen. Show it anyway.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
            guard !NSApp.windows.contains(where: { $0.isVisible && $0.canBecomeMain }) else { return }
            let existing = NSApp.windows.filter(\.canBecomeMain)
            if !existing.isEmpty { existing.forEach { $0.orderFrontRegardless() }; return }
            // SwiftUI never created the window. A "reopen" Apple Event (what a Dock click sends) makes it open one.
            let reopen = NSAppleEventDescriptor(eventClass: AEEventClass(kCoreEventClass), eventID: AEEventID(kAEReopenApplication),
                                                targetDescriptor: NSAppleEventDescriptor(processIdentifier: ProcessInfo.processInfo.processIdentifier),
                                                returnID: AEReturnID(kAutoGenerateReturnID), transactionID: AETransactionID(kAnyTransactionID))
            _ = try? reopen.sendEvent(options: [.noReply], timeout: 1)
        }
    }

    func applicationDidBecomeActive(_ n: Notification) {
        if !NSApp.windows.contains(where: { $0.isVisible && $0.canBecomeMain }) {
            NSApp.windows.filter(\.canBecomeMain).first?.makeKeyAndOrderFront(nil)
        }
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

extension Notification.Name {
    static let moveFile = Notification.Name("loupe.moveFile")
    static let markViewed = Notification.Name("loupe.markViewed")
    static let refresh = Notification.Name("loupe.refresh")
}

@MainActor func openRepository(_ app: AppModel) {
    let panel = NSOpenPanel()
    panel.canChooseDirectories = true
    panel.canChooseFiles = false
    panel.allowsMultipleSelection = true
    panel.prompt = "Open"
    guard panel.runModal() == .OK else { return }
    for url in panel.urls { Task { await app.addRepo(url) } }
}

// MARK: - Root

struct ContentView: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        @Bindable var app = app
        NavigationSplitView {
            Sidebar()
                .navigationSplitViewColumnWidth(min: 200, ideal: 250, max: 360)
        } detail: {
            if let sel = app.selection {
                WorktreeView(model: app.model(for: sel)).id(sel)
            } else {
                ContentUnavailableView {
                    Label("No Worktree Selected", systemImage: "arrow.triangle.branch")
                } description: {
                    Text("Open a repository to review its changes and pull requests.")
                } actions: {
                    Button("Open Repository…") { openRepository(app) }.buttonStyle(.borderedProminent)
                }
            }
        }
        .overlay {
            if let o = app.checksOverlay {
                ChecksOverlay(model: o.model, baseURL: o.baseURL) { withAnimation(.easeOut(duration: 0.15)) { app.checksOverlay = nil } }
            }
        }
        .sheet(isPresented: $app.showSwitcher) { QuickSwitcher() }
        .alert("Error", isPresented: Binding(get: { app.error != nil }, set: { if !$0 { app.error = nil } })) {
            Button("OK") { app.error = nil }
        } message: { Text(app.error ?? "") }
        .onDrop(of: [.fileURL], isTargeted: nil) { providers in
            for p in providers {
                _ = p.loadObject(ofClass: URL.self) { url, _ in if let url { Task { @MainActor in await app.addRepo(url) } } }
            }
            return true
        }
    }
}

// MARK: - Sidebar

struct Sidebar: View {
    @Environment(AppModel.self) private var app
    @State private var addingWorktreeFor: String?
    @State private var removing: (repo: String, wt: Worktree)?

    var body: some View {
        @Bindable var app = app
        List(selection: $app.selection) {
            ForEach(app.repoPaths, id: \.self) { repo in
                Section {
                    ForEach(app.collapsed.contains(repo) ? [] : app.worktrees[repo] ?? []) { wt in
                        WorktreeRow(wt: wt, isMain: wt.path == repo, model: app.model(for: wt.path))
                            .tag(wt.path)
                            .contextMenu { worktreeMenu(repo: repo, wt: wt) }
                    }
                } header: {
                    let expanded = !app.collapsed.contains(repo)
                    HStack(spacing: 4) {
                        Button {
                            withAnimation(.easeOut(duration: 0.15)) {
                                if expanded { app.collapsed.insert(repo) } else { app.collapsed.remove(repo) }
                            }
                        } label: {
                            HStack(spacing: 4) {
                                Image(systemName: "chevron.right").font(.caption2.weight(.semibold))
                                    .rotationEffect(.degrees(expanded ? 90 : 0))
                                Text((repo as NSString).lastPathComponent)
                            }
                        }
                        .buttonStyle(.plain)
                        Spacer()
                        // Collapsed: still show whether anything in this repo has uncommitted work.
                        let changes = (app.worktrees[repo] ?? []).reduce(0) { $0 + app.model(for: $1.path).changeCount }
                        if !expanded && changes > 0 {
                            Text("\(changes)").font(.caption.monospacedDigit()).padding(.horizontal, 6).padding(.vertical, 1)
                                .background(.quaternary, in: Capsule())
                        }
                    }
                    .selectionDisabled()   // otherwise the first row's selection highlight also wraps the header
                    .contextMenu {
                        Button("New Worktree…") { addingWorktreeFor = repo }
                        Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: repo)]) }
                        Button("Prune Stale Worktrees") { Task { try? await Git.run(["worktree", "prune"], in: repo); await app.refreshWorktrees(repo: repo) } }
                        Divider()
                        Button("Remove from Loupe") { app.removeRepo(repo) }
                    }
                }
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom) {
            HStack {
                Button { openRepository(app) } label: { Label("Open Repository", systemImage: "plus.rectangle.on.folder") }
                    .buttonStyle(.borderless)
                Spacer()
                // New worktree for the repo of the selected worktree (or the only repo).
                if let repo = app.selection.flatMap(app.repo(containing:)) ?? app.repoPaths.first {
                    Button { addingWorktreeFor = repo } label: { Image(systemName: "plus") }
                        .buttonStyle(.borderless)
                        .help("New Worktree in \((repo as NSString).lastPathComponent)…")
                }
            }
            .padding(10)
        }
        .sheet(item: Binding(get: { addingWorktreeFor.map(IdentifiedString.init) }, set: { addingWorktreeFor = $0?.value })) { r in
            AddWorktreeSheet(repo: r.value)
        }
        .confirmationDialog("Remove worktree “\(removing?.wt.name ?? "")”?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } })) {
            Button("Remove", role: .destructive) { remove(force: false) }
            Button("Force Remove (discard changes)", role: .destructive) { remove(force: true) }
        } message: { Text("The directory is deleted. The branch is kept.") }
    }

    private func remove(force: Bool) {
        guard let r = removing else { return }
        Task { do { try await app.removeWorktree(repo: r.repo, path: r.wt.path, force: force) } catch { app.error = error.localizedDescription } }
    }

    @ViewBuilder private func worktreeMenu(repo: String, wt: Worktree) -> some View {
        Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: wt.path)]) }
        Button("Open in Terminal") { openIn(bundleID: "com.apple.Terminal", wt.path) }
        Button("Open in Zed") { openIn(bundleID: "dev.zed.Zed", wt.path) }
        Button("Open in VS Code") { openIn(bundleID: "com.microsoft.VSCode", wt.path) }
        Button("Copy Path") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(wt.path, forType: .string) }
        if wt.path != repo {
            Divider()
            Button("Remove Worktree…", role: .destructive) { removing = (repo, wt) }
        }
    }
}

struct IdentifiedString: Identifiable { var value: String; var id: String { value } }

func openIn(bundleID: String, _ path: String) {
    guard let app = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID) else { NSSound.beep(); return }
    NSWorkspace.shared.open([URL(fileURLWithPath: path)], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
}

struct WorktreeRow: View {
    var wt: Worktree
    var isMain: Bool
    var model: WorktreeModel

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: isMain ? "folder" : "arrow.triangle.branch")
                .foregroundStyle(isMain ? Color.secondary : Color.accentColor)
            VStack(alignment: .leading, spacing: 1) {
                Text(wt.branch ?? "detached @ \(wt.head.prefix(7))").lineLimit(1)
                if !isMain { Text(wt.name).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
            }
            Spacer(minLength: 4)
            if model.ahead > 0 { Text(verbatim: "↑\(String(model.ahead))").font(.caption2.monospacedDigit()).foregroundStyle(.secondary) }
            if model.behind > 0 { Text(verbatim: "↓\(String(model.behind))").font(.caption2.monospacedDigit()).foregroundStyle(.secondary) }
            if model.changeCount > 0 {
                Text("\(model.changeCount)").font(.caption.monospacedDigit()).padding(.horizontal, 6).padding(.vertical, 1)
                    .background(.quaternary, in: Capsule())
            }
            if wt.isLocked { Image(systemName: "lock.fill").font(.caption2).foregroundStyle(.secondary) }
        }
        .help(wt.path)
    }
}

// MARK: - Add worktree

struct AddWorktreeSheet: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    var repo: String
    var initialBase = ""
    @State private var branch = ""
    @State private var newBranch = true
    @State private var base = ""
    @State private var path = ""
    @State private var pathEdited = false
    @State private var branches: [String] = []
    @State private var error: String?
    @State private var working = false

    var body: some View {
        Form {
            Picker("Branch", selection: $newBranch) {
                Text("New branch").tag(true)
                Text("Existing branch").tag(false)
            }
            .pickerStyle(.segmented)
            if newBranch {
                TextField("Name", text: $branch, prompt: Text("feature/my-change"))
                Picker("Based on", selection: $base) {
                    Text("HEAD").tag("")
                    if !initialBase.isEmpty && !branches.contains(initialBase) { Text(String(initialBase.prefix(10))).tag(initialBase) }
                    ForEach(branches, id: \.self) { Text($0).tag($0) }
                }
            } else {
                Picker("Branch", selection: $branch) {
                    Text("Choose…").tag("")
                    ForEach(branches, id: \.self) { Text($0).tag($0) }
                }
            }
            HStack {
                TextField("Location", text: Binding(get: { path }, set: { path = $0; pathEdited = true }))
                Button("Choose…") {
                    let p = NSSavePanel()
                    p.nameFieldStringValue = (path as NSString).lastPathComponent
                    p.directoryURL = URL(fileURLWithPath: (path as NSString).deletingLastPathComponent)
                    if p.runModal() == .OK, let u = p.url { path = u.path; pathEdited = true }
                }
            }
            if let error { Text(error).foregroundStyle(.red).font(.callout) }
        }
        .formStyle(.grouped)
        .frame(width: 480)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            ToolbarItem(placement: .confirmationAction) {
                Button("Create") { create() }.disabled(branch.isEmpty || path.isEmpty || working)
            }
        }
        .navigationTitle("New Worktree")
        .task {
            let out = (try? await Git.string(["for-each-ref", "--format=%(refname:short)", "--sort=-committerdate", "refs/heads", "refs/remotes"], in: repo)) ?? ""
            branches = out.split(separator: "\n").map(String.init).filter { !$0.hasSuffix("/HEAD") }
            base = initialBase
            suggestPath()
        }
        .onChange(of: branch) { suggestPath() }
    }

    private func suggestPath() {
        guard !pathEdited else { return }
        let name = (repo as NSString).lastPathComponent
        let slug = branch.isEmpty ? "worktree" : branch.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: "origin-", with: "")
        path = ((repo as NSString).deletingLastPathComponent as NSString).appendingPathComponent("\(name)-\(slug)")
    }

    private func create() {
        working = true
        Task {
            defer { working = false }
            do {
                // Picking "origin/x" as an existing branch → create local "x" tracking it.
                if !newBranch, branch.contains("/"), branches.contains(branch), !(try await isLocalBranch(branch)) {
                    let local = String(branch.split(separator: "/", maxSplits: 1)[1])
                    try await app.addWorktree(repo: repo, path: path, branch: local, base: branch, newBranch: true)
                } else {
                    try await app.addWorktree(repo: repo, path: path, branch: branch, base: base, newBranch: newBranch)
                }
                dismiss()
            } catch { self.error = error.localizedDescription }
        }
    }

    private func isLocalBranch(_ b: String) async throws -> Bool {
        (try? await Git.run(["show-ref", "--verify", "--quiet", "refs/heads/\(b)"], in: repo)) != nil
    }
}

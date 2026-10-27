import SwiftUI
import AppKit

@main
struct LoupeApp: App {
    @NSApplicationDelegateAdaptor private var delegate: AppDelegate
    @State private var app = AppModel()

    init() {
        // Dev aid: `Loupe --api /repos/o/r/pulls/1/reviews` prints a Forgejo response using the app's own Keychain
        // access (the terminal's `security` tool gets an ACL prompt for Loupe's item).
        // Dev aid: `Loupe --lens <repo> <revA> <revB|worktree>` prints the change lenses' report.
        if let i = CommandLine.arguments.firstIndex(of: "--lens"), i + 3 < CommandLine.arguments.count {
            let a = CommandLine.arguments
            let (dir, ra, rb) = (a[i + 1], a[i + 2], a[i + 3])
            let sem = DispatchSemaphore(value: 0)
            Task.detached {
                let b: Snapshot = rb == "worktree" ? .worktree : .rev(rb)
                let raw = (try? await Git.string(["diff", "--name-status", "-z", "-M", ra] + (rb == "worktree" ? [] : [rb]), in: dir)) ?? ""
                let f = raw.split(separator: "\0", omittingEmptySubsequences: false).map(String.init)
                var changes: [ChangedPath] = [], k = 0
                while k < f.count, !f[k].isEmpty {
                    let st = f[k]
                    if st.hasPrefix("R") || st.hasPrefix("C") { changes.append(ChangedPath(old: f[k + 1], new: f[k + 2])); k += 3 }
                    else if st == "A" { changes.append(ChangedPath(old: nil, new: f[k + 1])); k += 2 }
                    else if st == "D" { changes.append(ChangedPath(old: f[k + 1], new: nil)); k += 2 }
                    else { changes.append(ChangedPath(old: f[k + 1], new: f[k + 1])); k += 2 }
                }
                let start = Date()
                for r in await ChangeLens.analyze(changes, a: .rev(ra), b: b, in: dir) {
                    print("■ \(r.title): \(r.summary)")
                    for s in r.sections {
                        print("  \(s.title)")
                        for x in s.facts.prefix(40) {
                            let g = switch x.kind { case .added: "+"; case .removed: "−"; case .changed: "~"; case .renamed: "→"; case .info: "·"; case .warning: "!" }
                            print("    \(g) \(x.title)\(x.detail.map { "  — \($0)" } ?? "")\(x.path.map { "  @\($0)\(x.line.map { ":\($0)" } ?? "")" } ?? "")")
                        }
                        if s.facts.count > 40 { print("    … \(s.facts.count - 40) more") }
                    }
                }
                print(String(format: "(%.0f ms)", Date().timeIntervalSince(start) * 1000))
                sem.signal()
            }
            sem.wait(); exit(0)
        }
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

    @State private var shortcuts = Shortcuts.shared

    var body: some Scene {
        let _ = shortcuts.revision   // a changed shortcut rebuilds the menus below
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
                Button("Open Repository…") { openRepository(app) }.shortcut(.openRepository)
            }
            CommandGroup(after: .sidebar) {
                // NavigationSplitView's own sidebar button goes through the split view controller; so does this.
                Button("Toggle Sidebar") { NSApp.sendAction(#selector(NSSplitViewController.toggleSidebar(_:)), to: nil, from: nil) }
                    .shortcut(.toggleSidebar)
                Divider()
                // The tab is @AppStorage("tab") in WorktreeView, which follows UserDefaults.
                Button("Changes") { UserDefaults.standard.set("changes", forKey: "tab") }.shortcut(.changesTab)
                Button("History") { UserDefaults.standard.set("history", forKey: "tab") }.shortcut(.historyTab)
                Button("Pull Requests") { UserDefaults.standard.set("pulls", forKey: "tab") }.shortcut(.pullsTab)
                Divider()
                Button("Go to Worktree…") { app.showSwitcher = true }.shortcut(.goToWorktree)
            }
            CommandMenu("Review") {
                Button("Next File") { NotificationCenter.default.post(name: .moveFile, object: 1) }.shortcut(.nextFile)
                Button("Previous File") { NotificationCenter.default.post(name: .moveFile, object: -1) }.shortcut(.previousFile)
                Button("Mark Viewed & Next") { NotificationCenter.default.post(name: .markViewed, object: nil) }.shortcut(.markViewed)
                Button("Compact / Full Mode") { UserDefaults.standard.set(!UserDefaults.standard.bool(forKey: "prFullMode"), forKey: "prFullMode") }.shortcut(.fullMode)
                Divider()
                Button("Refresh") { NotificationCenter.default.post(name: .refresh, object: nil) }.shortcut(.refresh)
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
        Appearance.saved.apply()
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
            detail
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

    @ViewBuilder private var detail: some View {
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
}

// MARK: - Sidebar

struct Sidebar: View {
    @Environment(AppModel.self) private var app
    @State private var addingWorktreeFor: String?
    @State private var removing: (repo: String, wt: Worktree)?
    @State private var dropTarget: String?   // repo header a dragged repo would land before
    @State private var renaming: Space?
    @State private var newName = ""
    @State private var deleting: Space?

    var body: some View {
        SpacePager(index: app.spaces.firstIndex { $0.id == app.currentSpace } ?? 0, count: app.spaces.count) { step in
            app.switchSpace(by: step)   // bounds-checked: the space list can change under a swipe
        } page: { i in
            let sp = app.spaces[i]
            VStack(spacing: 0) {
                if app.spaces.count > 1 {
                    Text(sp.name).font(.title3.weight(.bold)).frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 18).padding(.vertical, 6)
                        .contextMenu { spaceMenu(sp) }
                }
                repoList(sp)
            }
        }
        .safeAreaInset(edge: .bottom) {
            HStack(spacing: 8) {
                Button { openRepository(app) } label: { Image(systemName: "plus.rectangle.on.folder") }
                    .help("Open Repository…")
                Spacer()
                ForEach(app.spaces) { sp in
                    let current = sp.id == app.currentSpace
                    Button { go(to: sp.id) } label: {
                        Image(systemName: current ? "circle.fill" : "circle").font(.system(size: 8)).padding(2).contentShape(Rectangle())
                    }
                    .foregroundStyle(current ? .primary : .secondary)
                    .help(sp.name)
                    .contextMenu { spaceMenu(sp) }
                    // Drop a repo header here to move the repo to that space.
                    .dropDestination(for: String.self) { items, _ in
                        let repos = items.filter(app.repoPaths.contains)
                        if !repos.isEmpty { app.moveRepos(repos, to: sp.id) }
                        return !repos.isEmpty
                    }
                }
                Spacer()
                Button { withAnimation(.snappy) { app.addSpace() } } label: { Image(systemName: "plus") }
                    .help("New Space")
            }
            .buttonStyle(.borderless)
            .padding(10)
            .background(.bar)   // rows scroll under the inset; without a background they show through the buttons
            .overlay(alignment: .top) { Divider() }
        }
        .sheet(item: Binding(get: { addingWorktreeFor.map(IdentifiedString.init) }, set: { addingWorktreeFor = $0?.value })) { r in
            AddWorktreeSheet(repo: r.value)
        }
        .confirmationDialog("Remove worktree “\(removing?.wt.name ?? "")”?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } })) {
            Button("Remove", role: .destructive) { remove(force: false) }
            Button("Force Remove (discard changes)", role: .destructive) { remove(force: true) }
        } message: { Text("The directory is deleted. The branch is kept.") }
        .alert("Rename Space", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Name", text: $newName)
            Button("Rename") {
                let name = newName.trimmingCharacters(in: .whitespaces)
                if let r = renaming, !name.isEmpty, let i = app.spaces.firstIndex(where: { $0.id == r.id }) { app.spaces[i].name = name }
            }
            Button("Cancel", role: .cancel) {}
        }
        .confirmationDialog("Delete space “\(deleting?.name ?? "")”?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } })) {
            Button("Delete", role: .destructive) { if let d = deleting { withAnimation(.snappy) { app.deleteSpace(d.id) } } }
        } message: { Text("Its repositories move to the neighbouring space. Its accounts are signed out and their tokens removed from the Keychain.") }
    }

    @ViewBuilder private func spaceMenu(_ sp: Space) -> some View {
        Button("Rename…") { newName = sp.name; renaming = sp }
        Button("Delete…", role: .destructive) { deleting = sp }.disabled(app.spaces.count < 2)
    }

    private func go(to id: String) {
        withAnimation(.snappy) { app.switchSpace(to: id) }
    }

    private func repoList(_ sp: Space) -> some View {
        @Bindable var app = app
        return List(selection: $app.selection) {
            ForEach(sp.repos, id: \.self) { repo in
                Section {
                    ForEach(app.collapsed.contains(repo) ? [] : app.worktrees[repo] ?? []) { wt in
                        WorktreeRow(wt: wt, repo: repo, model: app.model(for: wt.path))
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
                                    .foregroundStyle(.secondary)
                                Text((repo as NSString).lastPathComponent)
                                    .font(.body.weight(.semibold)).foregroundStyle(.primary)   // the repo leads, not its rows
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
                        Button { addingWorktreeFor = repo } label: { Image(systemName: "plus") }
                            .buttonStyle(.borderless)
                            .help("New Worktree in \((repo as NSString).lastPathComponent)…")
                    }
                    .padding(.trailing, 10)   // headers run to the list edge; line the count up with the rows' badges
                    .selectionDisabled()   // otherwise the first row's selection highlight also wraps the header
                    // Drag a header onto another to reorder, or onto a space dot to move the repo there.
                    .draggable(repo) { Text((repo as NSString).lastPathComponent).padding(6).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 6)) }
                    .dropDestination(for: String.self) { items, _ in
                        guard let r = items.first, sp.repos.contains(r) else { return false }
                        withAnimation(.snappy) { app.moveRepo(r, before: repo) }
                        return true
                    } isTargeted: { on in
                        if on { dropTarget = repo } else if dropTarget == repo { dropTarget = nil }
                    }
                    .overlay(alignment: .top) {
                        if dropTarget == repo { Capsule().fill(Color.accentColor).frame(height: 2).offset(y: -4) }
                    }
                    .contextMenu {
                        Button("New Worktree…") { addingWorktreeFor = repo }
                        Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: repo)]) }
                        Button("Prune Stale Worktrees") { Task { try? await Git.run(["worktree", "prune"], in: repo); await app.refreshWorktrees(repo: repo) } }
                        if app.spaces.count > 1 {
                            Menu("Move to Space") {
                                ForEach(app.spaces.filter { $0.id != sp.id }) { other in
                                    Button(other.name) { app.moveRepos([repo], to: other.id) }
                                }
                            }
                        }
                        Divider()
                        Button("Remove from Loupe") { app.removeRepo(repo) }
                    }
                }
            }
        }
        .listStyle(.sidebar)
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

/// Spaces paged like a photo gallery: while two fingers are on the trackpad the spaces follow them both ways, and
/// on release they spring to the nearest space (or the next one, for a flick). Touching again mid-spring catches the
/// pages where they are. Only the shown space is in the view tree at rest; its neighbours join just for a swipe or
/// the slide after a click on a space dot. Laying out every space's list on every frame made the window's own
/// animations (showing and hiding the sidebar) stutter.
struct SpacePager<Page: View>: View {
    var index: Int
    var count: Int
    var move: (Int) -> Void   // by this many spaces, once the pages come to rest on another one
    @ViewBuilder var page: (Int) -> Page
    @State private var position: CGFloat?   // in pages, while a swipe, its spring or a dot-click slide moves them
    @State private var width: CGFloat = 0
    @State private var settling = false      // the index change came from a swipe: don't slide again
    @State private var slideFrom: Int?       // a dot-click slide's old space, kept until the slide ends

    var body: some View {
        let p = position ?? CGFloat(index)
        let shown = position == nil ? [index] : Array(Set([Int(p.rounded(.down)), Int(p.rounded(.up)), index] + (slideFrom.map { [$0] } ?? []))).filter { (0..<count).contains($0) }.sorted()
        ZStack {
            ForEach(shown, id: \.self) { i in
                page(i)
                    .frame(width: width > 0 ? width : nil)
                    .frame(maxHeight: .infinity)
                    .offset(x: (CGFloat(i) - p) * width)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .clipped()
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
        .overlay(SwipePager(index: index, count: count,
            position: { position = $0 },
            rest: { page in
                // The pages already sit at `page`: swap the index in with no animation.
                var t = Transaction(); t.disablesAnimations = true
                withTransaction(t) {
                    position = nil
                    if page != index { settling = true; move(page - index) }
                }
            }))
        .onChange(of: index) { old, new in
            // A click on a space dot: slide from the old space to the new one, then drop the old one.
            if settling { settling = false; return }
            guard position == nil, old != new else { return }
            position = CGFloat(old); slideFrom = old
            withAnimation(.snappy(duration: 0.28)) { position = CGFloat(new) } completion: { position = nil; slideFrom = nil }
        }
    }
}

/// The trackpad half of SpacePager. A transparent view over the pages that takes scroll events only (clicks, drags
/// and menus hit-test straight through). Each gesture is held back for its first few points to see which way it
/// goes: horizontal ones drive the pages, vertical ones (and mouse wheels) are replayed to the list underneath.
/// The release spring runs on a display link rather than a SwiftUI animation so a new touch can catch it mid-flight.
struct SwipePager: NSViewRepresentable {
    var index: Int
    var count: Int
    var position: (CGFloat) -> Void
    var rest: (Int) -> Void   // the pages came to rest on this space
    func makeNSView(context: Context) -> PagerView { PagerView() }
    func updateNSView(_ v: PagerView, context: Context) { v.config = self }

    final class PagerView: NSView {
        var config: SwipePager?
        private enum Axis { case none, undecided, horizontal, vertical }
        private var axis = Axis.none
        private var held: [NSEvent] = []          // the undecided start of a gesture
        private weak var below: NSView?           // where vertical gestures go
        private var passthrough = false
        private var sx: CGFloat = 0, sy: CGFloat = 0
        private var p: CGFloat = 0, v: CGFloat = 0   // position (pages) and velocity (pages/s)
        private var moving = false                   // p is live: a swipe or its spring is in progress
        private var target: CGFloat = 0
        private var samples: [(t: TimeInterval, p: CGFloat)] = []
        private var link: CADisplayLink?
        private var lastFrame: CFTimeInterval = 0

        override func hitTest(_ point: NSPoint) -> NSView? {
            guard !passthrough, NSApp.currentEvent?.type == .scrollWheel else { return nil }
            return super.hitTest(point)
        }

        override func scrollWheel(with e: NSEvent) {
            guard e.hasPreciseScrollingDeltas else { replay(e); return }   // mouse wheel
            if !e.momentumPhase.isEmpty {   // a flick's coast: our spring has its velocity; the list's is the list's
                if axis != .horizontal { replay(e) }
                return
            }
            if e.phase.contains(.mayBegin) || (e.phase.contains(.began) && axis != .undecided) { start(e) }
            switch axis {
            case .none, .vertical: replay(e)
            case .undecided:
                held.append(e)
                sx += e.scrollingDeltaX; sy += e.scrollingDeltaY
                if e.phase.contains(.ended) || e.phase.contains(.cancelled) {   // lifted without moving
                    flush(); axis = .none
                    if moving { release(velocity: 0) }
                } else if hypot(sx, sy) > 4 {
                    if abs(sx) > abs(sy) {
                        axis = .horizontal; held = []
                        if !moving { p = CGFloat(config?.index ?? 0); moving = true }
                        samples = []; drag(sx, e)
                    } else {
                        axis = .vertical; flush()
                        if moving { release(velocity: 0) }
                    }
                }
            case .horizontal:
                if e.phase.contains(.ended) || e.phase.contains(.cancelled) { release(velocity: velocity()) }
                else { drag(e.scrollingDeltaX, e) }
            }
        }

        private func start(_ e: NSEvent) {
            stopLink()   // a touch catches the pages mid-spring
            axis = .undecided; held = []; sx = 0; sy = 0
            passthrough = true
            below = window?.contentView?.hitTest(e.locationInWindow)
            passthrough = false
        }

        private func replay(_ e: NSEvent) { below?.scrollWheel(with: e) }
        private func flush() { held.forEach(replay); held = [] }

        /// `dx` is the event's delta: with natural scrolling it already follows the finger.
        private func drag(_ dx: CGFloat, _ e: NSEvent) {
            guard let c = config, bounds.width > 0 else { return }
            let finger = e.isDirectionInvertedFromDevice ? dx : -dx
            var dp = -finger / bounds.width
            if p + dp < 0 || p + dp > CGFloat(c.count - 1) { dp *= 0.35 }   // rubber band past the ends
            p += dp
            samples.append((e.timestamp, p))
            samples.removeAll { e.timestamp - $0.t > 0.08 }
            c.position(p)
        }

        private func velocity() -> CGFloat {
            guard let f = samples.first, let l = samples.last, l.t > f.t else { return 0 }
            return (l.p - f.p) / CGFloat(l.t - f.t)
        }

        /// Picks the resting space: the nearest one, or the next one in the flick's direction.
        private func release(velocity: CGFloat) {
            guard let c = config else { return }
            axis = .none
            var t = p.rounded()
            if velocity > 0.6 { t = p.rounded(.down) + 1 } else if velocity < -0.6 { t = p.rounded(.up) - 1 }
            target = max(0, min(CGFloat(c.count - 1), t))
            v = velocity
            if link == nil {
                let l = displayLink(target: self, selector: #selector(frame(_:)))
                l.add(to: .main, forMode: .common)
                link = l
                lastFrame = 0
            }
        }

        /// A critically damped spring toward `target`, carrying the finger's velocity.
        @objc private func frame(_ l: CADisplayLink) {
            let dt = lastFrame == 0 ? 1.0 / 120 : min(1.0 / 30, l.timestamp - lastFrame)
            lastFrame = l.timestamp
            let k: CGFloat = 500, damping = 2 * k.squareRoot()   // stiff: at rest in ~0.3 s
            v += (-k * (p - target) - damping * v) * CGFloat(dt)
            p += v * CGFloat(dt)
            // At rest once the rest of the motion is under half a point: the spring's long tail is invisible.
            let w = max(bounds.width, 1)
            if abs(p - target) * w < 0.5 && abs(v) * w < 30 {
                stopLink(); moving = false; p = target
                config?.position(p)
                config?.rest(Int(target))
            } else {
                config?.position(p)
            }
        }

        private func stopLink() { link?.invalidate(); link = nil }
        override func viewDidMoveToWindow() { if window == nil { stopLink() } }
    }
}

struct IdentifiedString: Identifiable { var value: String; var id: String { value } }

func openIn(bundleID: String, _ path: String) {
    guard let app = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID) else { NSSound.beep(); return }
    NSWorkspace.shared.open([URL(fileURLWithPath: path)], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
}

struct WorktreeRow: View {
    var wt: Worktree
    var repo: String
    var model: WorktreeModel
    private var isMain: Bool { wt.path == repo }

    /// A row is a worktree: its folder leads, minus the repo-name prefix the header already shows
    /// ("web-qa" under "web" reads "qa"); the branch checked out in it comes second. The main worktree is the
    /// repo's own folder, so its name would only repeat the header.
    private var title: String {
        let r = (repo as NSString).lastPathComponent
        if isMain { return "Root" }
        for sep in ["-", "_", "."] where wt.name.hasPrefix(r + sep) && wt.name.count > r.count + 1 { return String(wt.name.dropFirst(r.count + 1)) }
        return wt.name
    }

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: isMain ? "folder" : "arrow.triangle.branch")
                .foregroundStyle(isMain ? Color.secondary : Color.accentColor)
            VStack(alignment: .leading, spacing: 1) {
                Text(title).lineLimit(1)
                Text(wt.branch ?? "detached @ \(wt.head.prefix(7))").font(.caption).foregroundStyle(.secondary).lineLimit(1)
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

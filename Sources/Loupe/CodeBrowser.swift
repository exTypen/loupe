import SwiftUI

// What the IDE parts work on: a pull request at its head, or a worktree as it is on disk. Both open files in tabs,
// jump to definitions and references, and step back and forward through the jumps.

struct CodeLocation: Hashable { var path: String; var line: Int }

enum NavResults {
    case definitions(String, [CodeSymbol])
    case references(String, [CodeMatch], noDefinition: Bool)
}

@MainActor protocol CodeBrowser: AnyObject, Observable {
    var tabs: [String] { get set }
    var selectedPath: String? { get set }
    var reveal: DiffReveal? { get set }
    var back: [CodeLocation] { get set }
    var forward: [CodeLocation] { get set }
    var navResults: NavResults? { get set }
    var navBusy: String? { get }
    /// How the change touches `path`, if it does: the tab's dot.
    func changeKind(_ path: String) -> FileChange.Kind?
    /// The file's current text, for the outline.
    func headText(_ path: String) -> String?
    func isLoaded(_ path: String) -> Bool
}

extension CodeBrowser {
    func open(_ path: String, line: Int, record: Bool = true) {
        if record, let cur = selectedPath, cur != path || reveal?.line != line {
            back.append(CodeLocation(path: cur, line: reveal?.line ?? 1)); forward = []
        }
        selectedPath = path
        reveal = DiffReveal(line: line)
    }

    func goBack() {
        guard let to = back.popLast() else { return }
        if let cur = selectedPath { forward.append(CodeLocation(path: cur, line: reveal?.line ?? 1)) }
        open(to.path, line: to.line, record: false)
    }

    func goForward() {
        guard let to = forward.popLast() else { return }
        if let cur = selectedPath { back.append(CodeLocation(path: cur, line: reveal?.line ?? 1)) }
        open(to.path, line: to.line, record: false)
    }

    func closeTab(_ path: String) {
        guard let i = tabs.firstIndex(of: path) else { return }
        tabs.remove(at: i)
        if selectedPath == path { selectedPath = tabs.isEmpty ? nil : tabs[min(i, tabs.count - 1)] }
    }

    func closeOtherTabs(_ path: String) { tabs = tabs.filter { $0 == path }; if selectedPath != path { selectedPath = path } }

    /// The text a whole-file diff ends with: its new side.
    static func newSide(_ f: FileDiff?) -> String? {
        guard let f, !f.isBinary else { return nil }
        return f.lines.filter { $0.kind != .removed && $0.kind != .meta }.map { String($0.text) }.joined(separator: "\n")
    }
}

extension PullRequestModel: CodeBrowser {
    func changeKind(_ path: String) -> FileChange.Kind? { file(path)?.badge }
    func isLoaded(_ path: String) -> Bool { fullFiles[path] != nil }
}

// MARK: - A worktree

/// Changes' Full mode: every file in the worktree as it is on disk, changed ones with their changes against HEAD
/// inline, and code navigation over the worktree.
@MainActor @Observable
final class WorktreeBrowser: CodeBrowser {
    @ObservationIgnored unowned let model: WorktreeModel
    var tabs: [String] = []
    var selectedPath: String? {
        didSet {
            // Whatever opens, from the tree, a jump or a result, opens as a tab next to the current one.
            guard !following, let p = selectedPath, !tabs.contains(p) else { return }
            let at = oldValue.flatMap { tabs.firstIndex(of: $0) }.map { $0 + 1 } ?? tabs.count
            tabs.insert(p, at: min(at, tabs.count))
        }
    }
    var reveal: DiffReveal?
    var back: [CodeLocation] = []
    var forward: [CodeLocation] = []
    var navResults: NavResults?
    var navBusy: String?
    var tree: [String] = []                  // every tracked and untracked (not ignored) path, tree order
    var collapsed: Set<String> = []
    var files: [String: FileDiff] = [:]      // open files, whole; dropped when the worktree changes
    @ObservationIgnored private var treeKey = ""
    @ObservationIgnored private var filesVersion = -1
    @ObservationIgnored private var collapseApplied = false
    @ObservationIgnored private var following = false

    init(model: WorktreeModel) { self.model = model }

    /// Compact mode: keep up with the file the change list shows, without opening tabs for it.
    func follow(_ path: String) {
        guard selectedPath != path else { return }
        following = true; selectedPath = path; following = false
    }

    func change(_ path: String) -> FileChange? {
        model.files.first { $0.path == path && !$0.staged } ?? model.files.first { $0.path == path }
    }
    func changeKind(_ path: String) -> FileChange.Kind? { change(path)?.kind }
    func isLoaded(_ path: String) -> Bool { files[path] != nil }
    func headText(_ path: String) -> String? { Self.newSide(files[path]) }

    /// `git ls-files` plus untracked files. Directories start folded, except where something changed.
    func loadTree() async {
        // Status refreshes, and the change list arriving after the tree (the folding waits for it).
        let key = "\(model.statusVersion) \(model.changeCount)"
        guard treeKey != key else { return }
        guard let out = try? await Git.run(["ls-files", "-co", "--exclude-standard", "-z"], in: model.path) else { return }
        let changed = model.files.map(\.path)
        let (paths, dirs, open) = await Task.detached(priority: .userInitiated) {
            var set = Set(out.split(separator: 0).map { String(decoding: $0, as: UTF8.self) })
            set.formUnion(changed)   // deleted files stay reachable
            let paths = set.sorted(by: FileTree.treeOrder)
            func parents(_ p: String) -> [String] {
                var out: [String] = [], d = (p as NSString).deletingLastPathComponent
                while !d.isEmpty { out.append(d); d = (d as NSString).deletingLastPathComponent }
                return out
            }
            return (paths, Set(paths.flatMap(parents)), Set(changed.flatMap(parents)))
        }.value
        tree = paths; treeKey = key
        // Once the changes are known (the tree can load first): open where something changed, fold the rest.
        if !collapseApplied { collapsed = dirs.subtracting(open); collapseApplied = !changed.isEmpty }
    }

    /// The file as it is: a changed file with its changes against HEAD inline, any other one as plain lines.
    func load(_ path: String) async {
        if filesVersion != model.statusVersion { files = [:]; filesVersion = model.statusVersion }
        guard files[path] == nil else { return }
        let dir = model.path
        var f: FileDiff?
        if let c = change(path) {
            if c.kind == .untracked || c.kind == .added || c.kind == .deleted {
                f = try? await Git.diff(for: c, in: dir)   // all added or all removed: the diff is the file
            } else if let data = try? await Git.run(["diff", "--no-color", "--no-ext-diff", "-M", "-U1000000", "HEAD", "--", path], in: dir) {
                f = await Task.detached { PullRequestModel.wholeFile(DiffParser.parse(data).first) }.value
            }
        }
        if f == nil, let data = FileManager.default.contents(atPath: (dir as NSString).appendingPathComponent(path)) {
            f = await Task.detached { PullRequestModel.plainFile(path, data) }.value
        }
        if let f { files[path] = f }
    }

    /// ⌘-click / Go to Definition / Find References on `word`, over the worktree as it is.
    func navigate(_ word: String, _ action: CodeAction, from path: String) async {
        navBusy = word; defer { navBusy = nil }
        let dir = model.path
        if action == .definition {
            let defs = await CodeNavigator.definitions(of: word, at: nil, in: dir, from: path)
            if defs.count == 1, let d = defs.first { navResults = nil; open(d.path, line: d.line); return }
            if !defs.isEmpty { navResults = .definitions(word, defs); return }
        }
        navResults = .references(word, await CodeNavigator.references(of: word, at: nil, in: dir), noDefinition: action == .definition)
    }
}

// MARK: - Views

/// Full mode's open files. Changed files carry their status colour; the selected tab is the one shown.
struct FileTabBar<B: CodeBrowser>: View {
    var browser: B
    @State private var hovered: String?

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 0) {
                    ForEach(browser.tabs, id: \.self) { path in tab(path).id(path) }
                }
            }
            .onChange(of: browser.selectedPath) { _, p in if let p { withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo(p) } } }
        }
        .frame(height: 30)
        .background(.bar, ignoresSafeAreaEdges: [])   // a bar in the content, not under the toolbar
    }

    private func tab(_ path: String) -> some View {
        let active = browser.selectedPath == path
        let name = (path as NSString).lastPathComponent
        // Same file name twice (mod.rs, index.ts): add the parent directory.
        let twin = browser.tabs.contains { $0 != path && ($0 as NSString).lastPathComponent == name }
        let parent = ((path as NSString).deletingLastPathComponent as NSString).lastPathComponent
        let changed = browser.changeKind(path)
        return HStack(spacing: 5) {
            FileIcon(path: path)
            Text(name).lineLimit(1).foregroundStyle(active ? .primary : .secondary)
            if twin, !parent.isEmpty { Text(parent).font(.caption).foregroundStyle(.tertiary).lineLimit(1) }
            if let c = changed { Circle().fill(c.color).frame(width: 6, height: 6).help("Changed") }
            Button { browser.closeTab(path) } label: { Image(systemName: "xmark").font(.system(size: 9, weight: .semibold)) }
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
        .onTapGesture { browser.selectedPath = path }
        .onHover { hovered = $0 ? path : (hovered == path ? nil : hovered) }
        .help(path)
        .contextMenu {
            Button("Close Tab") { browser.closeTab(path) }
            Button("Close Other Tabs") { browser.closeOtherTabs(path) }.disabled(browser.tabs.count < 2)
            Button("Close All Tabs") { browser.tabs = []; browser.selectedPath = nil }
            Divider()
            Button("Copy Path") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(path, forType: .string) }
        }
    }
}

/// Back / forward through jumps, the file's outline, and what's being looked up.
struct FileNavControls<B: CodeBrowser>: View {
    var browser: B
    var path: String
    @State private var outline: [CodeSymbol] = []

    var body: some View {
        HStack(spacing: 2) {
            if let w = browser.navBusy {
                ProgressView().controlSize(.mini)
                Text(w).font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(1)
            }
            Button { browser.goBack() } label: { Image(systemName: "chevron.left") }
                .disabled(browser.back.isEmpty).help("Back").shortcut(.navigateBack)
            Button { browser.goForward() } label: { Image(systemName: "chevron.right") }
                .disabled(browser.forward.isEmpty).help("Forward").shortcut(.navigateForward)
            if !outline.isEmpty {
                Menu {
                    ForEach(outline, id: \.self) { s in
                        Button { browser.open(path, line: s.line) } label: { Text("\(s.name)  \(s.kind.rawValue) · \(s.line)") }
                    }
                } label: { Image(systemName: "list.bullet.indent") }
                .menuStyle(.borderlessButton).fixedSize()
                .help("Outline: jump to a definition in this file")
            }
        }
        .buttonStyle(.borderless)
        .task(id: [path, browser.isLoaded(path) ? "1" : "0"]) {
            guard let text = browser.headText(path) else { outline = []; return }
            let p = path
            outline = await Task.detached(priority: .utility) { CodeIndex.outline(text, path: p) }.value
        }
    }
}

/// Under the file: the definitions to choose from, or every reference.
struct NavResultsPanel<B: CodeBrowser>: View {
    var results: NavResults
    var browser: B

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
                Button { browser.navResults = nil } label: { Image(systemName: "xmark") }.buttonStyle(.borderless).keyboardShortcut(.cancelAction)
            }
            .font(.callout)
            .padding(.horizontal, 12).padding(.vertical, 6)
            .background(.bar, ignoresSafeAreaEdges: [])
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
        Button { browser.open(path, line: line) } label: {
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

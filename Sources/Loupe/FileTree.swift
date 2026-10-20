import SwiftUI

/// Flattened, collapsible directory tree over file paths. Flattening (instead of nested
/// DisclosureGroups) keeps a 5000-file PR a single lazy List.
struct TreeRow: Identifiable, Hashable {
    var id: String          // file path, or "dir:" + dir path
    var name: String        // display name; compressed chains read "a/b/c"
    var depth: Int
    var dirPath: String?    // non-nil → directory row
    var filePath: String?
}

enum FileTree {
    /// Directories before files at every level, then by name — the order the tree displays in.
    static func treeOrder(_ a: String, _ b: String) -> Bool {
        let x = a.split(separator: "/"), y = b.split(separator: "/")
        for i in 0..<min(x.count, y.count) where x[i] != y[i] {
            let xDir = i < x.count - 1, yDir = i < y.count - 1
            if xDir != yDir { return xDir }
            return x[i].localizedStandardCompare(y[i]) == .orderedAscending
        }
        return x.count < y.count
    }

    private final class Node {
        var children: [String: Node] = [:]
        var order: [String] = []
        var file: String?
        func child(_ name: String) -> Node {
            if let c = children[name] { return c }
            let c = Node(); children[name] = c; order.append(name); return c
        }
    }

    /// `paths` must already be in `treeOrder`.
    static func rows(_ paths: [String], collapsed: Set<String>) -> [TreeRow] {
        let root = Node()
        for p in paths {
            var n = root
            for comp in p.split(separator: "/").dropLast() { n = n.child(String(comp)) }
            let leaf = n.child(String(p.split(separator: "/").last ?? Substring(p)))
            leaf.file = p
        }
        var out: [TreeRow] = []
        func walk(_ n: Node, prefix: String, depth: Int) {
            for name in n.order {
                var c = n.children[name]!
                if let f = c.file { out.append(TreeRow(id: f, name: name, depth: depth, filePath: f)); continue }
                var label = name, path = prefix + name
                // Compress single-directory chains: central_api → src becomes "central_api/src".
                while c.order.count == 1, let only = c.children[c.order[0]], only.file == nil {
                    label += "/" + c.order[0]; path += "/" + c.order[0]; c = only
                }
                out.append(TreeRow(id: "dir:" + path, name: label, depth: depth, dirPath: path))
                if !collapsed.contains(path) { walk(c, prefix: path + "/", depth: depth + 1) }
            }
        }
        walk(root, prefix: "", depth: 0)
        return out
    }
}

struct FileTreeList<FileContent: View, DirAccessory: View>: View {
    var paths: [String]                      // tree-ordered
    @Binding var selection: String?
    @Binding var collapsed: Set<String>
    var dirMenu: ((String) -> (title: String, action: () -> Void))? = nil
    @ViewBuilder var file: (String) -> FileContent
    @ViewBuilder var dirAccessory: (String) -> DirAccessory

    var body: some View {
        let rows = FileTree.rows(paths, collapsed: collapsed)
        List(selection: Binding(get: { selection }, set: { v in
            // Clicking a directory toggles it rather than selecting it.
            if let v, v.hasPrefix("dir:") { toggle(String(v.dropFirst(4))) } else { selection = v }
        })) {
            ForEach(rows) { r in
                HStack(spacing: 4) {
                    if let dir = r.dirPath {
                        Image(systemName: "chevron.right")
                            .font(.caption2.weight(.semibold))
                            .rotationEffect(.degrees(collapsed.contains(dir) ? 0 : 90))
                            .foregroundStyle(.secondary)
                            .frame(width: 12)
                        Image(systemName: collapsed.contains(dir) ? "folder" : "folder.fill").foregroundStyle(.secondary)
                        Text(r.name).lineLimit(1).truncationMode(.head).foregroundStyle(.secondary)
                        Spacer(minLength: 4)
                        dirAccessory(dir)
                    } else if let f = r.filePath {
                        Color.clear.frame(width: 12)
                        file(f)
                    }
                }
                .padding(.leading, CGFloat(r.depth) * 10)
                .tag(r.id)
                .contextMenu {
                    if let dir = r.dirPath {
                        Button(collapsed.contains(dir) ? "Expand" : "Collapse") { toggle(dir) }
                        if let item = dirMenu?(dir) { Button(item.title, action: item.action) }
                        Divider()
                    }
                    Button("Expand All") { collapsed = [] }
                    Button("Collapse All") { collapsed = Set(allDirs) }
                }
            }
        }
        .listStyle(.inset)
    }

    private var allDirs: [String] {
        Set(paths.flatMap { p in
            let c = p.split(separator: "/").dropLast()
            return c.indices.map { c[...$0].joined(separator: "/") }
        }).sorted()
    }

    private func toggle(_ dir: String) {
        if collapsed.contains(dir) { collapsed.remove(dir) } else { collapsed.insert(dir) }
    }
}

/// File row used inside the tree: name only (the directory is already shown by the tree).
struct TreeFileRow: View {
    var path: String
    var badge: FileChange.Kind
    var additions: Int
    var deletions: Int
    var isBinary = false
    var dimmed = false

    var body: some View {
        HStack(spacing: 6) {
            FileIcon(path: path)
            Text((path as NSString).lastPathComponent).lineLimit(1).truncationMode(.middle)
            Spacer(minLength: 4)
            Group {
                if isBinary { Text("binary").foregroundStyle(.tertiary) }
                else {
                    Text(verbatim: "+\(String(additions))").foregroundStyle(.green)
                    Text(verbatim: "−\(String(deletions))").foregroundStyle(.red)
                }
            }
            .font(.caption.monospacedDigit())
            .fixedSize()   // numbers never wrap; the name truncates instead
            StatusIcon(kind: badge)
        }
        .opacity(dimmed ? 0.5 : 1)
        .help(path)
    }
}

/// Two panes with a draggable divider whose leading width is persisted. HSplitView ignores idealWidth on macOS
/// and opens every pane at its minimum, so users had to re-widen it each time.
struct ResizableSplit<Leading: View, Trailing: View>: View {
    @AppStorage private var width: Double
    private let defaultWidth: Double, minWidth: Double, maxWidth: Double
    private let leading: Leading, trailing: Trailing
    @State private var dragStart: Double?

    init(key: String, minWidth: Double, maxWidth: Double, defaultFraction: Double = 0.9,
         @ViewBuilder leading: () -> Leading, @ViewBuilder trailing: () -> Trailing) {
        let def = (maxWidth * defaultFraction).rounded()
        _width = AppStorage(wrappedValue: def, key)
        defaultWidth = def; self.minWidth = minWidth; self.maxWidth = maxWidth
        self.leading = leading(); self.trailing = trailing()
    }

    var body: some View {
        HStack(spacing: 0) {
            leading.frame(width: CGFloat(min(max(width, minWidth), maxWidth)))
            Rectangle().fill(Color(nsColor: .separatorColor)).frame(width: 1)
                .overlay {
                    Color.clear.frame(width: 9).contentShape(Rectangle())
                        .onHover { inside in if inside { NSCursor.resizeLeftRight.push() } else { NSCursor.pop() } }
                        .gesture(DragGesture(minimumDistance: 1, coordinateSpace: .global)
                            .onChanged { v in
                                let start = dragStart ?? width
                                dragStart = start
                                width = min(max(start + v.translation.width, minWidth), maxWidth)
                            }
                            .onEnded { _ in dragStart = nil })
                        .onTapGesture(count: 2) { width = defaultWidth }   // double-click: back to default
                }
            trailing.frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

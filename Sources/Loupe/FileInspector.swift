import SwiftUI
import AppKit

/// A file in a worktree, optionally at a revision: what the Blame and History panel shows.
struct FileTarget: Hashable {
    var worktree: String
    var path: String
    var rev: String? = nil
}

// MARK: - Blame

struct BlameCommit: Hashable { var sha: String; var author: String; var date: Date; var summary: String }
struct BlameLine { var sha: String; var text: String }

extension Git {
    /// `git blame --porcelain`: each line comes with its commit; a commit's details only on its first appearance.
    static func blame(_ path: String, rev: String?, in dir: String) async throws -> (lines: [BlameLine], commits: [String: BlameCommit]) {
        let out = try await string(["blame", "--porcelain"] + (rev.map { [$0] } ?? []) + ["--", path], in: dir)
        var lines: [BlameLine] = [], commits: [String: BlameCommit] = [:]
        var sha = ""
        for l in out.split(separator: "\n", omittingEmptySubsequences: false) {
            if l.hasPrefix("\t") { lines.append(BlameLine(sha: sha, text: String(l.dropFirst()))); continue }
            let f = l.split(separator: " ", maxSplits: 1)
            guard let key = f.first else { continue }
            let value = f.count > 1 ? String(f[1]) : ""
            if key.count == 40 || key.count == 64, key.allSatisfy(\.isHexDigit) {
                sha = String(key)
                if commits[sha] == nil { commits[sha] = BlameCommit(sha: sha, author: "", date: .distantPast, summary: "") }
            } else if key == "author" { commits[sha]?.author = value }
            else if key == "author-time" { commits[sha]?.date = Date(timeIntervalSince1970: TimeInterval(value) ?? 0) }
            else if key == "summary" { commits[sha]?.summary = value }
        }
        return (lines, commits)
    }

    /// Commits touching `path`, following renames, with the file's path in each.
    static func fileLog(_ path: String, in dir: String) async throws -> [(commit: Commit, path: String)] {
        let out = try await string(["log", "--follow", "-M", "-n", "1000", "--format=%x1e" + logFormat.replacingOccurrences(of: "%x1e", with: ""),
                                    "--decorate=full", "--name-status", "--", path], in: dir)
        return out.split(separator: "\u{1e}").compactMap { rec in
            let lines = rec.split(separator: "\n")
            guard let head = lines.first, let c = parseLog(Data(head.utf8)).first else { return nil }
            let name = lines.dropFirst().last.map { String($0.split(separator: "\t").last ?? "") } ?? path
            return (c, name.isEmpty ? path : name)
        }
    }
}

// MARK: - Blame and History, in the diff

/// Blame for a diff's new side: the commit of each line (index 0 is line 1). The diff draws it beside the line numbers.
struct DiffBlame: Equatable {
    var id = UUID()
    var shas: [String]
    var commits: [String: BlameCommit]
    static func == (a: Self, b: Self) -> Bool { a.id == b.id }
}

extension EnvironmentValues {
    @Entry var diffBlame: DiffBlame? = nil
}

/// Blame or History for the file showing. At most one is on.
enum FilePanel: String { case none = "", blame, history }

/// The diff, with what the Blame and History buttons turn on. Blame annotates the diff itself; History lists the file's
/// commits on the left, and the one picked shows its change to the file in the diff's place.
struct WithFilePanel<Content: View>: View {
    var file: FileTarget?
    var version: AnyHashable? = nil   // changes when the diff does, so blame follows edits
    @ViewBuilder var content: Content
    @AppStorage("filePanel") private var panel = FilePanel.none
    @AppStorage("filePanelWidth") private var width = 340.0
    @AppStorage("prFullMode") private var fullMode = false
    @AppStorage("diffSide") private var side = DiffSide.diff
    @State private var dragStart: Double?
    @State private var blame: DiffBlame?
    @State private var picked: FileCommit?
    @State private var pickedDiff: FileDiff?

    var body: some View {
        HStack(spacing: 0) {
            // On the left, like blame: the commits, then the code they changed.
            if let file, panel == .history {
                FileHistoryList(target: file, picked: $picked, close: { panel = .none }).frame(width: width).id(file)
                Rectangle().fill(Color(nsColor: .separatorColor)).frame(width: 1)
                    .overlay {
                        Color.clear.frame(width: 9).contentShape(Rectangle())
                            .onHover { inside in if inside { NSCursor.resizeLeftRight.push() } else { NSCursor.pop() } }
                            .gesture(DragGesture(minimumDistance: 1, coordinateSpace: .global)
                                .onChanged { v in
                                    let start = dragStart ?? width
                                    dragStart = start
                                    width = min(max(start + v.translation.width, 260), 700)
                                }
                                .onEnded { _ in dragStart = nil })
                            .onTapGesture(count: 2) { width = 340 }   // double-click: back to default
                    }
            }
            Group {
                if let p = picked, panel == .history { commitDiff(p) }
                else { content.environment(\.diffBlame, panel == .blame ? blame : nil) }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .onChange(of: file) { picked = nil }
        .task(id: [AnyHashable(file), AnyHashable(panel == .blame), version]) {
            guard panel == .blame, let file else { blame = nil; return }
            guard let r = try? await Git.blame(file.path, rev: file.rev, in: file.worktree) else { blame = nil; return }
            blame = DiffBlame(shas: r.lines.map(\.sha), commits: r.commits)
        }
        .task(id: [AnyHashable(picked?.commit.sha), AnyHashable(fullMode || side != .diff)]) { await loadPicked() }
    }

    /// The picked commit's change to the file, where the diff was, under a bar that says so and goes back.
    @ViewBuilder private func commitDiff(_ p: FileCommit) -> some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "clock.arrow.circlepath").foregroundStyle(.secondary)
                Text(p.commit.short).font(.callout.monospaced()).foregroundStyle(.secondary)
                Text(p.commit.subject).lineLimit(1).truncationMode(.tail)
                Text("\(p.commit.author) · \(p.commit.date.formatted(.relative(presentation: .named)))").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                Spacer(minLength: 8)
                Button("Back to the Change") { picked = nil }.controlSize(.small)
            }
            .padding(.horizontal, 12).padding(.vertical, 6)
            .background(Color.accentColor.opacity(0.08))
            Divider()
            if let d = pickedDiff, d.path == p.path || d.oldPath == p.path || d.newPath == p.path {
                if d.isBinary { ContentUnavailableView("Binary File", systemImage: "doc.zipper") }
                else { DiffTableView(file: d, changeRuler: fullMode) }
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }

    private func loadPicked() async {
        guard let p = picked, let file else { pickedDiff = nil; return }
        let data = try? await Git.run(["show", "--format=", "--no-color", "--no-ext-diff", "-M", "--diff-merges=first-parent"]
                                      + (fullMode || side != .diff ? ["-U1000000"] : []) + [p.commit.sha, "--", p.path], in: file.worktree)
        guard picked?.commit.sha == p.commit.sha else { return }
        let full = fullMode || side != .diff
        pickedDiff = await Task.detached {
            let d = data.flatMap { DiffParser.parse($0).first }
            return full ? PullRequestModel.wholeFile(d) : d
        }.value
    }
}

/// A commit that touched a file, with the file's path in it (renames are followed).
struct FileCommit: Equatable { var commit: Commit; var path: String }

/// The History panel: the file's commits, newest first. Picking one shows its change in the diff.
private struct FileHistoryList: View {
    var target: FileTarget
    @Binding var picked: FileCommit?
    var close: () -> Void
    @State private var entries: [FileCommit] = []
    @State private var loading = true

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                Text("History").font(.callout.weight(.semibold))
                Text((target.path as NSString).lastPathComponent).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                Spacer()
                Button(action: close) { Image(systemName: "xmark") }.buttonStyle(.borderless).help("Close")
            }
            .padding(.horizontal, 12).padding(.vertical, 7)
            .background(.bar, ignoresSafeAreaEdges: [])
            Divider()
            List(selection: Binding(get: { picked?.commit.sha }, set: { sha in picked = entries.first { $0.commit.sha == sha } })) {
                ForEach(entries, id: \.commit.sha) { e in
                    VStack(alignment: .leading, spacing: 2) {
                        CommitRow(commit: e.commit, isHead: false)
                        if e.path != target.path {
                            Label(e.path, systemImage: "arrow.turn.down.right").font(.caption2).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
                        }
                    }
                    .tag(e.commit.sha)
                }
            }
            .listStyle(.inset)
            .overlay {
                if loading { ProgressView() }
                else if entries.isEmpty { ContentUnavailableView("No History", systemImage: "clock") }
            }
        }
        .task {
            entries = ((try? await Git.fileLog(target.path, in: target.worktree)) ?? []).map { FileCommit(commit: $0.commit, path: $0.path) }
            loading = false
        }
    }
}

// MARK: - Highlighting snippets

enum SnippetHighlight {
    nonisolated static func lines(_ texts: [String], path: String) -> [AttributedString] {
        guard let lang = Language.forPath(path) else { return texts.map { AttributedString($0) } }
        var state = Syntax.State()
        return texts.map { t in
            var a = AttributedString(t)
            let chars = Array(t.utf16)
            for span in Syntax.tokenize(t, lang, &state) {
                let lo = Int(span.start), hi = min(chars.count, lo + Int(span.length))
                guard lo < hi, let r = Range(NSRange(location: lo, length: hi - lo), in: t), let ar = Range(r, in: a) else { continue }
                a[ar].foregroundColor = Color(nsColor: Syntax.color(span.kind))
            }
            return a
        }
    }
}

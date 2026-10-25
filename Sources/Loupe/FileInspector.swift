import SwiftUI
import AppKit

/// A file in a worktree, optionally at a revision: what a File window shows.
struct FileTarget: Codable, Hashable {
    var worktree: String
    var path: String
    var rev: String? = nil
    var mode = "blame"
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

// MARK: - Window

struct FileInspector: View {
    @State var target: FileTarget
    @AppStorage("layout") private var layout = DiffLayout.unified

    var body: some View {
        Group {
            if target.mode == "history" { FileHistoryView(target: target, layout: layout) }
            else { BlameView(target: $target) }
        }
        .navigationTitle((target.path as NSString).lastPathComponent)
        .navigationSubtitle(subtitle)
        .toolbar {
            ToolbarItem(placement: .principal) {
                Picker("Mode", selection: $target.mode) {
                    Text("Blame").tag("blame")
                    Text("History").tag("history")
                }
                .pickerStyle(.segmented)
            }
        }
        .frame(minWidth: 700, minHeight: 400)
    }

    private var subtitle: String {
        let dir = (target.path as NSString).deletingLastPathComponent
        let base = ((target.worktree as NSString).lastPathComponent as NSString).appendingPathComponent(dir)
        return target.mode == "blame" && target.rev != nil ? "\(base) · at \(String(target.rev!.prefix(8)))" : base
    }
}

/// The file with, beside each run of lines, the commit that last changed them.
struct BlameView: View {
    @Binding var target: FileTarget
    @State private var lines: [BlameLine] = []
    @State private var oddRun: [Bool] = []   // per line: in an odd-numbered commit run (for the block shade)
    @State private var commits: [String: BlameCommit] = [:]
    @State private var highlighted: [AttributedString] = []
    @State private var error: String?
    @State private var loading = true
    @State private var picked: String?   // sha whose popover is open

    var body: some View {
        Group {
            if let error {
                ContentUnavailableView("Can't Blame This File", systemImage: "exclamationmark.triangle", description: Text(error))
            } else if loading && lines.isEmpty {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                // A two-axis ScrollView centres content smaller than itself; pin it to the top left instead.
                GeometryReader { geo in
                    ScrollView([.vertical, .horizontal]) {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            ForEach(lines.indices, id: \.self) { i in row(i) }
                        }
                        .padding(.vertical, 4)
                        .frame(minWidth: geo.size.width, minHeight: geo.size.height, alignment: .topLeading)
                    }
                }
                .background(Color(nsColor: Theme.background))
            }
        }
        .task(id: [target.path, target.rev ?? ""]) { await load() }
    }

    private func row(_ i: Int) -> some View {
        let l = lines[i]
        let first = i == 0 || lines[i - 1].sha != l.sha
        let c = commits[l.sha]
        let uncommitted = l.sha.allSatisfy { $0 == "0" }
        let button = Button { picked = l.sha } label: {
                HStack(spacing: 8) {
                    if first {
                        Text(uncommitted ? "Not committed" : String(l.sha.prefix(7))).font(.caption.monospaced()).foregroundStyle(.secondary)
                        Text(uncommitted ? "" : c?.author ?? "").font(.caption).lineLimit(1)
                        Spacer(minLength: 4)
                        if let d = c?.date, !uncommitted { Text(d, format: .relative(presentation: .numeric)).font(.caption).foregroundStyle(.tertiary).lineLimit(1) }
                    } else { Spacer() }
                }
                .padding(.horizontal, 8)
                .frame(width: 250, height: 18)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(uncommitted)
        return HStack(spacing: 0) {
            // Only a run's first row can show the card.
            Group {
                if first { button.popover(isPresented: Binding(get: { picked == l.sha }, set: { if !$0 { picked = nil } })) { commitCard(l.sha) } }
                else { button }
            }
            .background(blockShade(i))
            Text(String(i + 1)).font(.caption.monospacedDigit()).foregroundStyle(Color(nsColor: Theme.lineNumber))
                .frame(width: 46, alignment: .trailing).padding(.trailing, 10)
            Text(i < highlighted.count ? highlighted[i] : AttributedString(l.text))
                .font(.editor(EditorSettings.shared.fontSize))
                .foregroundStyle(Color(nsColor: Theme.foreground))
                .fixedSize()
                .textSelection(.enabled)
            Spacer(minLength: 0)
        }
        .overlay(alignment: .top) { if first && i > 0 { Divider() } }
    }

    /// Alternate a faint shade per commit run, so runs read as blocks.
    private func blockShade(_ i: Int) -> Color {
        i < oddRun.count && oddRun[i] ? Color.primary.opacity(0.07) : Color.primary.opacity(0.03)
    }

    nonisolated static func oddRuns(_ lines: [BlameLine]) -> [Bool] {
        var odd = false
        return lines.indices.map { i in
            if i > 0, lines[i].sha != lines[i - 1].sha { odd.toggle() }
            return odd
        }
    }

    private func commitCard(_ sha: String) -> some View {
        let c = commits[sha]
        return VStack(alignment: .leading, spacing: 8) {
            Text(c?.summary ?? "").font(.headline).textSelection(.enabled).frame(maxWidth: 360, alignment: .leading)
            HStack(spacing: 8) {
                Text(String(sha.prefix(10))).font(.callout.monospaced()).textSelection(.enabled)
                Text(c?.author ?? "")
                if let d = c?.date { Text(d.formatted(date: .abbreviated, time: .shortened)).foregroundStyle(.secondary) }
            }
            .font(.callout)
            HStack {
                Button("Copy SHA") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(sha, forType: .string) }
                Button("Blame Before This Commit") { picked = nil; target.rev = sha + "^" }
                    .help("See who wrote these lines before this commit changed them")
            }
            .controlSize(.small)
        }
        .padding(14)
    }

    private func load() async {
        loading = true; defer { loading = false }
        do {
            let r = try await Git.blame(target.path, rev: target.rev, in: target.worktree)
            lines = r.lines; oddRun = Self.oddRuns(r.lines); commits = r.commits; error = nil
            let texts = r.lines.map(\.text), path = target.path
            highlighted = await Task.detached(priority: .userInitiated) { Self.highlight(texts, path: path) }.value
        } catch { self.error = error.localizedDescription; lines = []; oddRun = [] }
    }

    nonisolated static func highlight(_ texts: [String], path: String) -> [AttributedString] {
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

/// Every commit that touched the file, and the file's diff in the one picked.
struct FileHistoryView: View {
    var target: FileTarget
    var layout: DiffLayout
    @State private var entries: [(commit: Commit, path: String)] = []
    @State private var selected: String?
    @State private var diff: FileDiff?
    @State private var loading = true

    var body: some View {
        ResizableSplit(key: "fileHistoryListWidth", minWidth: 260, maxWidth: 440, defaultFraction: 340/440) {
            List(selection: Binding(get: { selected }, set: { selected = $0; loadDiff() })) {
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
            .frame(maxHeight: .infinity)
            .overlay { if entries.isEmpty && !loading { ContentUnavailableView("No History", systemImage: "clock") } }
        } trailing: {

            Group {
                if let diff {
                    VStack(spacing: 0) {
                        DiffHeader(path: diff.path, oldPath: diff.oldPath != diff.newPath ? diff.oldPath : nil, additions: diff.additions, deletions: diff.deletions) { EmptyView() }
                        Divider()
                        if diff.isBinary { ContentUnavailableView("Binary File", systemImage: "doc.zipper") }
                        else { DiffTableView(file: diff, layout: layout) }
                    }
                } else if selected != nil { ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity) }
                else { ContentUnavailableView("Select a Commit", systemImage: "clock.arrow.circlepath") }
            }
            .frame(minWidth: 380, maxWidth: .infinity, maxHeight: .infinity)
        }
        .task {
            entries = (try? await Git.fileLog(target.path, in: target.worktree)) ?? []
            loading = false
            selected = entries.first?.commit.sha
            loadDiff()
        }
    }

    private func loadDiff() {
        guard let sha = selected, let e = entries.first(where: { $0.commit.sha == sha }) else { diff = nil; return }
        diff = nil
        Task {
            let data = try? await Git.run(["show", "--format=", "--no-color", "--no-ext-diff", "-M", "--diff-merges=first-parent", sha, "--", e.path], in: target.worktree)
            if selected == sha { diff = data.flatMap { DiffParser.parse($0).first } }
        }
    }
}

import SwiftUI

// MARK: - Submodules: status, diff, actions

/// The `<sub>` field of `status --porcelain=v2`: `S<c><m><u>` for a submodule, `N...` for anything else.
struct SubmoduleStatus: Hashable {
    var commitChanged: Bool     // checked out at another commit than the one recorded
    var modified: Bool          // tracked changes inside it
    var untracked: Bool         // untracked files inside it

    init(commitChanged: Bool = false, modified: Bool = false, untracked: Bool = false) {
        self.commitChanged = commitChanged; self.modified = modified; self.untracked = untracked
    }

    init?(_ field: Substring) {
        let c = Array(field)
        guard c.count == 4, c[0] == "S" else { return nil }
        self.init(commitChanged: c[1] == "C", modified: c[2] == "M", untracked: c[3] == "U")
    }
}

/// `git diff --submodule=log` for one submodule: the commit range and the commits in it.
struct SubmoduleDiff: Equatable {
    struct Commit: Equatable { var added: Bool; var subject: String }   // `>` reached by the new commit, `<` only by the old (a rewind)
    var old: String?            // abbreviated; nil for a new submodule
    var new: String?            // nil for a deleted one
    var note: String?           // "new submodule", "rewind", "commits not present", ...
    var commits: [Commit] = []
    var modifiedContent = false
    var untrackedContent = false

    static func parse(_ text: String) -> SubmoduleDiff {
        var d = SubmoduleDiff()
        for line in text.split(separator: "\n") {
            if line.hasPrefix("  > ") { d.commits.append(Commit(added: true, subject: String(line.dropFirst(4)))); continue }
            if line.hasPrefix("  < ") { d.commits.append(Commit(added: false, subject: String(line.dropFirst(4)))); continue }
            guard line.hasPrefix("Submodule ") else { continue }
            if line.hasSuffix(" contains modified content") { d.modifiedContent = true; continue }
            if line.hasSuffix(" contains untracked content") { d.untrackedContent = true; continue }
            // "Submodule <path> <a>..<b>:" or "... <a>...<b> (rewind):"; the path may hold spaces, the range never does.
            var rest = line.hasSuffix(":") ? line.dropLast() : line
            if rest.hasSuffix(")"), let open = rest.range(of: " (", options: .backwards) {
                d.note = String(rest[open.upperBound...].dropLast())
                rest = rest[..<open.lowerBound]
            }
            guard let range = rest.split(separator: " ").last else { continue }
            let ends = range.contains("...") ? range.components(separatedBy: "...") : range.components(separatedBy: "..")
            guard ends.count == 2 else { continue }
            func sha(_ s: String) -> String? { s.allSatisfy { $0 == "0" } ? nil : s }
            d.old = sha(ends[0]); d.new = sha(ends[1])
        }
        return d
    }
}

enum Submodules {
    /// `git config -f .gitmodules --get-regexp '^submodule\..*\.path$'`: "submodule.<name>.path <path>" per line.
    static func paths(fromConfig out: String) -> [String] {
        out.split(separator: "\n").compactMap { line in line.range(of: ".path ").map { String(line[$0.upperBound...]) } }
    }
}

/// Per worktree: submodules listed in .gitmodules that were never checked out.
@MainActor @Observable
final class SubmoduleModel {
    let path: String
    var uninitialized: [String] = []

    init(path: String) { self.path = path }

    /// On every status refresh, but only a repo with a .gitmodules pays for it: one git call and a stat per submodule.
    func refresh() async {
        let fm = FileManager.default
        guard fm.fileExists(atPath: (path as NSString).appendingPathComponent(".gitmodules")) else {
            if !uninitialized.isEmpty { uninitialized = [] }
            return
        }
        let out = String(decoding: (try? await Git.run(["config", "-f", ".gitmodules", "--get-regexp", "^submodule\\..*\\.path$"], in: path, allowFailure: true)) ?? Data(), as: UTF8.self)
        // A checked-out submodule has a .git (a "gitdir:" file pointing into the superproject's .git/modules).
        let u = Submodules.paths(fromConfig: out).filter { !fm.fileExists(atPath: (path as NSString).appendingPathComponent($0) + "/.git") }
        if u != uninitialized { uninitialized = u }
    }
}

extension WorktreeModel {
    func initSubmodules() async {
        await perform("Initializing submodules") { try await Git.run(["submodule", "update", "--init", "--recursive"], in: path, env: gitAuth) }
    }

    /// Checks the submodules out at the commits this repository records for them.
    func updateSubmodules(_ paths: [String]) async {
        await perform("Updating submodules") { try await Git.run(["submodule", "update", "--init", "--recursive", "--"] + paths, in: path, env: gitAuth) }
    }

    /// The submodule as a repository of its own in the sidebar, as if opened with ⌘O.
    func openSubmodule(_ sub: String) async {
        await app.addRepo(URL(fileURLWithPath: (path as NSString).appendingPathComponent(sub)))
    }

    func isCheckedOut(_ sub: String) -> Bool {
        FileManager.default.fileExists(atPath: (path as NSString).appendingPathComponent(sub) + "/.git")
    }
}

// MARK: - Views

struct SubmoduleMenuItems: View {
    var model: WorktreeModel
    var files: [FileChange]

    var body: some View {
        let subs = Array(Set(files.filter { $0.submodule != nil }.map(\.path))).sorted()
        if !subs.isEmpty {
            Divider()
            if subs.count == 1, let s = subs.first {
                Button("Open Submodule") { Task { await model.openSubmodule(s) } }.disabled(!model.isCheckedOut(s))
            }
            Button("Update to Recorded Commit\(subs.count == 1 ? "" : "s")") { Task { await model.updateSubmodules(subs) } }
        }
    }
}

/// Submodules listed in .gitmodules but never checked out: offer to do it.
struct SubmoduleBanner: View {
    var model: WorktreeModel
    @State private var hidden = false

    var body: some View {
        let n = model.submodules.uninitialized.count
        if n > 0, !hidden {
            HStack(spacing: 10) {
                Image(systemName: "shippingbox").foregroundStyle(.blue)
                VStack(alignment: .leading, spacing: 1) {
                    Text(n == 1 ? "A submodule isn't initialized" : "\(n) submodules aren't initialized").fontWeight(.medium)
                    Text(model.submodules.uninitialized.prefix(3).joined(separator: ", ") + (n > 3 ? " and \(n - 3) more" : ""))
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.tail)
                }
                Spacer()
                Button("Not Now") { hidden = true }
                Button("Initialize") { Task { await model.initSubmodules() } }
                    .buttonStyle(.borderedProminent)
                    .disabled(model.busy != nil)
                    .help("git submodule update --init --recursive")
            }
            .controlSize(.small)
            .padding(.horizontal, 12).padding(.vertical, 8)
            .background(Color.blue.opacity(0.08))
            Divider()
        }
    }
}

/// A submodule's change instead of a "Subproject commit" diff: the commits between the recorded and the checked-out
/// commit, and whether there's work inside it.
struct SubmodulePane: View {
    var model: WorktreeModel
    var file: FileChange
    @State private var diff: SubmoduleDiff?

    var body: some View {
        VStack(spacing: 0) {
            DiffHeader(path: file.path, additions: 0, deletions: 0, showCounts: false) {
                Button("Open Submodule") { Task { await model.openSubmodule(file.path) } }.disabled(!model.isCheckedOut(file.path))
                if file.staged { Button("Unstage") { Task { await model.unstage([file]) } } }
                else {
                    if file.submodule?.commitChanged == true {
                        Button("Update to Recorded Commit") { Task { await model.updateSubmodules([file.path]) } }
                            .help("Check the submodule out at the commit this repository records (git submodule update)")
                    }
                    Button("Stage Pointer") { Task { await model.stage([file]) } }.buttonStyle(.borderedProminent)
                        .disabled(file.submodule?.commitChanged != true)
                        .help("Record the submodule's checked-out commit in this repository")
                }
            }
            Divider()
            if let d = diff { content(d) } else { ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity) }
        }
        .task(id: "\(file.id) \(model.statusVersion)") {
            let out = try? await Git.string(["diff", "--no-color", "--no-ext-diff", "--submodule=log"] + (file.staged ? ["--cached"] : []) + ["--", file.path], in: model.path)
            let d = SubmoduleDiff.parse(out ?? "")
            if d != diff { diff = d }
        }
    }

    @ViewBuilder private func content(_ d: SubmoduleDiff) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                Label("Submodule", systemImage: "shippingbox").font(.headline)
                if d.old != nil || d.new != nil {
                    HStack(spacing: 8) {
                        Text(d.old ?? "none").font(.body.monospaced())
                        Image(systemName: "arrow.right").foregroundStyle(.secondary)
                        Text(d.new ?? "none").font(.body.monospaced())
                        if let note = d.note { Text(note).font(.caption).foregroundStyle(.secondary) }
                    }
                    .textSelection(.enabled)
                    Text(file.staged ? "The recorded commit, before and after this change" : "The recorded commit, and the one checked out in the submodule")
                        .font(.caption).foregroundStyle(.secondary)
                }
                if !d.commits.isEmpty {
                    let back = d.commits.filter { !$0.added }.count, fwd = d.commits.count - back
                    Text([fwd > 0 ? "\(fwd) new commit\(fwd == 1 ? "" : "s")" : nil, back > 0 ? "\(back) commit\(back == 1 ? "" : "s") left behind" : nil]
                        .compactMap { $0 }.joined(separator: ", ")).font(.subheadline.weight(.medium)).padding(.top, 4)
                    VStack(alignment: .leading, spacing: 4) {
                        ForEach(Array(d.commits.enumerated()), id: \.offset) { _, c in
                            HStack(spacing: 6) {
                                Image(systemName: c.added ? "plus.circle" : "arrow.uturn.backward.circle").foregroundStyle(c.added ? .green : .orange)
                                Text(c.subject).lineLimit(1).truncationMode(.tail).textSelection(.enabled)
                            }
                        }
                    }
                }
                if d.modifiedContent || d.untrackedContent {
                    Label(d.modifiedContent && d.untrackedContent ? "Contains modified and untracked files"
                          : d.modifiedContent ? "Contains modified files" : "Contains untracked files", systemImage: "pencil.circle")
                        .foregroundStyle(.secondary).padding(.top, 4)
                    Text("Open the submodule to commit or discard them.").font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

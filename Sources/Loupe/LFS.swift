import SwiftUI
import AppKit

// MARK: - Git LFS: detection, pointers, locks

/// What a pointer file in the repository stands for: the object's sha256 and size.
struct LFSPointer: Equatable {
    var oid: String
    var size: Int64

    init(oid: String, size: Int64) { self.oid = oid; self.size = size }

    /// The pointer's lines (spec v1: `version`, then sorted keys, `oid` and `size` among them).
    init?<S: StringProtocol>(lines: [S]) {
        guard let v = lines.first, v.hasPrefix("version https://git-lfs.github.com/spec/") || v.hasPrefix("version https://hawser.github.com/spec/") else { return nil }
        var oid: String?, size: Int64?
        for l in lines.dropFirst() {
            if l.hasPrefix("oid sha256:") { oid = String(l.dropFirst(11)) }
            else if l.hasPrefix("size ") { size = Int64(l.dropFirst(5)) }
        }
        guard let oid, oid.count == 64, let size else { return nil }
        self.init(oid: oid, size: size)
    }

    init?(_ text: String) { self.init(lines: text.split(separator: "\n")) }

    /// Where `git lfs` keeps the object once it's been downloaded or added: `<git-common-dir>/lfs/objects/ab/cd/<oid>`.
    func objectPath(commonDir: String) -> String {
        "\(commonDir)/lfs/objects/\(oid.prefix(2))/\(oid.dropFirst(2).prefix(2))/\(oid)"
    }
}

/// A diff whose sides are LFS pointers (or nothing, for an added or deleted file).
struct LFSDiff: Equatable {
    var old: LFSPointer?
    var new: LFSPointer?

    init?(_ d: FileDiff) {
        guard !d.isBinary, !d.lines.isEmpty, d.lines.count < 24 else { return nil }   // a pointer is a handful of lines
        var a: [Substring] = [], b: [Substring] = []
        for l in d.lines {
            switch l.kind {
            case .context: a.append(l.text); b.append(l.text)
            case .removed: a.append(l.text)
            case .added: b.append(l.text)
            default: break
            }
        }
        let o = LFSPointer(lines: a), n = LFSPointer(lines: b)
        guard o != nil || n != nil, o != nil || a.isEmpty, n != nil || b.isEmpty else { return nil }
        old = o; new = n
    }
}

struct LFSLock: Decodable, Equatable {
    struct Owner: Decodable, Equatable { var name: String }
    var id: String
    var path: String
    var owner: Owner?
}

enum LFS {
    /// git-lfs where git (with `Git.searchPath`) will find it.
    static let installed: Bool = Git.searchPath(ProcessInfo.processInfo.environment["PATH"]).split(separator: ":")
        .contains { FileManager.default.isExecutableFile(atPath: "\($0)/git-lfs") }

    static let installHint = "brew install git-lfs && git lfs install"

    /// A `.gitattributes` that routes some path through the LFS filter.
    static func usesLFS(gitattributes: String) -> Bool {
        gitattributes.split(whereSeparator: \.isNewline).contains { line in
            let t = line.trimmingCharacters(in: .whitespaces)
            return !t.hasPrefix("#") && t.split(whereSeparator: \.isWhitespace).dropFirst().contains("filter=lfs")
        }
    }

    /// `git lfs locks --json`: path → owner.
    static func parseLocks(_ data: Data) -> [String: String]? {
        guard let locks = try? JSONDecoder().decode([LFSLock].self, from: data) else { return nil }
        return Dictionary(locks.map { ($0.path, $0.owner?.name ?? "someone") }, uniquingKeysWith: { a, _ in a })
    }

    /// The pattern `git lfs track` gets for "every file like this one": its extension, or nil without one.
    static func extensionPattern(_ path: String) -> String? {
        let ext = (path as NSString).pathExtension
        return ext.isEmpty ? nil : "*.\(ext)"
    }
}

/// Per worktree: whether the repo uses LFS, and who holds which lock.
@MainActor @Observable
final class LFSModel {
    let path: String
    var used = false
    var locks: [String: String] = [:]     // path → owner
    var lockingSupported = false          // `git lfs locks` answered and locking wasn't turned off for the remote
    @ObservationIgnored private var commonDir: String?
    @ObservationIgnored private var locksLoaded: Date?

    init(path: String) { self.path = path }

    /// On every status refresh: one small file read. The locks (a request to the server) at most once a minute, off to the side.
    func refresh(env: [String: String]) async {
        if commonDir == nil {
            commonDir = (try? await Git.string(["rev-parse", "--path-format=absolute", "--git-common-dir"], in: path))?.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        let attrs = (try? String(contentsOfFile: (path as NSString).appendingPathComponent(".gitattributes"), encoding: .utf8)) ?? ""
        let u = LFS.usesLFS(gitattributes: attrs) || commonDir.map { FileManager.default.fileExists(atPath: "\($0)/lfs/objects") } == true
        if u != used { used = u }
        if used, LFS.installed, locksLoaded.map({ Date().timeIntervalSince($0) > 60 }) ?? true {
            locksLoaded = Date()
            Task { await loadLocks(env: env) }
        }
    }

    func loadLocks(env: [String: String]) async {
        locksLoaded = Date()
        // git-lfs answers `[]` even when the server is unreachable or has no locking API, so the only "no" is a
        // remote with `lfs.<url>.locksverify false`, what git-lfs tells you to set for such a server.
        let verify = (try? await Git.string(["config", "--get-regexp", "^lfs\\..*\\.locksverify$"], in: path)) ?? ""
        let off = verify.split(separator: "\n").contains { $0.hasSuffix(" false") }
        let out = off ? nil : try? await Git.run(["lfs", "locks", "--json"], in: path, env: env)
        let found = out.flatMap(LFS.parseLocks)
        if (found != nil) != lockingSupported { lockingSupported = found != nil }
        if (found ?? [:]) != locks { locks = found ?? [:] }
    }
}

extension WorktreeModel {
    /// `git lfs track`: every file with this extension, or just this one (`--filename` keeps its name literal).
    /// It edits .gitattributes, which shows up as a change to commit.
    func lfsTrack(_ pattern: String, literal: Bool) async {
        await perform("Tracking with LFS") { try await Git.run(["lfs", "track"] + (literal ? ["--filename"] : []) + ["--", pattern], in: path) }
    }

    func lfsLock(_ paths: [String], unlock: Bool) async {
        await perform(unlock ? "Unlocking" : "Locking") {
            for p in paths { try await Git.run(["lfs", unlock ? "unlock" : "lock", "--", p], in: path, env: gitAuth) }
        }
        await lfs.loadLocks(env: gitAuth)
    }
}

// MARK: - Views

/// The file list's context menu: track with LFS, lock and unlock.
struct LFSMenuItems: View {
    var model: WorktreeModel
    var files: [FileChange]

    var body: some View {
        let paths = Array(Set(files.filter { $0.submodule == nil }.map(\.path))).sorted()
        if LFS.installed, !paths.isEmpty {
            Menu("Git LFS") {
                if paths.count == 1, let p = paths.first {
                    if let ext = LFS.extensionPattern(p) { Button("Track \(ext) with LFS") { Task { await model.lfsTrack(ext, literal: false) } } }
                    Button("Track This File with LFS") { Task { await model.lfsTrack(p, literal: true) } }
                }
                if model.lfs.lockingSupported {
                    if paths.count == 1 { Divider() }
                    let locked = paths.filter { model.lfs.locks[$0] != nil }, free = paths.filter { model.lfs.locks[$0] == nil }
                    let n = { (ps: [String]) in ps.count == 1 ? "" : " \(ps.count) Files" }
                    if !free.isEmpty { Button("Lock\(n(free))") { Task { await model.lfsLock(free, unlock: false) } } }
                    if !locked.isEmpty { Button("Unlock\(n(locked))") { Task { await model.lfsLock(locked, unlock: true) } } }
                }
            }
        }
    }
}

/// A lock on a file in the list: who holds it.
struct LFSLockBadge: View {
    var owner: String
    var body: some View {
        Image(systemName: "lock.fill").font(.caption).foregroundStyle(.secondary).help("Locked by \(owner) (Git LFS)")
    }
}

/// The repository uses LFS but git-lfs isn't installed: files show as pointers and checkouts can't fetch them.
/// Shown once; dismissing it is remembered for every repository.
struct LFSMissingBanner: View {
    var model: WorktreeModel
    @AppStorage("lfsMissingBannerDismissed") private var dismissed = false

    var body: some View {
        if model.lfs.used, !LFS.installed, !dismissed {
            HStack(spacing: 10) {
                Image(systemName: "externaldrive.badge.exclamationmark").foregroundStyle(.orange)
                VStack(alignment: .leading, spacing: 1) {
                    Text("This repository uses Git LFS, but git-lfs isn't installed").fontWeight(.medium)
                    Text("Large files stay as pointers until it is. In Terminal: \(LFS.installHint)").font(.caption).foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }
                Spacer()
                Button("Copy Command") {
                    NSPasteboard.general.clearContents(); NSPasteboard.general.setString(LFS.installHint, forType: .string)
                }
                Button("Dismiss") { dismissed = true }
            }
            .controlSize(.small)
            .padding(.horizontal, 12).padding(.vertical, 8)
            .background(Color.orange.opacity(0.08))
            Divider()
        }
    }
}

/// Instead of the pointer's text: the object's size and id on each side, and images when they're on disk.
struct LFSObjectView: View {
    var lfs: LFSDiff
    var dir: String                // a worktree of the repo, to find `.git/lfs/objects`
    var path: String               // the file, for its extension
    var newInWorktree = false      // an unstaged change: the new side is the file on disk
    @State private var images: [String: NSImage] = [:]   // oid → image
    @State private var loaded = false

    private static let imageTypes: Set<String> = ["png", "jpg", "jpeg", "gif", "tif", "tiff", "bmp", "heic", "webp", "psd", "ico", "icns"]
    private var isImage: Bool { Self.imageTypes.contains((path as NSString).pathExtension.lowercased()) }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Label("LFS object", systemImage: "externaldrive").font(.headline)
                HStack(alignment: .top, spacing: 24) {
                    if lfs.old != nil || lfs.new == nil { side("Before", lfs.old) }
                    if lfs.old != nil && lfs.new != nil { Image(systemName: "arrow.right").foregroundStyle(.secondary).padding(.top, 22) }
                    if lfs.new != nil { side("After", lfs.new) }
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .task(id: "\(dir) \(path) \(lfs.old?.oid ?? "") \(lfs.new?.oid ?? "")") { await loadImages() }
    }

    @ViewBuilder private func side(_ title: String, _ p: LFSPointer?) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.caption).foregroundStyle(.secondary)
            if let p {
                Text(ByteCountFormatter.string(fromByteCount: p.size, countStyle: .file)).font(.title3.monospacedDigit())
                Text(String(p.oid.prefix(12))).font(.caption.monospaced()).foregroundStyle(.secondary).textSelection(.enabled).help("sha256:\(p.oid)")
                if let img = images[p.oid] {
                    Image(nsImage: img).resizable().interpolation(.high).aspectRatio(contentMode: .fit)
                        .frame(maxWidth: 360, maxHeight: 360, alignment: .topLeading)
                        .help("\(Int(img.size.width)) × \(Int(img.size.height))")
                } else if isImage && loaded {
                    Text("Not downloaded").font(.caption).foregroundStyle(.tertiary).help("Pull, or run git lfs fetch, to get this version")
                }
            } else {
                Text("No file").foregroundStyle(.secondary)
            }
        }
    }

    private func loadImages() async {
        loaded = false
        defer { loaded = true }
        guard isImage else { images = [:]; return }
        let common = (try? await Git.string(["rev-parse", "--path-format=absolute", "--git-common-dir"], in: dir))?.trimmingCharacters(in: .whitespacesAndNewlines)
        let disk = newInWorktree ? (dir as NSString).appendingPathComponent(path) : nil
        let sides = [(lfs.old, nil as String?), (lfs.new, disk)].compactMap { p, f in p.map { ($0, f) } }
        let found = await Task.detached(priority: .userInitiated) {
            var out: [String: Data] = [:]
            for (p, file) in sides where p.size < 64 << 20 {
                let fm = FileManager.default
                let object = common.map { p.objectPath(commonDir: $0) }.flatMap { fm.fileExists(atPath: $0) ? $0 : nil }
                // The file on disk is that version only when it's the object itself, not a pointer.
                let onDisk = file.flatMap { f in (try? fm.attributesOfItem(atPath: f)[.size] as? NSNumber)?.int64Value == p.size ? f : nil }
                if let src = object ?? onDisk, let data = fm.contents(atPath: src) { out[p.oid] = data }
            }
            return out
        }.value
        images = found.compactMapValues(NSImage.init(data:))
    }
}

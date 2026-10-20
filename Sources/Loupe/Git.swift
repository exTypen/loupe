import Foundation

struct GitError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

/// Thin wrapper over the git CLI. The CLI (not libgit2) keeps us 100% compatible with
/// worktrees, hooks, credential helpers, signing, and whatever the user's config does.
enum Git {
    static let trace = ProcessInfo.processInfo.environment["LOUPE_TRACE"] != nil
    static let binary: String = {
        for p in ["/opt/homebrew/bin/git", "/usr/local/bin/git", "/usr/bin/git"]
        where FileManager.default.isExecutableFile(atPath: p) { return p }
        return "/usr/bin/git"
    }()

    @discardableResult
    static func run(_ args: [String], in dir: String, stdin: Data? = nil, allowFailure: Bool = false) async throws -> Data {
        let enqueued = Date()
        return try await withCheckedThrowingContinuation { cont in
            DispatchQueue.global(qos: .userInitiated).async {
                let p = Process()
                p.executableURL = URL(fileURLWithPath: binary)
                p.arguments = args
                p.currentDirectoryURL = URL(fileURLWithPath: dir)
                var env = ProcessInfo.processInfo.environment
                env["GIT_OPTIONAL_LOCKS"] = "0"   // status must not fight other git processes for index.lock
                env["GIT_TERMINAL_PROMPT"] = "0"
                env["LC_ALL"] = "C"
                p.environment = env
                let out = Pipe(), err = Pipe(), inp = Pipe()
                p.standardOutput = out
                p.standardError = err
                p.standardInput = stdin == nil ? FileHandle.nullDevice : inp
                let started = Date()
                do { try p.run() } catch { cont.resume(throwing: error); return }
                if let stdin {
                    DispatchQueue.global().async {
                        inp.fileHandleForWriting.write(stdin)
                        try? inp.fileHandleForWriting.close()
                    }
                }
                // Drain stderr concurrently so a chatty stderr can't deadlock a big stdout.
                var errData = Data()
                let g = DispatchGroup()
                g.enter()
                DispatchQueue.global().async { errData = err.fileHandleForReading.readDataToEndOfFile(); g.leave() }
                let outData = out.fileHandleForReading.readDataToEndOfFile()
                p.waitUntilExit()
                g.wait()
                if trace { FileHandle.standardError.write(Data(String(format: "[git %.0fms queued+run] %@ (%@)\n", Date().timeIntervalSince(enqueued) * 1000, args.prefix(3).joined(separator: " "), String(format: "%.0fms run", Date().timeIntervalSince(started) * 1000)).utf8)) }
                if p.terminationStatus != 0 && !allowFailure {
                    let msg = String(decoding: errData, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
                    cont.resume(throwing: GitError(message: msg.isEmpty ? "git \(args.first ?? "") failed (\(p.terminationStatus))" : msg))
                } else {
                    cont.resume(returning: outData)
                }
            }
        }
    }

    static func string(_ args: [String], in dir: String) async throws -> String {
        String(decoding: try await run(args, in: dir), as: UTF8.self)
    }
}

// MARK: - Worktrees

struct Worktree: Identifiable, Hashable {
    var path: String
    var head: String = ""
    var branch: String?        // short name, nil when detached
    var isBare = false
    var isLocked = false
    var isPrunable = false
    var id: String { path }
    var name: String { (path as NSString).lastPathComponent }
}

extension Git {
    static func worktrees(in dir: String) async throws -> [Worktree] {
        parseWorktrees(try await string(["worktree", "list", "--porcelain"], in: dir))
    }

    static func parseWorktrees(_ s: String) -> [Worktree] {
        var result: [Worktree] = []
        var cur: Worktree?
        for line in s.split(separator: "\n", omittingEmptySubsequences: false) {
            if line.isEmpty { if let c = cur { result.append(c) }; cur = nil; continue }
            let (key, value) = line.splitOnce(" ")
            switch key {
            case "worktree": cur = Worktree(path: String(value))
            case "HEAD": cur?.head = String(value)
            case "branch": cur?.branch = value.hasPrefix("refs/heads/") ? String(value.dropFirst(11)) : String(value)
            case "bare": cur?.isBare = true
            case "locked": cur?.isLocked = true
            case "prunable": cur?.isPrunable = true
            default: break
            }
        }
        if let c = cur { result.append(c) }
        return result
    }
}

// MARK: - Status

struct FileChange: Identifiable, Hashable {
    enum Kind: Character { case modified = "M", added = "A", deleted = "D", renamed = "R", copied = "C", typeChanged = "T", untracked = "?", conflicted = "U" }
    var path: String
    var origPath: String?
    var kind: Kind
    var staged: Bool
    var id: String { (staged ? "S:" : "W:") + path }
}

extension Git {
    static func status(in dir: String) async throws -> (branch: String?, oid: String, ahead: Int, behind: Int, files: [FileChange]) {
        let data = try await run(["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=all"], in: dir)
        return parseStatus(data)
    }

    /// Parses `git status --porcelain=v2 -z --branch`. Each file can yield a staged and an unstaged entry.
    static func parseStatus(_ data: Data) -> (branch: String?, oid: String, ahead: Int, behind: Int, files: [FileChange]) {
        var branch: String?, oid = "", ahead = 0, behind = 0
        var files: [FileChange] = []
        let parts = data.split(separator: 0, omittingEmptySubsequences: false).map { String(decoding: $0, as: UTF8.self) }
        var i = 0
        func kind(_ c: Character) -> FileChange.Kind { FileChange.Kind(rawValue: c) ?? .modified }
        while i < parts.count {
            let e = parts[i]; i += 1
            guard let t = e.first else { continue }
            switch t {
            case "#":
                if e.hasPrefix("# branch.oid ") { oid = String(e.dropFirst(13)) }
                if e.hasPrefix("# branch.head ") { let h = String(e.dropFirst(14)); branch = h == "(detached)" ? nil : h }
                if e.hasPrefix("# branch.ab ") {
                    let ab = e.dropFirst(12).split(separator: " ")
                    if ab.count == 2 { ahead = Int(ab[0].dropFirst()) ?? 0; behind = Int(ab[1].dropFirst()) ?? 0 }
                }
            case "1", "2":
                // 1 XY sub mH mI mW hH hI path   |   2 XY sub mH mI mW hH hI Xscore path \0 origPath
                let f = e.split(separator: " ", maxSplits: t == "1" ? 8 : 9, omittingEmptySubsequences: false)
                guard f.count >= (t == "1" ? 9 : 10) else { continue }
                let xy = Array(f[1])
                let path = String(f[t == "1" ? 8 : 9])
                var orig: String?
                if t == "2", i < parts.count { orig = parts[i]; i += 1 }
                if xy[0] != "." { files.append(FileChange(path: path, origPath: orig, kind: kind(xy[0]), staged: true)) }
                if xy[1] != "." { files.append(FileChange(path: path, origPath: xy[1] == "R" ? orig : nil, kind: kind(xy[1]), staged: false)) }
            case "u":
                let f = e.split(separator: " ", maxSplits: 10, omittingEmptySubsequences: false)
                if f.count >= 11 { files.append(FileChange(path: String(f[10]), kind: .conflicted, staged: false)) }
            case "?":
                files.append(FileChange(path: String(e.dropFirst(2)), kind: .untracked, staged: false))
            default: break
            }
        }
        return (branch, oid, ahead, behind, files)
    }

    static func diff(for file: FileChange, in dir: String, context: Int = 3) async throws -> FileDiff? {
        let args: [String]
        if file.kind == .untracked {
            args = ["diff", "--no-color", "--no-ext-diff", "--no-index", "-U\(context)", "--", "/dev/null", file.path]
        } else {
            args = ["diff", "--no-color", "--no-ext-diff", "-M", "-U\(context)"] + (file.staged ? ["--cached"] : []) + ["--", file.path] + (file.origPath.map { [$0] } ?? [])
        }
        // --no-index exits 1 when files differ.
        let data = try await run(args, in: dir, allowFailure: file.kind == .untracked)
        return DiffParser.parse(data).first
    }
}

extension Substring {
    func splitOnce(_ sep: Character) -> (Substring, Substring) {
        guard let i = firstIndex(of: sep) else { return (self, "") }
        return (self[..<i], self[index(after: i)...])
    }
}

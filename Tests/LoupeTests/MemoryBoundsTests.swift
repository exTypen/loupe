import Testing
import Foundation
@testable import Loupe

/// Caps, eviction and cancellation that keep memory bounded (scripts/memtest found each of these growing without bound).
struct CodeIndexBoundsTests {
    @Test func sizeCaps() {
        #expect(!CodeIndex.isTooLarge("func a() {}\n"))
        #expect(CodeIndex.isTooLarge(String(repeating: "x", count: CodeIndex.maxParseBytes + 1)))
        // Minified: few lines, each very long.
        let minified = (0..<100).map { _ in String(repeating: "var a=1;", count: 200) }.joined(separator: "\n")
        #expect(CodeIndex.isTooLarge(minified))
        // Many short lines over the line cap.
        #expect(CodeIndex.isTooLarge(String(repeating: "x\n", count: CodeIndex.maxParseLines + 10)))
        // Ordinary source of a few thousand lines.
        #expect(!CodeIndex.isTooLarge((0..<3000).map { "func f\($0)() { return \($0) }" }.joined(separator: "\n")))
    }

    @Test func outlineStillWorks() async {
        let text = "struct A {}\nfunc b() {}\n"
        guard case .symbols(let s) = await CodeIndex.outline(of: text, path: "x.swift") else { Issue.record("too large?"); return }
        #expect(s.map(\.name).contains("b"))
        #expect(await CodeIndex.outline(of: String(repeating: "var a=1;", count: 400_000), path: "x.js") == .tooLarge)
    }

    @Test func concurrentCallsShareOneParse() async {
        let cache = OutlineCache()
        let text = (0..<2000).map { "func f\($0)() {}" }.joined(separator: "\n")
        async let a = cache.outline(text, path: "a.swift")
        async let b = cache.outline(text, path: "a.swift")
        let (x, y) = await (a, b)
        #expect(x == y && !x.isEmpty)
        #expect(cache.parses.withLock { $0 } == 1)
        _ = await cache.outline(text, path: "a.swift")   // cached
        #expect(cache.parses.withLock { $0 } == 1)
        #expect(cache.inFlight == 0)
    }

    @Test func cancellingTheCallerStopsTheParse() async {
        let cache = OutlineCache()
        // Under the caps but slow enough to still be parsing when cancelled.
        let text = (0..<45_000).map { "func f\($0)(a: Int, b: String) -> [Int: String] { return [a: b] }" }.joined(separator: "\n")
        let t = Task { await cache.outline(String(text.prefix(CodeIndex.maxParseBytes - 1)), path: "big.swift") }
        try? await Task.sleep(for: .milliseconds(50))
        t.cancel()
        let start = Date()
        let r = await t.value
        #expect(r.isEmpty)
        #expect(Date().timeIntervalSince(start) < 2)
        #expect(cache.inFlight == 0)
    }

    @Test func cancelledSymbolsReturnEmpty() async {
        let t = Task { () -> [CodeSymbol] in
            withUnsafeCurrentTask { $0?.cancel() }
            return CodeIndex.symbols(in: "func a() {}\n", path: "a.swift")
        }
        #expect(await t.value.isEmpty)
    }
}

struct GitCancellationTests {
    @Test func readOnlyDetection() {
        #expect(Git.isReadOnly(["show", "HEAD:a"]))
        #expect(Git.isReadOnly(["-c", "core.quotepath=off", "diff", "--no-color"]))
        #expect(!Git.isReadOnly(["commit", "-m", "x"]))
        #expect(!Git.isReadOnly(["-c", "core.editor=true", "rebase", "--continue"]))
    }

    /// Cancelling stops a running process at once.
    @Test func cancellingStopsTheProcess() throws {
        let running = Git.RunningProcess(killable: true)
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/sleep"); p.arguments = ["30"]
        #expect(try running.start(p))
        let start = Date()
        running.cancel()
        p.waitUntilExit()
        #expect(Date().timeIntervalSince(start) < 3)
        #expect(running.wasCancelled)
    }

    /// A read cancelled before it starts never launches git.
    @Test func cancelledBeforeLaunch() async {
        let t = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            return try await Git.run(["status"], in: FileManager.default.temporaryDirectory.path)
        }
        let r = await t.result
        if case .failure(let e) = r { #expect(e is CancellationError) } else { Issue.record("expected cancellation") }
    }

    @Test func writesAreNotKilled() async throws {
        let running = Git.RunningProcess(killable: false)
        running.cancel()
        #expect(!running.wasCancelled)
    }
}

@MainActor struct PullDetailsEvictionTests {
    static func pr(_ n: Int) -> PullRequest {
        PullRequest(id: n, number: n, title: "t", body: nil, state: "open", draft: nil, user: FUser(login: "a", avatar_url: nil),
                    head: FBranchRef(ref: "b", sha: "1", label: nil), base: FBranchRef(ref: "main", sha: "0", label: nil),
                    additions: nil, deletions: nil, changed_files: nil, comments: nil, mergeable: nil, merged: nil, merged_at: nil,
                    requested_reviewers: nil, html_url: "", updated_at: "", created_at: nil)
    }

    @Test func keepsOnlyTheMostRecent() {
        let l = PullListModel()
        let c = ForgejoClient(baseURL: "http://127.0.0.1:1", token: "x")
        let r = RemoteRepo(remoteURL: "http://127.0.0.1:1/o/r.git")!
        let local = LocalCheckout(dir: "/tmp", remote: "origin")
        for n in 1...5 { l.selected = n; _ = l.detail(for: Self.pr(n), client: c, repo: r, local: local) }
        #expect(Set(l.details.keys) == [4, 5])
        let five = l.details[5]
        _ = l.detail(for: Self.pr(4), client: c, repo: r, local: local)   // reopening reuses
        l.selected = 3; _ = l.detail(for: Self.pr(3), client: c, repo: r, local: local)
        #expect(Set(l.details.keys) == [3, 4])
        #expect(five != nil)
        l.selected = nil
        #expect(l.details.count <= PullListModel.keptDetails)
    }
}

@MainActor struct WorktreeEvictionTests {
    @Test func goneWorktreesAreForgotten() async throws {
        let base = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-evict-\(UUID().uuidString)").path
        let dir = base + "/r", wt = base + "/r-wt"
        try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        for args in [["init", "-q", "-b", "main"], ["config", "user.name", "t"], ["config", "user.email", "t@t"], ["config", "commit.gpgsign", "false"],
                     ["commit", "-q", "--allow-empty", "-m", "one"], ["worktree", "add", "-q", "-b", "x", wt]] {
            try await Git.run(args, in: dir)
        }
        let dirReal = try await Git.string(["rev-parse", "--show-toplevel"], in: dir).trimmingCharacters(in: .whitespacesAndNewlines)
        let app = AppModel()
        await app.refreshWorktrees(repo: dirReal)
        let paths = app.worktrees[dirReal]?.map(\.path) ?? []
        #expect(paths.count == 2)
        for p in paths { _ = app.model(for: p) }
        let before = app.cachedModelCount
        let linked = paths.first { $0 != dirReal }!
        _ = UndoStack.for(linked)
        try await Git.run(["worktree", "remove", "--force", linked], in: dirReal)
        await app.refreshWorktrees(repo: dirReal)
        #expect(app.worktrees[dirReal]?.count == 1)
        #expect(app.cachedModelCount == before - 1)
        try? FileManager.default.removeItem(atPath: base)
    }

    @Test func leavingAWorktreeReleasesItsCommitFiles() async throws {
        let app = AppModel()
        let m = app.model(for: "/nonexistent-\(UUID().uuidString)")
        m.history.files = [FileDiff()]
        m.releaseHeavyState()
        #expect(m.history.files.isEmpty)
    }
}

@MainActor struct JobLogBoundsTests {
    @Test func tailKeepsWholeLines() {
        let d = Data("aaaa\nbbbb\ncccc\n".utf8)
        let (t, start) = JobLogModel.tail(d, start: 0, limit: 7)
        #expect(String(decoding: t, as: UTF8.self) == "cccc\n")
        #expect(start == 10)
        #expect(JobLogModel.tail(d, start: 5, limit: 100).1 == 5)
    }

    @Test func liveTailDropsItsOldestOutput() {
        let job = ActionJob(id: 1, run_id: 1, attempt: 1, name: "j", html_url: nil, needs: nil, runs_on: nil, task_id: nil, status: "running", steps: nil)
        let m = JobLogModel(client: ForgejoClient(baseURL: "http://127.0.0.1:1", token: "x"), repo: RemoteRepo(remoteURL: "http://127.0.0.1:1/o/r.git")!,
                            job: job, features: CIFeatures.detect(github: false, version: "11.0.0"))
        let line = String(repeating: "x", count: 99) + "\n"
        m.storage.append(NSAttributedString(string: String(repeating: line, count: JobLogModel.maxShown / 100 + 1000)))
        let lastLoc = m.storage.length - 100
        m.problems = [LogProblem(line: "old", range: NSRange(location: 5, length: 3)), LogProblem(line: "new", range: NSRange(location: lastLoc, length: 3))]
        m.trimFront()
        #expect(m.storage.length <= JobLogModel.maxShown)
        #expect(m.problems.map(\.line) == ["new"])
        #expect(m.problems[0].range.location == m.storage.length - 100)
        #expect(m.truncatedBytes > 0)
    }
}

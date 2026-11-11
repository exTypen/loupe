import Testing
import Foundation
@testable import Loupe

private func item(_ sha: String, _ message: String, _ action: RebaseAction = .pick, parents: [String] = ["p"], new: String? = nil) -> RebaseItem {
    RebaseItem(sha: sha, parents: parents, message: message, action: action, newMessage: new)
}

@Test func todoKeepsOrderAndPlainActions() throws {
    let b = try RebaseTodo.build([item("a", "A"), item("b", "B", .edit), item("c", "C", .drop), item("d", "D")], messageDir: "/m")
    #expect(b.todo == "pick a\nedit b\ndrop c\npick d\n")
    #expect(b.messages.isEmpty)
}

@Test func todoTurnsRewordAndSquashIntoAmendExecs() throws {
    let b = try RebaseTodo.build([
        item("a", "A", .reword, new: "A2"),
        item("b", "B"), item("c", "C", .squash), item("d", "D", .fixup), item("e", "E", .squash, new: "E2"),
        item("f", "F"), item("g", "G", .fixup),          // fixup alone keeps F's message: no exec
        item("h", "H", .reword),                          // reword without a change: no exec
    ], messageDir: "/tmp/it's")
    #expect(b.todo == """
        pick a
        exec git commit --amend --allow-empty --quiet -F '/tmp/it'\\''s/message-1'
        pick b
        fixup c
        fixup d
        fixup e
        exec git commit --amend --allow-empty --quiet -F '/tmp/it'\\''s/message-2'
        pick f
        fixup g
        pick h

        """)
    #expect(b.messages == ["message-1": "A2", "message-2": "B\n\nC\n\nE2"])
}

@Test func squashSkipsDroppedCommitsAndNeedsAnOlderOne() throws {
    let b = try RebaseTodo.build([item("a", "A"), item("b", "B", .drop), item("c", "C", .fixup)], messageDir: "/m")
    #expect(b.todo == "pick a\ndrop b\nfixup c\n")
    #expect(throws: GitError.self) { try RebaseTodo.build([item("a", "A", .drop), item("b", "B", .squash)], messageDir: "/m") }
}

@Test func dragMovesAndMelds() {
    let items = ["a", "b", "c", "d"].map { item($0, $0.uppercased()) }   // oldest first; History shows d on top
    #expect(RebaseTodo.move(items, "a", to: .above, of: "c").map(\.sha) == ["b", "c", "a", "d"])
    #expect(RebaseTodo.move(items, "d", to: .below, of: "b").map(\.sha) == ["a", "d", "b", "c"])
    let melded = RebaseTodo.move(items, "d", to: .onto, of: "a", fixup: true)
    #expect(melded.map(\.sha) == ["a", "d", "b", "c"])
    #expect(melded[1].action == .fixup)
    #expect(RebaseTodo.move(items, "a", to: .onto, of: "c")[2].action == .squash)
}

@Test func firstChangeAndPushedCommits() {
    let items = ["a", "b", "c"].map { item($0, $0.uppercased()) }
    let range = RebaseRange(base: "x", items: items, pushed: ["a", "b"])
    #expect(RebaseTodo.firstChange(original: ["a", "b", "c"], plan: items) == nil)
    var plan = items; plan[2].action = .drop
    #expect(RebaseTodo.firstChange(original: ["a", "b", "c"], plan: plan) == 2)
    #expect(RebaseTodo.rewrittenPushed(range, plan: plan).isEmpty)   // a and b keep their SHAs
    plan = items; plan[1].action = .reword; plan[1].newMessage = "B2"
    #expect(RebaseTodo.rewrittenPushed(range, plan: plan).map(\.sha) == ["b"])
}

@Test func parsesLogWithMultilineMessages() {
    let s = "aaa\u{1f}p1\u{1f}Subject\n\nBody line\n\u{1e}\nbbb\u{1f}p1 p2\u{1f}Merge x\n\u{1e}\n"
    let items = RebaseTodo.parseLog(s)
    #expect(items.map(\.sha) == ["aaa", "bbb"])
    #expect(items[0].message == "Subject\n\nBody line")
    #expect(items[0].subject == "Subject")
    #expect(items[1].parents == ["p1", "p2"])
}

// MARK: - End to end on a temp repo

private struct Repo {
    let dir: String
    init() async throws {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-rebase-test-\(UUID().uuidString)").path
        try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        try await git("init", "-q", "-b", "main")
        for (k, v) in [("user.name", "t"), ("user.email", "t@t"), ("commit.gpgsign", "false"), ("core.hooksPath", "/dev/null")] {
            try await git("config", k, v)
        }
    }
    @discardableResult func git(_ args: String...) async throws -> String { try await Git.string(args, in: dir) }
    func commit(_ name: String, file: String? = nil) async throws {
        try "\(name)\n".write(toFile: dir + "/" + (file ?? name), atomically: true, encoding: .utf8)
        try await git("add", ".")
        try await git("commit", "-qm", name)
    }
    func subjects() async throws -> [String] { try await git("log", "--format=%s").split(separator: "\n").map(String.init) }
    func sha(_ rev: String) async throws -> String { try await git("rev-parse", rev).trimmingCharacters(in: .whitespacesAndNewlines) }
    func plan(from rev: String, _ change: ([RebaseItem]) -> [RebaseItem]) async throws -> Bool {
        let range = try await Git.rebaseRange(from: try await sha(rev), in: dir)
        return try await Git.runRebase(range, plan: change(range.items), in: dir)
    }
    func remove() { try? FileManager.default.removeItem(atPath: dir) }
}

@Test func rebaseEndToEnd() async throws {
    let r = try await Repo()
    defer { r.remove() }
    for n in ["a", "b", "c", "d"] { try await r.commit(n) }

    // Reorder: d below b.
    var stopped = try await r.plan(from: "HEAD~2") { RebaseTodo.move($0, $0[2].sha, to: .below, of: $0[0].sha) }
    #expect(!stopped)
    #expect(try await r.subjects() == ["c", "b", "d", "a"])

    // Squash c into b, with b's and c's messages; fixup d into a, keeping a's.
    stopped = try await r.plan(from: "HEAD~3") { items in
        var p = RebaseTodo.move(items, items[3].sha, to: .onto, of: items[2].sha)                  // c onto b
        p = RebaseTodo.move(p, p[1].sha, to: .onto, of: p[0].sha, fixup: true)                       // d onto a
        return p
    }
    #expect(!stopped)
    #expect(try await r.subjects() == ["b", "a"])
    #expect(try await r.git("log", "-1", "--format=%B").trimmingCharacters(in: .whitespacesAndNewlines) == "b\n\nc")
    #expect(try await r.git("show", "--name-only", "--format=", "HEAD~1").split(separator: "\n").sorted() == ["a", "d"])

    // Reword the root commit, with a quote and a second paragraph; the worktree is dirty, so --autostash carries it.
    try "dirty\n".write(toFile: r.dir + "/a", atomically: true, encoding: .utf8)
    stopped = try await r.plan(from: "HEAD~1") { items in
        var p = items; p[0].action = .reword; p[0].newMessage = "a's new name\n\nwith a body"; return p
    }
    #expect(!stopped)
    #expect(try await r.subjects() == ["b", "a's new name"])
    #expect(try await r.git("log", "-1", "--format=%B", "HEAD~1").trimmingCharacters(in: .whitespacesAndNewlines) == "a's new name\n\nwith a body")
    #expect(try String(contentsOfFile: r.dir + "/a", encoding: .utf8) == "dirty\n")
    try await r.git("checkout", "--", "a")

    // Drop.
    try await r.commit("e")
    stopped = try await r.plan(from: "HEAD~1") { items in var p = items; p[0].action = .drop; return p }
    #expect(!stopped)
    #expect(try await r.subjects() == ["e", "a's new name"])

    // Edit stops with the rebase waiting; Continue finishes it and runs the exec lines still queued.
    try await r.commit("f")
    let gitDir = r.dir + "/.git"
    stopped = try await r.plan(from: "HEAD~1") { items in
        var p = items; p[0].action = .edit; p[1].action = .reword; p[1].newMessage = "f2"; return p
    }
    #expect(stopped)
    #expect(FileManager.default.fileExists(atPath: gitDir + "/rebase-merge/amend"))
    try await Git.run(["-c", "core.editor=true", "rebase", "--continue"], in: r.dir, env: ["GIT_EDITOR": "true"])
    #expect(!FileManager.default.fileExists(atPath: gitDir + "/rebase-merge"))
    #expect(try await r.subjects() == ["f2", "e", "a's new name"])

    // A conflict stops too, and Abort puts everything back.
    try await r.commit("g1", file: "same")
    try await r.commit("g2", file: "same")
    let before = try await r.sha("HEAD")
    stopped = try await r.plan(from: "HEAD~1") { RebaseTodo.move($0, $0[1].sha, to: .below, of: $0[0].sha) }
    #expect(stopped)
    #expect(!FileManager.default.fileExists(atPath: gitDir + "/rebase-merge/amend"))
    try await r.git("rebase", "--abort")
    #expect(try await r.sha("HEAD") == before)
}

@Test func rebaseRangeRefusesMergesAndMarksPushed() async throws {
    let r = try await Repo()
    defer { r.remove() }
    try await r.commit("a")
    try await r.commit("b")
    try await r.git("update-ref", "refs/remotes/origin/main", "HEAD")   // "pushed" up to b
    try await r.commit("c")
    var range = try await Git.rebaseRange(from: try await r.sha("HEAD~1"), in: r.dir)
    #expect(range.items.map(\.subject) == ["b", "c"])
    #expect(range.pushed == [try await r.sha("HEAD~1")])
    #expect(range.base == (try await r.sha("HEAD~2")))
    range = try await Git.rebaseRange(from: try await r.sha("HEAD~2"), in: r.dir)
    #expect(range.base == nil && range.items.count == 3)   // from the root

    try await r.git("switch", "-qc", "side", "HEAD~1")
    try await r.commit("s")
    try await r.git("switch", "-q", "main")
    try await r.git("merge", "-q", "--no-ff", "--no-edit", "side")
    try await r.commit("d")
    await #expect(throws: GitError.self) { try await Git.rebaseRange(from: try await r.sha("HEAD~3"), in: r.dir) }
}

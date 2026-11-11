import Testing
import Foundation
@testable import Loupe

/// A throwaway repo with one commit of a.txt ("one\n").
private func repo() async throws -> String {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-undo-\(UUID().uuidString)").path
    try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    for args in [["init", "-q", "-b", "main"], ["config", "user.name", "t"], ["config", "user.email", "t@t"], ["config", "commit.gpgsign", "false"]] {
        try await Git.run(args, in: dir)
    }
    try write("one\n", "a.txt", dir)
    try await Git.run(["add", "a.txt"], in: dir)
    try await Git.run(["commit", "-q", "-m", "first"], in: dir)
    return dir
}

private func write(_ s: String, _ name: String, _ dir: String) throws {
    try s.write(toFile: (dir as NSString).appendingPathComponent(name), atomically: true, encoding: .utf8)
}
private func read(_ name: String, _ dir: String) -> String? { try? String(contentsOfFile: (dir as NSString).appendingPathComponent(name), encoding: .utf8) }
private func git(_ args: String..., in dir: String) async throws -> String {
    try await Git.string(args, in: dir).trimmingCharacters(in: .whitespacesAndNewlines)
}

@MainActor @Test func discardUndoRestoresTrackedAndUntracked() async throws {
    let dir = try await repo()
    try write("two\n", "a.txt", dir)
    try write("new\n", "n.txt", dir)
    let stack = UndoStack(path: dir)
    try await stack.run(.files("Discard of 2 Files", ["a.txt", "n.txt"])) {
        try await Git.run(["restore", "--worktree", "--", "a.txt"], in: dir)
        try FileManager.default.removeItem(atPath: (dir as NSString).appendingPathComponent("n.txt"))
    }
    #expect(read("a.txt", dir) == "one\n" && read("n.txt", dir) == nil)
    #expect(stack.title == "Undo Discard of 2 Files" && stack.toast != nil)
    try await stack.undo()
    #expect(read("a.txt", dir) == "two\n")
    #expect(read("n.txt", dir) == "new\n")
    #expect(stack.entries.isEmpty)
}

@MainActor @Test func discardUndoRefusesWhenTheFileChangedSince() async throws {
    let dir = try await repo()
    try write("two\n", "a.txt", dir)
    let stack = UndoStack(path: dir)
    try await stack.run(.files("Discard in a.txt", ["a.txt"])) { try await Git.run(["restore", "--worktree", "--", "a.txt"], in: dir) }
    try write("edited later\n", "a.txt", dir)
    await #expect(throws: UndoRefusal.self) { try await stack.undo() }
    #expect(read("a.txt", dir) == "edited later\n")   // never overwritten
    #expect(stack.entries.isEmpty)                    // and the undo is gone for good
}

@MainActor @Test func stageUndoRestoresTheIndex() async throws {
    let dir = try await repo()
    try write("two\n", "a.txt", dir)
    let stack = UndoStack(path: dir)
    try await stack.run(.files("Stage a.txt", [], index: true, toast: false)) { try await Git.run(["add", "a.txt"], in: dir) }
    #expect(try await git("diff", "--cached", "--name-only", in: dir) == "a.txt")
    try await stack.undo()
    #expect(try await git("diff", "--cached", "--name-only", in: dir) == "")
    #expect(read("a.txt", dir) == "two\n")
}

@MainActor @Test func commitUndoKeepsChangesStaged() async throws {
    let dir = try await repo()
    let first = try await git("rev-parse", "HEAD", in: dir)
    try write("two\n", "a.txt", dir)
    try await Git.run(["add", "a.txt"], in: dir)
    let stack = UndoStack(path: dir)
    var message = ""
    try await stack.run(.reset("Commit", soft: true, offerRevert: true) { message = "second" }) {
        try await Git.run(["commit", "-q", "-m", "second"], in: dir)
    }
    #expect(try await git("rev-parse", "HEAD", in: dir) != first)
    try await stack.undo()
    #expect(try await git("rev-parse", "HEAD", in: dir) == first)
    #expect(try await git("diff", "--cached", "--name-only", in: dir) == "a.txt")
    #expect(message == "second")
}

@MainActor @Test func amendUndoRestoresThePreviousCommit() async throws {
    let dir = try await repo()
    let first = try await git("rev-parse", "HEAD", in: dir)
    try write("two\n", "a.txt", dir)
    try await Git.run(["add", "a.txt"], in: dir)
    let stack = UndoStack(path: dir)
    try await stack.run(.reset("Amend", soft: true)) { try await Git.run(["commit", "-q", "--amend", "-m", "first, amended"], in: dir) }
    try await stack.undo()
    #expect(try await git("rev-parse", "HEAD", in: dir) == first)
    #expect(try await git("diff", "--cached", "--name-only", in: dir) == "a.txt")
}

@MainActor @Test func undoOfAPushedCommitIsRefusedWithARevertOffer() async throws {
    let dir = try await repo()
    try write("two\n", "a.txt", dir)
    try await Git.run(["add", "a.txt"], in: dir)
    let stack = UndoStack(path: dir)
    try await stack.run(.reset("Commit", soft: true, offerRevert: true)) { try await Git.run(["commit", "-q", "-m", "second"], in: dir) }
    let head = try await git("rev-parse", "HEAD", in: dir)
    try await Git.run(["update-ref", "refs/remotes/origin/main", head], in: dir)   // as if pushed
    do { try await stack.undo(); Issue.record("undo should be refused") }
    catch let r as UndoRefusal { #expect(r.revert == head && r.message.contains("origin/main")) }
    #expect(try await git("rev-parse", "HEAD", in: dir) == head)
    #expect(stack.entries.count == 1)   // still there: Revert is offered instead
}

@MainActor @Test func stashDropUndoStoresItAgain() async throws {
    let dir = try await repo()
    try write("two\n", "a.txt", dir)
    try await Git.run(["stash", "push", "-q", "-m", "wip"], in: dir)
    let sha = try await git("rev-parse", "stash@{0}", in: dir)
    let subject = try await git("log", "-1", "--format=%s", sha, in: dir)
    let stack = UndoStack(path: dir)
    try await stack.run(.stash("Drop Stash", sha: sha, message: subject, applied: false, dropped: true)) {
        try await Git.run(["stash", "drop", "-q", "stash@{0}"], in: dir)
    }
    #expect(try await git("stash", "list", in: dir) == "")
    try await stack.undo()
    #expect(try await git("stash", "list", "--format=%H %gs", in: dir) == "\(sha) On main: wip")
}

@MainActor @Test func stashPopUndoTakesBackTheChangesAndTheStash() async throws {
    let dir = try await repo()
    try write("two\n", "a.txt", dir)
    try write("u\n", "u.txt", dir)
    try await Git.run(["stash", "push", "-q", "--include-untracked", "-m", "wip"], in: dir)
    let sha = try await git("rev-parse", "stash@{0}", in: dir)
    let stack = UndoStack(path: dir)
    try await stack.run(.stash("Pop Stash", sha: sha, message: "On main: wip", applied: true, dropped: true)) {
        try await Git.run(["stash", "pop", "-q", "stash@{0}"], in: dir)
    }
    #expect(read("a.txt", dir) == "two\n" && read("u.txt", dir) == "u\n")
    try await stack.undo()
    #expect(read("a.txt", dir) == "one\n" && read("u.txt", dir) == nil)
    #expect(try await git("stash", "list", "--format=%H", in: dir) == sha)
}

@MainActor @Test func cherryPickUndoResetsToTheHeadBefore() async throws {
    let dir = try await repo()
    let base = try await git("rev-parse", "HEAD", in: dir)
    try await Git.run(["switch", "-q", "-c", "side"], in: dir)
    try write("side\n", "s.txt", dir)
    try await Git.run(["add", "s.txt"], in: dir)
    try await Git.run(["commit", "-q", "-m", "side"], in: dir)
    let pick = try await git("rev-parse", "HEAD", in: dir)
    try await Git.run(["switch", "-q", "main"], in: dir)
    try write("dirty but unrelated\n", "a.txt", dir)   // reset --keep keeps local changes
    let stack = UndoStack(path: dir)
    try await stack.run(.reset("Cherry-pick of side", offerRevert: true)) { try await Git.run(["cherry-pick", pick], in: dir) }
    #expect(read("s.txt", dir) == "side\n")
    try await stack.undo()
    #expect(try await git("rev-parse", "HEAD", in: dir) == base)
    #expect(read("s.txt", dir) == nil)
    #expect(read("a.txt", dir) == "dirty but unrelated\n")
}

@MainActor @Test func entriesAreDroppedOnceHeadMovesElsewhere() async throws {
    let dir = try await repo()
    try write("two\n", "a.txt", dir)
    let stack = UndoStack(path: dir)
    try await stack.run(.files("Discard of a.txt", ["a.txt"])) { try await Git.run(["restore", "--worktree", "--", "a.txt"], in: dir) }
    let oid = try await git("rev-parse", "HEAD", in: dir)
    stack.prune(branch: "main", oid: oid)
    #expect(stack.entries.count == 1)
    try await Git.run(["commit", "-q", "--allow-empty", "-m", "elsewhere"], in: dir)   // a terminal commit
    stack.prune(branch: "main", oid: try await git("rev-parse", "HEAD", in: dir))
    #expect(stack.entries.isEmpty && stack.toast == nil)
}

@MainActor @Test func stackIsBounded() async throws {
    let dir = try await repo()
    let stack = UndoStack(path: dir)
    for i in 0..<(UndoStack.limit + 5) { await stack.record("Step \(i)") {} }
    #expect(stack.entries.count == UndoStack.limit && stack.entries.first?.label == "Step 5")
}

@MainActor @Test func discardUndoNeverDeletesADirectoryMadeSince() async throws {
    let dir = try await repo()
    try write("new\n", "n", dir)
    let stack = UndoStack(path: dir)
    try await stack.run(.files("Discard of n", ["n"])) { try FileManager.default.removeItem(atPath: (dir as NSString).appendingPathComponent("n")) }
    try FileManager.default.createDirectory(atPath: (dir as NSString).appendingPathComponent("n"), withIntermediateDirectories: true)
    try write("keep\n", "n/inside.txt", dir)
    await #expect(throws: UndoRefusal.self) { try await stack.undo() }
    #expect(read("n/inside.txt", dir) == "keep\n")
}

@MainActor @Test func aStatusReadFromBeforeAnActionDoesNotPrune() async throws {
    let dir = try await repo()
    let before = try await git("rev-parse", "HEAD", in: dir)
    let stamp = UndoStack(path: dir).stamp
    let stack = UndoStack(path: dir)
    try write("two\n", "a.txt", dir)
    try await Git.run(["add", "a.txt"], in: dir)
    try await stack.run(.reset("Commit", soft: true)) { try await Git.run(["commit", "-q", "-m", "second"], in: dir) }
    stack.prune(branch: "main", oid: before, since: stamp)   // the status was read before the commit
    #expect(stack.entries.count == 1)
    stack.prune(branch: "main", oid: before, since: stack.stamp)   // a fresh read showing HEAD moved elsewhere
    #expect(stack.entries.isEmpty)
}

@MainActor @Test func stashPopUndoHandlesNonASCIINames() async throws {
    let dir = try await repo()
    try write("é\n", "café.txt", dir)
    try await Git.run(["stash", "push", "-q", "--include-untracked", "-m", "wip"], in: dir)
    let sha = try await git("rev-parse", "stash@{0}", in: dir)
    let stack = UndoStack(path: dir)
    try await stack.run(.stash("Pop Stash", sha: sha, message: "On main: wip", applied: true, dropped: true)) {
        try await Git.run(["stash", "pop", "-q"], in: dir)
    }
    try await stack.undo()
    #expect(read("café.txt", dir) == nil)
    #expect(try await git("stash", "list", "--format=%H", in: dir) == sha)
}

@MainActor @Test func undoOfTheFirstCommitLeavesTheBranchUnbornAndStaged() async throws {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-undo-\(UUID().uuidString)").path
    try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    for args in [["init", "-q", "-b", "main"], ["config", "user.name", "t"], ["config", "user.email", "t@t"], ["config", "commit.gpgsign", "false"]] {
        try await Git.run(args, in: dir)
    }
    try write("one\n", "a.txt", dir)
    try await Git.run(["add", "a.txt"], in: dir)
    let stack = UndoStack(path: dir)
    try await stack.run(.reset("Commit", soft: true)) { try await Git.run(["commit", "-q", "-m", "first"], in: dir) }
    try await stack.undo()
    #expect(await UndoStack.oid(in: dir) == nil)
    #expect(try await git("symbolic-ref", "HEAD", in: dir) == "refs/heads/main")
    #expect(try await git("diff", "--cached", "--name-only", in: dir) == "a.txt")
}

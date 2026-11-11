import Testing
import Foundation
@testable import Loupe

// MARK: - Submodules

@Test func parsesSubmoduleEntriesFromStatusV2() {
    let raw = "# branch.oid abc\0# branch.head main\0"
        + "1 .M SCMU 160000 160000 160000 091e5df 091e5df libs/lib\0"
        + "1 M. SC.. 160000 160000 160000 111 222 vendor/other\0"
        + "1 .M N... 100644 100644 100644 aaa bbb src/a.txt\0"
    let files = Git.parseStatus(Data(raw.utf8)).files
    #expect(files.count == 3)
    #expect(files[0].path == "libs/lib" && !files[0].staged)
    #expect(files[0].submodule == SubmoduleStatus(commitChanged: true, modified: true, untracked: true))
    #expect(files[1].path == "vendor/other" && files[1].staged && files[1].submodule?.commitChanged == true && files[1].submodule?.modified == false)
    #expect(files[2].submodule == nil)
    #expect(SubmoduleStatus("S..U") == SubmoduleStatus(untracked: true))
    #expect(SubmoduleStatus("N...") == nil)
}

@Test func parsesSubmoduleLogDiffs() {
    let d = SubmoduleDiff.parse("""
    Submodule libs/my lib contains modified content
    Submodule libs/my lib 091e5df..51ffb32:
      > add b
      > add c
    """)
    #expect(d.old == "091e5df" && d.new == "51ffb32" && d.note == nil && d.modifiedContent && !d.untrackedContent)
    #expect(d.commits == [.init(added: true, subject: "add b"), .init(added: true, subject: "add c")])

    let rewind = SubmoduleDiff.parse("Submodule lib 51ffb32...091e5df (rewind):\n  < add b\n")
    #expect(rewind.old == "51ffb32" && rewind.new == "091e5df" && rewind.note == "rewind" && rewind.commits == [.init(added: false, subject: "add b")])

    let added = SubmoduleDiff.parse("Submodule lib 0000000...091e5df (new submodule)\n")
    #expect(added.old == nil && added.new == "091e5df" && added.note == "new submodule")

    let untracked = SubmoduleDiff.parse("Submodule lib contains untracked content\n")
    #expect(untracked.untrackedContent && untracked.old == nil && untracked.new == nil)
}

@Test func readsSubmodulePathsFromGitmodulesConfig() {
    #expect(Submodules.paths(fromConfig: "submodule.libs/lib.path libs/lib\nsubmodule.x.path vendor/x y\n") == ["libs/lib", "vendor/x y"])
    #expect(Submodules.paths(fromConfig: "").isEmpty)
}

@Test func ignoresObjectStoreChurnIncludingSubmodules() {
    #expect(FSWatcher.isNoise("/r/.git/objects/ab/cdef"))
    #expect(FSWatcher.isNoise("/r/.git/lfs/tmp/x"))
    #expect(FSWatcher.isNoise("/r/.git/modules/libs/lib/objects/ab/cd"))
    #expect(FSWatcher.isNoise("/r/.git/index.lock"))
    #expect(!FSWatcher.isNoise("/r/.git/modules/libs/lib/index"))
    #expect(!FSWatcher.isNoise("/r/src/objects/model.swift"))
}

@Test func findsHomebrewForGitEvenFromFinder() {
    #expect(Git.searchPath("/usr/bin:/bin") == "/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin")
    #expect(Git.searchPath("/opt/homebrew/bin:/usr/bin") == "/opt/homebrew/bin:/usr/bin:/usr/local/bin")
}

/// Real git: add a submodule, move it to a new commit, stage the pointer; a fresh clone has it uninitialized.
@Test func submoduleAddAndPointerChangeAgainstRealRepo() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-sm-\(UUID().uuidString)").path
    let lib = root + "/lib", sup = root + "/super", clone = root + "/clone"
    for d in [lib, sup] { try FileManager.default.createDirectory(atPath: d, withIntermediateDirectories: true) }
    defer { try? FileManager.default.removeItem(atPath: root) }
    func git(_ dir: String, _ args: String...) async throws {
        try await Git.run(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "protocol.file.allow=always", "-c", "init.defaultBranch=main"] + args, in: dir)
    }
    try await git(lib, "init", "-q")
    try "a\n".write(toFile: lib + "/a", atomically: true, encoding: .utf8)
    try await git(lib, "add", "a"); try await git(lib, "commit", "-qm", "add a")
    try await git(sup, "init", "-q"); try await git(sup, "commit", "-q", "--allow-empty", "-m", "init")
    try await git(sup, "submodule", "add", "-q", lib, "libs/lib")

    // Just added: a staged new submodule.
    let added = try #require(try await Git.status(in: sup).files.first { $0.path == "libs/lib" })
    #expect(added.staged && added.kind == .added && added.submodule != nil)
    try await git(sup, "commit", "-qm", "add sub")

    // A new commit inside it: the pointer changed, the diff lists the commit.
    let sub = sup + "/libs/lib"
    try "b\n".write(toFile: lib + "/b", atomically: true, encoding: .utf8)
    try await git(lib, "add", "b"); try await git(lib, "commit", "-qm", "add b")
    try await git(sub, "fetch", "-q", "origin"); try await git(sub, "checkout", "-q", "FETCH_HEAD")
    let moved = try #require(try await Git.status(in: sup).files.first { $0.path == "libs/lib" })
    #expect(!moved.staged && moved.submodule?.commitChanged == true && moved.submodule?.modified == false)
    let log = SubmoduleDiff.parse(try await Git.string(["diff", "--submodule=log", "--", "libs/lib"], in: sup))
    #expect(log.commits == [.init(added: true, subject: "add b")] && log.old != nil && log.new != nil)

    // Staging records the new pointer.
    try await git(sup, "add", "-A", "--", "libs/lib")
    let staged = try await Git.status(in: sup).files.filter { $0.path == "libs/lib" }
    #expect(staged.count == 1 && staged[0].staged && staged[0].submodule != nil)
    try await git(sup, "commit", "-qm", "bump lib")

    // Opened as a repository of its own: its worktree is the submodule, not its git dir under .git/modules.
    let wts = try await Git.worktrees(in: sub)
    let real = try #require(realpath(sub, nil))
    defer { free(real) }
    #expect(wts.first?.path == String(cString: real))
    // Asked from a linked worktree of the submodule, the main entry is still the submodule, not that worktree.
    try await git(sub, "worktree", "add", "-q", "--detach", root + "/sub-wt")
    let fromLinked = try await Git.worktrees(in: root + "/sub-wt")
    #expect(fromLinked.count == 2 && fromLinked.first?.path == String(cString: real))
    // Update to Recorded Commit would strand a commit made on a detached HEAD: it's counted.
    try await git(sub, "commit", "-q", "--allow-empty", "-m", "detached work")
    #expect(await Submodules.strandedCommits(in: sub) == 1)
    try await git(sub, "checkout", "-q", "-B", "work")
    #expect(await Submodules.strandedCommits(in: sub) == 0)
    try await git(sub, "checkout", "-q", "--detach", "HEAD~1")

    // A clone has it listed but not checked out, until it's initialized.
    try await git(root, "clone", "-q", sup, clone)
    let model = await SubmoduleModel(path: clone)
    await model.refresh()
    #expect(await model.uninitialized == ["libs/lib"])
    try await git(clone, "submodule", "update", "--init", "--recursive")
    await model.refresh()
    #expect(await model.uninitialized.isEmpty)
}

// MARK: - LFS

private let pointerA = """
version https://git-lfs.github.com/spec/v1
oid sha256:068aebc5295abffe3ce5319714d0066482c1e5fddb86be2771571ace534210e5
size 897
"""

@Test func parsesLFSPointers() throws {
    let p = try #require(LFSPointer(pointerA))
    #expect(p.size == 897 && p.oid.hasPrefix("068aebc5"))
    #expect(p.objectPath(commonDir: "/r/.git") == "/r/.git/lfs/objects/06/8a/068aebc5295abffe3ce5319714d0066482c1e5fddb86be2771571ace534210e5")
    #expect(LFSPointer("version 1\nsize 3\n") == nil)
    #expect(LFSPointer("version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 3\n") == nil)   // not a sha256
}

@Test func recognizesLFSPointerDiffs() throws {
    let changed = DiffParser.parse("""
    diff --git a/img.png b/img.png
    index bba3cf0..f8b2948 100644
    --- a/img.png
    +++ b/img.png
    @@ -1,3 +1,3 @@
     version https://git-lfs.github.com/spec/v1
    -oid sha256:068aebc5295abffe3ce5319714d0066482c1e5fddb86be2771571ace534210e5
    -size 897
    +oid sha256:4f5f6ea6bdb59175c8c0a654dd3aad54e349ea427c11ce5f7cc3f8a02e060a24
    +size 947

    """)
    let c = try #require(changed.first)
    let lfs = try #require(LFSDiff(c))
    #expect(lfs.old?.size == 897 && lfs.new?.size == 947 && lfs.new?.oid.hasPrefix("4f5f6e") == true)

    let added = DiffParser.parse("""
    diff --git a/a.bin b/a.bin
    new file mode 100644
    --- /dev/null
    +++ b/a.bin
    @@ -0,0 +1,3 @@
    +version https://git-lfs.github.com/spec/v1
    +oid sha256:068aebc5295abffe3ce5319714d0066482c1e5fddb86be2771571ace534210e5
    +size 897

    """)
    let ad = try #require(added.first)
    let a = try #require(LFSDiff(ad))
    #expect(a.old == nil && a.new?.size == 897)

    let text = DiffParser.parse("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\n")
    let t = try #require(text.first)
    #expect(LFSDiff(t) == nil)
}

@Test func detectsLFSInGitattributes() {
    #expect(LFS.usesLFS(gitattributes: "*.psd filter=lfs diff=lfs merge=lfs -text\n"))
    #expect(LFS.usesLFS(gitattributes: "*.txt text\n  assets/** filter=lfs -text\n"))
    #expect(!LFS.usesLFS(gitattributes: "# *.psd filter=lfs\n*.txt text eol=lf\n"))
    #expect(!LFS.usesLFS(gitattributes: ""))
    #expect(LFS.extensionPattern("art/logo.PSD") == "*.PSD")
    #expect(LFS.extensionPattern("Makefile") == nil)
}

@Test func parsesLFSLocks() {
    let json = #"[{"id":"1","path":"art/logo.psd","owner":{"name":"maya"},"locked_at":"2026-11-11T10:00:00Z"},{"id":"2","path":"b.bin"}]"#
    #expect(LFS.parseLocks(Data(json.utf8)) == ["art/logo.psd": "maya", "b.bin": "someone"])
    #expect(LFS.parseLocks(Data("[]".utf8)) == [:])
    #expect(LFS.parseLocks(Data("not json".utf8)) == nil)
}

/// Real git-lfs, when installed: a tracked file's diff is a pointer change, and the new object is on disk.
@Test(.enabled(if: LFS.installed)) func lfsPointerDiffAgainstRealRepo() async throws {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-lfs-\(UUID().uuidString)").path
    try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(atPath: dir) }
    func git(_ args: String...) async throws { try await Git.run(["-c", "user.name=t", "-c", "user.email=t@t"] + args, in: dir) }
    try await git("init", "-q")
    for (k, v) in [("clean", "git-lfs clean -- %f"), ("smudge", "git-lfs smudge -- %f"), ("process", "git-lfs filter-process"), ("required", "true")] {
        try await git("config", "filter.lfs.\(k)", v)
    }
    try await git("lfs", "track", "*.bin")
    try Data(repeating: 1, count: 500).write(to: URL(fileURLWithPath: dir + "/a.bin"))
    try await git("add", "."); try await git("commit", "-qm", "one")
    try Data(repeating: 2, count: 700).write(to: URL(fileURLWithPath: dir + "/a.bin"))

    let f = try #require(try await Git.status(in: dir).files.first { $0.path == "a.bin" })
    let d = try #require(try await Git.diff(for: f, in: dir))
    let lfs = try #require(LFSDiff(d))
    #expect(lfs.old?.size == 500 && lfs.new?.size == 700)
    let common = try await Git.string(["rev-parse", "--path-format=absolute", "--git-common-dir"], in: dir).trimmingCharacters(in: .whitespacesAndNewlines)
    #expect(FileManager.default.fileExists(atPath: lfs.old!.objectPath(commonDir: common)))

    let model = await LFSModel(path: dir)
    await model.refresh(env: [:])
    #expect(await model.used)
}

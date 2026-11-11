import Testing
import Foundation
@testable import Loupe

private func kinds(_ m: Merge3) -> [Merge3.Kind] { m.chunks.map(\.kind) }

/// The merge with each conflict taken from one side.
private func merged(_ m: Merge3, conflict: (Merge3.Chunk) -> ArraySlice<Substring>) -> String {
    m.chunks.flatMap { $0.kind == .conflict ? conflict($0) : m.merged($0) }.map { $0 + "\n" }.joined()
}

@Test func mergesChangesOnDifferentLinesWithoutConflicts() {
    let m = Merge3(base: "a\nb\nc\nd\ne\n", ours: "a\nB\nc\nd\ne\n", theirs: "a\nb\nc\nD\ne\nf\n")
    #expect(m.conflictCount == 0)
    #expect(kinds(m) == [.same, .ours, .same, .theirs, .same, .theirs])
    #expect(merged(m) { _ in [] } == "a\nB\nc\nD\ne\nf\n")
}

@Test func findsConflictsAndKeepsTheirBase() {
    let m = Merge3(base: "a\nb\nc\n", ours: "a\nours\nc\n", theirs: "a\ntheirs\nc\n")
    #expect(kinds(m) == [.same, .conflict, .same])
    let c = m.chunks[1]
    #expect(Array(m.base[c.base]) == ["b"] && Array(m.ours[c.ours]) == ["ours"] && Array(m.theirs[c.theirs]) == ["theirs"])
    #expect(merged(m) { m.theirs[$0.theirs] } == "a\ntheirs\nc\n")
}

@Test func sameChangeOnBothSidesIsNoConflict() {
    let m = Merge3(base: "a\nb\n", ours: "a\nx\n", theirs: "a\nx\n")
    #expect(kinds(m) == [.same, .both])
}

@Test func conflictsAreTrimmedOfLinesBothSidesAdded() {
    // Like git's zealous merge: the lines both sides agree on at a conflict's edges aren't part of it.
    let m = Merge3(base: "a\nold\nz\n", ours: "a\nshared\nmine\nend\nz\n", theirs: "a\nshared\nyours\nend\nz\n")
    #expect(kinds(m) == [.same, .both, .conflict, .both, .same])
    let c = m.chunks[2]
    #expect(Array(m.ours[c.ours]) == ["mine"] && Array(m.theirs[c.theirs]) == ["yours"] && Array(m.base[c.base]) == ["old"])
}

@Test func insertionsAtTheSamePlaceConflict() {
    let m = Merge3(base: "a\nb\n", ours: "a\nx\nb\n", theirs: "a\ny\nb\n")
    #expect(kinds(m) == [.same, .conflict, .same])
    #expect(m.chunks[1].base.isEmpty)
}

@Test func bothAddedMergesAgainstAnEmptyBase() {
    let m = Merge3(base: "", ours: "one\ntwo\n", theirs: "one\nthree\n")
    #expect(kinds(m) == [.both, .conflict])
    #expect(!m.noFinalNewline)
    #expect(Merge3(base: "a", ours: "b", theirs: "c").noFinalNewline)
}

@Test func matchesLinesKeptByADiff() {
    #expect(Merge3.matches(["a", "b", "c", "d"], ["a", "x", "c", "d", "e"]) == [0, nil, 2, 3])
    #expect(Merge3.matches(["a", "b"], []) == [nil, nil])
    #expect(Merge3.matches(["x", "a", "y"], ["a"]) == [nil, 0, nil])
}

@Test func parsesIndexStages() {
    let s = ConflictStages("100644 aaa 1\tf.txt\u{0}100644 bbb 2\tf.txt\u{0}100644 ccc 3\tf.txt\u{0}")
    #expect(s.kind == .text && s.entries[2]?.sha == "bbb")
    #expect(ConflictStages("100644 aaa 1\tf\u{0}100644 ccc 3\tf\u{0}").kind == .deletedByUs)
    #expect(ConflictStages("100644 aaa 1\tf\u{0}100644 bbb 2\tf\u{0}").kind == .deletedByThem)
    #expect(ConflictStages("100644 bbb 2\tf\u{0}100644 ccc 3\tf\u{0}").kind == .bothAdded)
    #expect(ConflictStages("100644 ccc 3\tf\u{0}").kind == .addedByThem)
    #expect(ConflictStages("160000 aaa 1\tsub\u{0}160000 bbb 2\tsub\u{0}160000 ccc 3\tsub\u{0}").kind == .submodule)
    #expect(ConflictStages("").kind == .none)
}

@Test func findsMarkerBlocksWithTheirRanges() {
    let text = "keep\n<<<<<<< HEAD\nmine\n||||||| base\nold\n=======\nyours\n>>>>>>> topic\ntail\n"
    let b = ConflictText.blocks(in: text)
    #expect(b.count == 1)
    let block = (text as NSString).substring(with: b[0].range)
    #expect(block.hasPrefix("<<<<<<< HEAD\n") && block.hasSuffix(">>>>>>> topic\n"))
    #expect(b[0].conflict.current == ["mine"] && b[0].conflict.base == ["old"] && b[0].conflict.incoming == ["yours"])
}

@Test func namesSidesForTheOperation() {
    let rebase = MergeLabels(operation: .rebase, branch: nil, subject: "abc Fix it")
    #expect(rebase.ours == "Upstream" && rebase.theirs == "Your Commit")
    let merge = MergeLabels(operation: .merge, branch: "main", subject: "def Feature")
    #expect(merge.ours == "Your Branch" && merge.oursDetail == "main" && merge.theirs == "Incoming")
}

@Test func lineLookupAndMarkerBlocks() {
    let st = MergeSession.lineStarts(["ab", "", "cde"])
    #expect(st == [0, 3, 4, 8])
    #expect(MergeSession.line(at: 0, in: st) == 0 && MergeSession.line(at: 3, in: st) == 1 && MergeSession.line(at: 7, in: st) == 2)
    let block = MergeSession.markerBlock(.init(ours: ["o"], base: ["b"], theirs: ["t"]), ("HEAD", "topic"))
    #expect(ConflictText(block.joined(separator: "\n") + "\n").conflictCount == 1)
    #expect(MergeAI.clean("```swift\n    let x = 1\n```\n") == "    let x = 1")
}

@Test func regionsFollowEdits() {
    let r = NSRange(location: 10, length: 5)
    let m = { (loc: Int, len: Int, delta: Int, owns: Bool) in MergeSession.moved(r, edit: NSRange(location: loc, length: len), delta: delta, owns: owns) }
    #expect(m(2, 3, 3, false) == NSRange(location: 13, length: 5))   // typed before it
    #expect(m(10, 1, 1, false) == NSRange(location: 11, length: 5))  // typed right at its start: before it
    #expect(m(15, 2, 2, false) == r)                                 // typed right after it
    #expect(m(12, 4, 4, false) == NSRange(location: 10, length: 9))  // typed inside it
    #expect(m(10, 2, -3, true) == NSRange(location: 10, length: 2))  // replaced by an action
    #expect(MergeSession.moved(NSRange(location: 4, length: 0), edit: NSRange(location: 4, length: 3), delta: 3, owns: true) == NSRange(location: 4, length: 3))
    #expect(m(8, 0, -4, false) == NSRange(location: 8, length: 3))   // a deletion eating its start
}

@Test func mergesARealConflictFromTheIndexStages() async throws {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-merge-\(UUID().uuidString)").path
    try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(atPath: dir) }
    func git(_ args: String...) async throws { try await Git.run(["-c", "user.name=t", "-c", "user.email=t@t"] + args, in: dir) }
    func write(_ name: String, _ s: String) throws { try s.write(toFile: dir + "/" + name, atomically: true, encoding: .utf8) }
    try await git("init", "-q", "-b", "main")
    try write("f.txt", "one\ntwo\nthree\nfour\nfive\nsix\nseven\n")
    try write("gone.txt", "base\n")
    try await git("add", ".")
    try await git("commit", "-qm", "base")
    try await git("switch", "-qc", "topic")
    try write("f.txt", "one\ntwo\nTHREE topic\nfour\nfive\nsix\nseven\neight\n")
    try write("gone.txt", "changed on topic\n")
    try await git("commit", "-qam", "topic")
    try await git("switch", "-q", "main")
    try write("f.txt", "ONE\ntwo\nthree main\nfour\nfive\nsix\nseven\n")
    try await git("rm", "-q", "gone.txt")
    try await git("commit", "-qam", "main")
    _ = try? await Git.run(["merge", "--no-edit", "topic"], in: dir, allowFailure: true)

    let stages = ConflictStages(try await Git.string(["ls-files", "-u", "-z", "--", "f.txt"], in: dir))
    #expect(stages.kind == .text)
    let show = { (n: Int) async throws in try await Git.string(["show", ":\(n):f.txt"], in: dir) }
    let m = Merge3(base: try await show(1), ours: try await show(2), theirs: try await show(3))
    #expect(m.conflictCount == 1)
    let disk = try String(contentsOfFile: dir + "/f.txt", encoding: .utf8)
    #expect(ConflictText.blocks(in: disk).count == m.conflictCount)   // ours lines up with git's own conflicts
    #expect(merged(m) { m.ours[$0.ours] + m.theirs[$0.theirs] } == "ONE\ntwo\nthree main\nTHREE topic\nfour\nfive\nsix\nseven\neight\n")
    // Built from the chunks with git's labels, the result is git's file: what merged on its own is the same text.
    let blocks = ConflictText.blocks(in: disk)
    let (lines, regions) = MergeSession.fromChunks(m, (blocks[0].conflict.currentLabel, blocks[0].conflict.incomingLabel))
    #expect(MergeSession.agree(disk: disk, blocks: blocks.map(\.range), result: lines, conflicts: regions))

    let gone = ConflictStages(try await Git.string(["ls-files", "-u", "-z", "--", "gone.txt"], in: dir))
    #expect(gone.kind == .deletedByUs)
}

@Test func crlfFilesSplitIntoLines() {
    // "\r\n" is one Character: splitting the String on "\n" would leave a CRLF file as one line, all of it one conflict.
    let m = Merge3(base: "a\r\nb\r\nc\r\nd\r\n", ours: "a\r\nB\r\nc\r\nd\r\n", theirs: "a\r\nb\r\nc\r\nD\r\n")
    #expect(m.conflictCount == 0 && m.base.count == 4)
    #expect(merged(m) { _ in [] } == "a\r\nB\r\nc\r\nD\r\n")
    let text = "x\r\n<<<<<<< HEAD\r\nmine\r\n=======\r\nyours\r\n>>>>>>> topic\r\ny\r\n"
    let t = ConflictText(text)
    #expect(t.conflictCount == 1)
    let b = ConflictText.blocks(in: text)
    #expect(b.count == 1 && b[0].conflict.incomingLabel == "topic" && b[0].conflict.current == ["mine\r"])
    #expect((text as NSString).substring(with: b[0].range).hasSuffix(">>>>>>> topic\r\n"))
}

@Test func editsOutsideTheConflictsKeepTheFileAsItIs() {
    let m = Merge3(base: "a\nx\nb\nc\nd\n", ours: "A\nx\nours\nc\nd\n", theirs: "a\nx\ntheirs\nc\nd\n")
    let (lines, regions) = MergeSession.fromChunks(m, ("HEAD", "topic"))
    let git = "A\nx\n<<<<<<< HEAD\nours\n||||||| base\nb\n=======\ntheirs\n>>>>>>> topic\nc\nd\n"
    let agree = { (disk: String) in MergeSession.agree(disk: disk, blocks: ConflictText.blocks(in: disk).map(\.range), result: lines, conflicts: regions) }
    #expect(agree(git))
    #expect(!agree(git.replacingOccurrences(of: "c\n", with: "c edited in vim\n")))   // a write would undo that edit
    #expect(!agree(git.replacingOccurrences(of: "\n", with: "\r\n")))                // autocrlf checkout: keep its line endings
    #expect(!agree("A\nx\nours\nc\nd\n"))                                               // resolved in an editor, or by rerere
}

@Test func strayMarkerLinesStillCountAsUnresolved() {
    #expect(MergeSession.hasMarkers("a\n<<<<<<< HEAD\nb\n"))
    #expect(MergeSession.hasMarkers(">>>>>>> topic\n"))
    #expect(!MergeSession.hasMarkers("Title\n=======\n<<<<<<<< not a marker\n"))
}

@Test func textTypedWhereAConflictResolvedToNothingIsItsOwn() {
    let empty = NSRange(location: 4, length: 0)
    #expect(MergeSession.moved(empty, edit: NSRange(location: 4, length: 6), delta: 6, owns: false) == NSRange(location: 4, length: 6))   // its undo
    #expect(MergeSession.moved(empty, edit: NSRange(location: 1, length: 2), delta: 2, owns: false) == NSRange(location: 6, length: 0))
}

@Test func writingTheResultKeepsTheFileExecutable() throws {
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-merge-\(UUID().uuidString).sh")
    defer { try? FileManager.default.removeItem(at: url) }
    try "#!/bin/sh\n".write(to: url, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
    try MergeSession.write("#!/bin/sh\necho hi\n", to: url)
    #expect((try FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as? Int) == 0o755)
    #expect(try String(contentsOf: url, encoding: .utf8) == "#!/bin/sh\necho hi\n")
}

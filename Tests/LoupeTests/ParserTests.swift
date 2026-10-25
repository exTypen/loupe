import Testing
import Foundation
import AppKit
@testable import Loupe

@Test func parsesDiffHunksAndLineNumbers() {
    let d = DiffParser.parse("""
    diff --git a/a.txt b/a.txt
    index 1..2 100644
    --- a/a.txt
    +++ b/a.txt
    @@ -1,3 +1,3 @@
     one
    -two
    +twO
     three
    @@ -10,2 +10,3 @@ ctx
     ten
    +new
     eleven
    diff --git a/old.txt b/new.txt
    similarity index 90%
    rename from old.txt
    rename to new.txt
    diff --git a/img.png b/img.png
    Binary files a/img.png and b/img.png differ
    """)
    #expect(d.count == 3)
    #expect(d[0].hunks.count == 2)
    #expect(d[0].additions == 2 && d[0].deletions == 1)
    let added = d[0].lines.first { $0.text == "new" }!
    #expect(added.newLine == 11 && added.oldLine == nil)
    #expect(d[0].patch(forHunk: 1).hasSuffix("@@ -10,2 +10,3 @@ ctx\n ten\n+new\n eleven\n"))
    #expect(d[1].oldPath == "old.txt" && d[1].newPath == "new.txt")
    #expect(d[2].isBinary)
    let hl = DiffTableView.intraLineHighlights(d[0])
    #expect(hl.count == 2)
}

@Test func parsesStatusV2() {
    let raw = "# branch.head main\0# branch.ab +2 -1\01 M. N... 100644 100644 100644 a b src/x.swift\01 .M N... 100644 100644 100644 a b y z.txt\02 R. N... 100644 100644 100644 a b R100 new.txt\0old.txt\0? untracked.md\0"
    let s = Git.parseStatus(Data(raw.utf8))
    #expect(s.branch == "main" && s.ahead == 2 && s.behind == 1)
    #expect(s.files.map(\.path) == ["src/x.swift", "y z.txt", "new.txt", "untracked.md"])
    #expect(s.files[0].staged && !s.files[1].staged)
    #expect(s.files[2].origPath == "old.txt" && s.files[2].kind == .renamed)
    #expect(s.files[3].kind == .untracked)
}

@Test func parsesWorktreesAndRemotes() {
    let w = Git.parseWorktrees("worktree /r\nHEAD abc\nbranch refs/heads/main\n\nworktree /r-feat\nHEAD def\ndetached\nlocked\n\n")
    #expect(w.count == 2 && w[0].branch == "main" && w[1].branch == nil && w[1].isLocked)
    let r = RemoteRepo(remoteURL: "git@git.example.com:acme/web.git"); #expect(r?.host == "git.example.com" && r?.owner == "acme" && r?.name == "web")
    #expect(RemoteRepo(remoteURL: "ssh://git@git.example.com:2222/acme/web.git")?.host == "git.example.com")
    #expect(RemoteRepo(remoteURL: "http://localhost:3300/loupe/demo.git")?.host == "localhost:3300")
}

@Test func stagesSingleHunkAgainstRealRepo() async throws {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-\(UUID().uuidString)").path
    try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(atPath: dir) }
    let file = dir + "/f.txt"
    try (1...60).map { "line \($0)" }.joined(separator: "\n").appending("\n").write(toFile: file, atomically: true, encoding: .utf8)
    try await Git.run(["init", "-q"], in: dir)
    try await Git.run(["-c", "user.name=t", "-c", "user.email=t@t", "add", "."], in: dir)
    try await Git.run(["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"], in: dir)
    var lines = (1...60).map { "line \($0)" }
    lines[2] = "CHANGED 3"; lines[50] = "CHANGED 51"
    try lines.joined(separator: "\n").appending("\n").write(toFile: file, atomically: true, encoding: .utf8)

    let status = try await Git.status(in: dir)
    let change = try #require(status.files.first)
    let diff = try #require(try await Git.diff(for: change, in: dir))
    #expect(diff.hunks.count == 2)
    try await Git.run(["apply", "--cached", "-"], in: dir, stdin: Data(diff.patch(forHunk: 1).utf8))

    let after = try await Git.status(in: dir).files
    #expect(after.count == 2)   // same file: staged + unstaged
    let staged = try #require(try await Git.diff(for: after.first { $0.staged }!, in: dir))
    #expect(staged.hunks.count == 1 && staged.lines.contains { $0.text == "CHANGED 51" })
    // Unstage it again via reverse apply of the staged hunk.
    try await Git.run(["apply", "--cached", "--reverse", "-"], in: dir, stdin: Data(staged.patch(forHunk: 0).utf8))
    #expect(try await Git.status(in: dir).files.filter(\.staged).isEmpty)
}

@Test func stagesAndDiscardsSingleLinesAgainstRealRepo() async throws {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-\(UUID().uuidString)").path
    try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(atPath: dir) }
    let file = dir + "/f.txt"
    let original = (1...10).map { "line \($0)" }
    try original.joined(separator: "\n").appending("\n").write(toFile: file, atomically: true, encoding: .utf8)
    try await Git.run(["init", "-q"], in: dir)
    try await Git.run(["-c", "user.name=t", "-c", "user.email=t@t", "add", "."], in: dir)
    try await Git.run(["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"], in: dir)
    // One hunk, four changes: line 4 and 5 replaced, two lines added after line 7.
    var edited = original
    edited[3] = "FOUR"; edited[4] = "FIVE"; edited.insert(contentsOf: ["NEW A", "NEW B"], at: 7)
    try edited.joined(separator: "\n").appending("\n").write(toFile: file, atomically: true, encoding: .utf8)
    let diffOf = { (staged: Bool) async throws -> FileDiff in
        let f = try #require(try await Git.status(in: dir).files.first { $0.staged == staged })
        return try #require(try await Git.diff(for: f, in: dir))
    }
    let index = { (d: FileDiff, kind: DiffLine.Kind, text: String) in d.lines.firstIndex { $0.kind == kind && $0.text == text }! }

    // Stage only "+NEW A": the index gets that line and nothing else.
    var d = try await diffOf(false)
    #expect(d.hunks.count == 1)
    try await Git.run(["apply", "--cached", "-"], in: dir, stdin: Data(try #require(d.patch(selecting: [index(d, .added, "NEW A")], reverse: false)).utf8))
    let indexed = try await Git.string(["show", ":f.txt"], in: dir)
    var expected = original; expected.insert("NEW A", at: 7)
    #expect(indexed == expected.joined(separator: "\n") + "\n")

    // Discard only "-line 4" / "+FOUR" from the working tree: line 4 comes back, the other edits stay.
    d = try await diffOf(false)
    let pair: Set<Int> = [index(d, .removed, "line 4"), index(d, .added, "FOUR")]
    try await Git.run(["apply", "--reverse", "-"], in: dir, stdin: Data(try #require(d.patch(selecting: pair, reverse: true)).utf8))
    var onDisk = edited; onDisk[3] = "line 4"
    #expect(try String(contentsOfFile: file, encoding: .utf8) == onDisk.joined(separator: "\n") + "\n")

    // Unstage "+NEW A" again.
    d = try await diffOf(true)
    try await Git.run(["apply", "--cached", "--reverse", "-"], in: dir, stdin: Data(try #require(d.patch(selecting: [index(d, .added, "NEW A")], reverse: true)).utf8))
    #expect(try await Git.status(in: dir).files.filter(\.staged).isEmpty)
    // Nothing selected that is a change → no patch.
    #expect(d.patch(selecting: [], reverse: false) == nil)
}

@Test func parsesLargeDiffQuickly() {
    // 500 files × 400 changed lines ≈ 400k diff lines, roughly a 20 MB PR.
    var s = ""
    for f in 0..<500 {
        s += "diff --git a/f\(f).swift b/f\(f).swift\nindex 1..2 100644\n--- a/f\(f).swift\n+++ b/f\(f).swift\n@@ -1,400 +1,400 @@\n"
        for i in 0..<200 { s += "-let value\(i) = compute(\(i), \"old\")\n+let value\(i) = compute(\(i), \"new\")\n" }
    }
    let t = ContinuousClock.now
    let files = DiffParser.parse(s)
    let elapsed = ContinuousClock.now - t
    #expect(files.count == 500 && files[0].lines.count == 401)
    print("parsed \(files.reduce(0) { $0 + $1.lines.count }) lines in \(elapsed)")
    #expect(elapsed < .seconds(2))
}

@Test func buildsCompressedCollapsibleTree() {
    let paths = ["central_api/src/services/authn.rs", "README.md", "central_api/src/lib.rs", "central_api/src/services/x.rs"]
        .sorted(by: FileTree.treeOrder)
    #expect(paths == ["central_api/src/services/authn.rs", "central_api/src/services/x.rs", "central_api/src/lib.rs", "README.md"])
    let rows = FileTree.rows(paths, collapsed: [])
    #expect(rows.map(\.name) == ["central_api/src", "services", "authn.rs", "x.rs", "lib.rs", "README.md"])
    #expect(rows.map(\.depth) == [0, 1, 2, 2, 1, 0])
    let folded = FileTree.rows(paths, collapsed: ["central_api/src/services"])
    #expect(folded.map(\.name) == ["central_api/src", "services", "lib.rs", "README.md"])
    #expect(FileTree.rows(paths, collapsed: ["central_api/src"]).count == 2)
}

@Test func parsesLog() {
    let raw = "abc\u{1f}p1 p2\u{1f}Ann\u{1f}a@x\u{1f}1700000000\u{1f}HEAD -> refs/heads/main, tag: refs/tags/v1\u{1f}Merge it\u{1e}\ndef\u{1f}\u{1f}Bob\u{1f}b@x\u{1f}1600000000\u{1f}\u{1f}root\u{1e}\n"
    let c = Git.parseLog(Data(raw.utf8))
    #expect(c.count == 2 && c[0].parents == ["p1", "p2"] && c[0].refs.count == 2 && c[1].parents.isEmpty && c[1].subject == "root")
}

@Test func mapsFileIcons() {
    #expect(FileIcons.iconName(for: "central_api/src/lib.rs") == "rust")
    #expect(FileIcons.iconName(for: "flake.nix") == "nix" && FileIcons.iconName(for: "flake.lock") == "nix")
    #expect(FileIcons.iconName(for: "user_console/src/Panel.test.tsx") == "test-jsx")
    #expect(FileIcons.iconName(for: "web/Panel.tsx") == "react_ts")
    #expect(FileIcons.iconName(for: "Cargo.toml") == "rust" && FileIcons.iconName(for: ".env.local") == "tune")
    #expect(FileIcons.iconName(for: "data/migrations/x/up.sql") == "database")
    #expect(FileIcons.iconName(for: "weird.unknownext") == "file")
    // Every mapped name must exist in the generated icon data.
    for p in ["a.rs", "a.nix", "a.ts", "a.tsx", "a.md", "a.yaml", "a.sql", "a.feature", "Dockerfile", "a.png", "a.sh", "a.json"] {
        #expect(FileIconData.svg[FileIcons.iconName(for: p)] != nil, "\(p)")
    }
}

@MainActor @Test func formatsActionLogs() {
    let raw = "2026-10-19T14:33:38.5145900Z ##[group]Getting Git version info\n2026-10-19T14:33:38.5145900Z git version 2.50\n2026-10-19T14:33:38.5145900Z ##[endgroup]\n2026-10-19T14:33:39.0Z ⭐ Run Main Build\n2026-10-19T14:33:40.0Z error[E0308]: mismatched types\n2026-10-19T14:33:41.0Z   ❌  Failure - Main Build\n"
    let out = JobLogModel.format(raw)
    #expect(out.string == "▸ Getting Git version info\ngit version 2.50\n⭐ Run Main Build\nerror[E0308]: mismatched types\n  ❌  Failure - Main Build\n")
    let problems = JobLogModel.findProblems(out, base: 0)
    #expect(problems.map(\.line) == ["error[E0308]: mismatched types", "❌  Failure - Main Build"])
    #expect(CheckState(status: "pending", description: "Has started running") == .running)
    #expect(CheckState(status: "pending", description: "Blocked by required conditions") == .pending)
}

@Test func tokenizesRust() {
    var st = Syntax.State()
    let line = #"    let x: Vec<&'a str> = format!("hi {}", 42); // done"#
    let spans = Syntax.tokenize(line, .rust, &st)
    func kinds(_ word: String) -> TokenKind? {
        let u = Array(line.utf16), w = Array(word.utf16)
        guard let at = (0...(u.count - w.count)).first(where: { Array(u[$0..<$0 + w.count]) == w }) else { return nil }
        return spans.first { Int($0.start) == at }?.kind
    }
    #expect(kinds("let") == .keyword)
    #expect(kinds("Vec") == .type)
    #expect(kinds("'a") == .attribute)
    #expect(kinds("format!") == .macro)
    #expect(kinds("\"hi {}\"") == .string)
    #expect(kinds("42") == .number)
    #expect(kinds("// done") == .comment)
}

@Test func carriesBlockCommentsPerSide() {
    let f = DiffParser.parse("diff --git a/a.ts b/a.ts\n--- a/a.ts\n+++ b/a.ts\n@@ -1,2 +1,2 @@\n-/* old open\n+const x = 1\n end */ const y = 2\n")[0]
    let s = Syntax.highlight(f, language: .typescript)
    #expect(s[1].first?.kind == .comment)                    // removed line opens a comment
    #expect(s[2].first?.kind == .keyword)                    // added line is unaffected by it
    #expect(s[3].first?.kind == .comment)                    // context continues the old side's comment
}

@Test func highlightsLargeFileQuickly() {
    var text = "diff --git a/big.rs b/big.rs\n--- /dev/null\n+++ b/big.rs\n@@ -0,0 +1,150000 @@\n"
    for i in 0..<150_000 { text += "+    let value_\(i) = compute(\(i), \"x\"); // note\n" }
    let f = DiffParser.parse(text)[0]
    let t = ContinuousClock.now
    let spans = Syntax.highlight(f, language: .rust)
    let elapsed = ContinuousClock.now - t
    print("highlighted \(spans.count) lines in \(elapsed)")
    #expect(spans.count == f.lines.count)
    #expect(elapsed < .seconds(3))
}


@MainActor @Test func selectsTextByDragWordAndLine() {
    let f = DiffParser.parse("diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,3 +1,3 @@\n let alpha = 1;\n-let beta_two = 2;\n+let gamma = 3;\n")[0]
    let c = DiffTableView.Coordinator()
    c.parent = DiffTableView(file: f)
    let table = DiffTable(frame: NSRect(x: 0, y: 0, width: 900, height: 400))
    table.addTableColumn(NSTableColumn(identifier: .init("diff")))
    table.dataSource = c; table.delegate = c; table.coordinator = c
    c.table = table
    c.rebuild(file: f, layout: .unified, comments: [], hunkActions: [])
    table.tableColumns[0].width = 900
    table.reloadData()
    // rows: 0 hunk, 1 " let alpha", 2 "-let beta_two", 3 "+let gamma"
    let x0 = c.textX(side: 1), cw = Theme.charWidth
    func pt(row: Int, col: Double) -> NSPoint { NSPoint(x: x0 + CGFloat(col) * cw, y: table.rect(ofRow: row).midY) }

    c.mouseDown(pt(row: 2, col: 6), clicks: 2)                 // inside "beta_two"
    #expect(c.selectedText() == "beta_two")
    c.mouseDown(pt(row: 3, col: 0), clicks: 3)
    #expect(c.selectedText() == "let gamma = 3;")
    c.mouseDown(pt(row: 1, col: 4), clicks: 1)                 // drag from "alpha" to the middle of row 3
    c.mouseDragged(pt(row: 3, col: 9))
    #expect(c.selectedText() == "alpha = 1;\nlet beta_two = 2;\nlet gamma")
    c.mouseDown(pt(row: 1, col: 4), clicks: 1)                 // plain click clears
    #expect(c.selectedText() == nil)
}

@MainActor @Test func dismissedApprovalsBecomePendingAfterPush() throws {
    // A PR after a new push: earlier approvals dismissed, alice re-requested.
    let prJSON = #"{"id":1,"number":7347,"title":"t","state":"open","user":{"login":"yasir"},"head":{"ref":"h","sha":"79eb"},"base":{"ref":"main","sha":"b"},"html_url":"x","updated_at":"x","requested_reviewers":[{"login":"alice"},{"login":"carol"}]}"#
    let reviewsJSON = #"[{"id":86195,"user":{"login":"alice"},"state":"APPROVED","body":"","stale":true,"dismissed":true},{"id":86199,"user":{"login":"bob"},"state":"APPROVED","body":"","stale":true,"dismissed":true},{"id":86202,"user":{"login":"alice"},"state":"REQUEST_REVIEW","body":"","stale":false,"dismissed":false},{"id":90000,"user":{"login":"carol"},"state":"APPROVED","body":"","stale":false,"dismissed":false}]"#
    let pr = try JSONDecoder().decode(PullRequest.self, from: Data(prJSON.utf8))
    let m = PullRequestModel(pr: pr, client: ForgejoClient(baseURL: "http://x", token: ""), repo: RemoteRepo(remoteURL: "git@x:o/r.git")!, local: LocalCheckout(dir: "/tmp", remote: "origin"))
    m.reviews = try JSONDecoder().decode([Review].self, from: Data(reviewsJSON.utf8))
    let states = Dictionary(uniqueKeysWithValues: m.reviewerStates.map { ($0.login, $0.state) })
    #expect(states["alice"] == .pending)
    #expect(states["bob"] == nil)          // dismissed and not re-requested: not shown
    #expect(states["carol"] == .approved)      // a fresh approval still counts, though Forgejo lists carol as requested
}

@Test func switcherFiltersByTokensAndOrdersByRecency() {
    let a = SwitcherItem(repo: "/r/shop", wt: Worktree(path: "/r/shop", branch: "main"))
    let b = SwitcherItem(repo: "/r/shop", wt: Worktree(path: "/r/shop-checkout", branch: "feat/checkout"))
    let c = SwitcherItem(repo: "/r/loupe", wt: Worktree(path: "/r/loupe", branch: "main"))
    #expect(switcherMatches([a, b, c], query: "", recent: ["/r/loupe", "/r/shop-checkout"]) == [c, b, a])
    #expect(switcherMatches([a, b, c], query: "shop main", recent: []) == [a])
    #expect(switcherMatches([a, b, c], query: "CHECK", recent: []) == [b])
}

@Test func parsesAndResolvesConflictMarkers() {
    let file = "a\n<<<<<<< HEAD\nmine\n||||||| base\nold\n=======\ntheirs\n>>>>>>> abc123 (fix)\nb\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> abc123 (fix)\n"
    let t = ConflictText(file)
    #expect(t.conflictCount == 2)
    guard case .conflict(let c) = t.segments[1] else { Issue.record("no conflict"); return }
    #expect(c.current == ["mine"] && c.base == ["old"] && c.incoming == ["theirs"])
    #expect(c.currentLabel == "HEAD" && c.incomingLabel == "abc123 (fix)")
    let once = t.resolving(0, with: c.incoming)
    #expect(once == "a\ntheirs\nb\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> abc123 (fix)\n")   // the other conflict survives verbatim
    #expect(ConflictText(once).resolving(0, with: ["x", "y"]) == "a\ntheirs\nb\nx\ny\n")
    #expect(ConflictText("no markers\n").conflictCount == 0)
}

@Test func laysOutGraphLanes() {
    func c(_ s: String, _ p: [String]) -> Commit { Commit(sha: s, parents: p, author: "", email: "", date: .now, refs: [], subject: "") }
    // M merges F into A; F branches off B.   M─┬─A─B─R
    //                                           └─F─┘
    let rows = Graph.layout([c("M", ["A", "F"]), c("A", ["B"]), c("F", ["B"]), c("B", ["R"]), c("R", [])], main: "M")
    #expect(rows[0] == GraphRow(col: 0, through: [], into: [], out: [0, 1]))
    #expect(rows[1] == GraphRow(col: 0, through: [1], into: [0], out: [0]))
    #expect(rows[2] == GraphRow(col: 1, through: [0], into: [1], out: [0]))   // F joins the lane already waiting for B
    #expect(rows[3] == GraphRow(col: 0, through: [], into: [0], out: [0]))
    #expect(rows[4].out.isEmpty)
    // The newest commit is on a side branch X off B: the main line still gets lane 0, X takes lane 1.
    let side = Graph.layout([c("X", ["B"]), c("M", ["B"]), c("B", [])], main: "M")
    #expect(side.map(\.col) == [1, 0, 0])
    #expect(side[0].out == [1] && side[1].through == [1] && side[2].into == [0, 1])
    // origin/main two commits ahead of main (M): they run in lane 1 and end in M, which stays in lane 0.
    let ahead = Graph.layout([c("O2", ["O1"]), c("O1", ["M"]), c("M", ["B"]), c("B", [])], main: "M")
    #expect(ahead.map(\.col) == [1, 1, 0, 0])
    #expect(ahead[2].into == [1] && ahead[2].out == [0])
}

@Test func blamesAndFollowsFileHistoryAgainstRealRepo() async throws {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-\(UUID().uuidString)").path
    try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(atPath: dir) }
    let id = ["-c", "user.name=alice", "-c", "user.email=a@x"]
    try "one\ntwo\n".write(toFile: dir + "/a.txt", atomically: true, encoding: .utf8)
    try await Git.run(["init", "-q"], in: dir)
    try await Git.run(["add", "."], in: dir)
    try await Git.run(id + ["commit", "-qm", "first"], in: dir)
    try await Git.run(["mv", "a.txt", "b.txt"], in: dir)
    try "one\nTWO\n".write(toFile: dir + "/b.txt", atomically: true, encoding: .utf8)
    try await Git.run(id + ["commit", "-qam", "rename and change"], in: dir)
    let b = try await Git.blame("b.txt", rev: nil, in: dir)
    #expect(b.lines.map(\.text) == ["one", "TWO"])
    #expect(b.commits[b.lines[0].sha]?.summary == "first" && b.commits[b.lines[1].sha]?.summary == "rename and change")
    #expect(b.commits[b.lines[0].sha]?.author == "alice")
    let log = try await Git.fileLog("b.txt", in: dir)
    #expect(log.map(\.commit.subject) == ["rename and change", "first"])
    #expect(log.map(\.path) == ["b.txt", "a.txt"])   // followed across the rename
}

@Test func cleansAIOutput() {
    #expect(CommitAI.clean("\u{1B}[1mFix the parser\u{1B}[0m\n") == "Fix the parser")
    #expect(CommitAI.clean("```\nAdd login\n\nWith tests\n```") == "Add login\n\nWith tests")
    #expect(CommitAI.clean("\"Bump version\"") == "Bump version")
}

@MainActor @Test func reassignsShortcutsAndResolvesClashes() {
    let s = Shortcuts.shared
    s.reset(); defer { s.reset() }
    #expect(s.shortcut(.toggleSidebar)?.display == "⇧⌘S")
    #expect(s.shortcut(.nextFile)?.display == "⌥↓")
    // Giving History the Changes shortcut takes it from Changes.
    let taken = s.set(Shortcut("1"), for: .historyTab)
    #expect(taken == .changesTab && s.shortcut(.changesTab) == nil && s.shortcut(.historyTab) == Shortcut("1"))
    s.set(nil, for: .refresh)
    #expect(s.shortcut(.refresh) == nil && s.isCustom(.refresh))
    s.set(ShortcutAction.refresh.standard, for: .refresh)   // back to the default: no longer custom
    #expect(!s.isCustom(.refresh) && s.shortcut(.refresh) == Shortcut("r"))
}

@Test func numbersConflictsFromZero() {
    let t = ConflictText("a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> b\nc\n<<<<<<< HEAD\np\n=======\nq\n>>>>>>> b\n")
    #expect(t.numbered.compactMap { if case .conflict = $0.segment { $0.conflict } else { nil } } == [0, 1])
}

@Test func findsDefinitionsWithTreeSitter() {
    let rust = "struct Foo;\nimpl Foo { fn bar(&self) {} }\n// helper is mentioned here\nfn helper() { let f = Foo; f.bar(); }\n"
    let defs = CodeIndex.outline(rust, path: "src/lib.rs")
    #expect(defs.map(\.name) == ["Foo", "bar", "helper"])
    #expect(defs.map(\.line) == [1, 2, 4])
    let ts = "export function encode() {}\nclass Preset { name = '' }\ninterface Opts { a: number }\n"
    #expect(Set(CodeIndex.outline(ts, path: "a.ts").map(\.name)).isSuperset(of: ["encode", "Preset", "Opts"]))
    #expect(CodeIndex.symbols(in: "x: 1", path: "a.yaml").isEmpty)   // no grammar: nothing, no crash
}

@Test func navigatesAtACommitWithoutCheckout() async throws {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-\(UUID().uuidString)").path
    try FileManager.default.createDirectory(atPath: dir + "/src", withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(atPath: dir) }
    try "pub fn fit(edge: u32) -> u32 { edge }\n".write(toFile: dir + "/src/fit.rs", atomically: true, encoding: .utf8)
    try "use crate::fit::fit;\n// fit is great\nfn main() { fit(3); }\n".write(toFile: dir + "/src/main.rs", atomically: true, encoding: .utf8)
    try await Git.run(["init", "-q"], in: dir)
    try await Git.run(["add", "."], in: dir)
    try await Git.run(["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "x"], in: dir)
    let head = try await Git.string(["rev-parse", "HEAD"], in: dir).trimmingCharacters(in: .whitespacesAndNewlines)
    try FileManager.default.removeItem(atPath: dir + "/src/fit.rs")   // the worktree doesn't matter: reads go to the commit
    let defs = await CodeNavigator.definitions(of: "fit", at: head, in: dir, from: "src/main.rs")
    #expect(defs.map(\.path) == ["src/fit.rs"] && defs.first?.line == 1)
    let refs = await CodeNavigator.references(of: "fit", at: head, in: dir)
    #expect(refs.count == 4 && refs.contains { $0.path == "src/main.rs" && $0.line == 3 })
}

@Test func replaysMigrationsIntoASchemaDiff() {
    let m1 = """
    CREATE TABLE public.users (id uuid PRIMARY KEY, email text NOT NULL, name text);
    CREATE TABLE "lanes" (id bigint, state text DEFAULT 'open', CONSTRAINT lanes_pkey PRIMARY KEY (id));
    CREATE UNIQUE INDEX users_email_idx ON users (lower(email));
    CREATE TYPE role AS ENUM ('admin', 'member');
    -- a comment; with a semicolon
    CREATE FUNCTION touch() RETURNS trigger AS $$ BEGIN NEW.updated_at = now(); RETURN NEW; END; $$ LANGUAGE plpgsql;
    """
    let m2 = """
    ALTER TABLE ONLY users ADD COLUMN IF NOT EXISTS role role NOT NULL DEFAULT 'member', DROP COLUMN name;
    ALTER TABLE users ALTER COLUMN email TYPE varchar(320);
    ALTER TYPE role ADD VALUE 'owner';
    DROP TABLE IF EXISTS lanes CASCADE;
    CREATE TABLE audit (id bigint, user_id uuid REFERENCES users(id));
    ALTER TABLE audit RENAME TO audit_events;
    DO $$ BEGIN RAISE NOTICE 'x'; END $$;
    """
    let a = SQL.replay([("m/001/up.sql", m1)])
    let b = SQL.replay([("m/001/up.sql", m1), ("m/002/up.sql", m2)])
    #expect(a.tables.keys.sorted() == ["lanes", "users"] && a.functions["touch"] != nil)
    #expect(a.tables["lanes"]?.primaryKey == ["id"] && a.tables["users"]?.columns.first?.notNull == true)
    let facts = SQLLens.diff(a, b)
    func has(_ kind: LensFact.Kind, _ title: String, _ detail: String) -> Bool { facts.contains { $0.kind == kind && $0.title == title && ($0.detail ?? "").contains(detail) } }
    #expect(has(.added, "audit_events", "table added"))
    #expect(has(.removed, "lanes", "table dropped"))
    #expect(has(.added, "users.role", "role NOT NULL DEFAULT 'member'"))
    #expect(has(.removed, "users.name", "column dropped"))
    #expect(has(.changed, "users.email", "type text → varchar(320)"))
    #expect(has(.changed, "role", "+ owner"))
    #expect(b.skipped.map(\.kind) == ["DO block"])
    #expect(facts.first { $0.title == "users.role" }?.line == 1)   // points at the ALTER in m/002
}

@Test func reportsSeedDataChangesPerTable() {
    let a = ["seed.sql": "INSERT INTO lanes (id) VALUES (1);\nINSERT INTO public.users (id) VALUES (1);\nUPDATE users SET x = 1;"]
    let b = ["seed.sql": "INSERT INTO public.users (id) VALUES (2);\nUPDATE users SET x = 1;\nDELETE FROM jobs;"]
    let f = SQLLens.dataChanges(["seed.sql"], a: a, b: b)
    #expect(f.contains { $0.kind == .removed && $0.title == "INSERT into lanes" })
    #expect(f.contains { $0.kind == .changed && $0.title == "INSERT into users" })
    #expect(f.contains { $0.kind == .added && $0.title == "DELETE from jobs" })
    #expect(!f.contains { $0.title.hasPrefix("UPDATE") })   // unchanged
}

@Test func readsDependencyFilesAndBumps() {
    let lock = "[[package]]\nname = \"serde\"\nversion = \"1.0.200\"\n\n[[package]]\nname = \"tokio\"\nversion = \"1.40.0\"\n"
    #expect(DependencyLens.parse("Cargo.lock", lock) == ["serde": "1.0.200", "tokio": "1.40.0"])
    let toml = "[package]\nname = \"x\"\n[dependencies]\nserde = { version = \"1.0\", features = [\"derive\"] }\nanyhow = \"1\"\n[dev-dependencies]\ninsta = \"1.3\"\n"
    #expect(DependencyLens.parse("Cargo.toml", toml) == ["serde": "1.0", "anyhow": "1", "insta (dev)": "1.3"])
    #expect(DependencyLens.parse("package.json", #"{"dependencies":{"react":"^19.1.0"},"devDependencies":{"vite":"^6.0.3"}}"#) == ["react": "^19.1.0", "vite (dev)": "^6.0.3"])
    #expect(DependencyLens.parse("go.mod", "module m\nrequire (\n\tgolang.org/x/net v0.30.0 // indirect\n)\nrequire github.com/a/b v1.2.3\n") == ["golang.org/x/net": "v0.30.0", "github.com/a/b": "v1.2.3"])
    #expect(DependencyLens.bump("4.17.0", "4.18.1") == "minor" && DependencyLens.bump("^1.2.3", "^2.0.0") == "major" && DependencyLens.bump("1.2.3", "1.2.0") == "downgrade")
}

@Test func flattensYAMLAndHidesSecrets() {
    let y = """
    name: ci
    on:
      push:
        paths:
          - "src/**"
    jobs:
      build:
        runs-on: linux
        steps:
          - uses: actions/checkout@v4
          - name: test
            run: |
              cargo test
              cargo clippy
    """
    let f = ConfigLens.yaml(y)
    #expect(f["name"] == "ci" && f["on.push.paths[0]"] == "\"src/**\"" && f["jobs.build.runs-on"] == "linux")
    #expect(f["jobs.build.steps[0].uses"] == "actions/checkout@v4" && f["jobs.build.steps[1].name"] == "test")
    #expect(f["jobs.build.steps[1].run"] == "cargo test\ncargo clippy")
    #expect(ConfigLens.secret("jobs.x.with.token") && !ConfigLens.secret("jobs.design-tokens.steps[3].run"))
}

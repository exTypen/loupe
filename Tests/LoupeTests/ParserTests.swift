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
    let prJSON = #"{"id":1,"number":7347,"title":"t","state":"open","user":{"login":"yasir"},"head":{"ref":"h","sha":"79eb"},"base":{"ref":"main","sha":"b"},"html_url":"x","updated_at":"x","requested_reviewers":[{"login":"alice"}]}"#
    let reviewsJSON = #"[{"id":86195,"user":{"login":"alice"},"state":"APPROVED","body":"","stale":true,"dismissed":true},{"id":86199,"user":{"login":"bob"},"state":"APPROVED","body":"","stale":true,"dismissed":true},{"id":86202,"user":{"login":"alice"},"state":"REQUEST_REVIEW","body":"","stale":false,"dismissed":false},{"id":90000,"user":{"login":"carol"},"state":"APPROVED","body":"","stale":false,"dismissed":false}]"#
    let pr = try JSONDecoder().decode(PullRequest.self, from: Data(prJSON.utf8))
    let m = PullRequestModel(pr: pr, client: ForgejoClient(baseURL: "http://x", token: ""), repo: RemoteRepo(remoteURL: "git@x:o/r.git")!, local: LocalCheckout(dir: "/tmp", remote: "origin"))
    m.reviews = try JSONDecoder().decode([Review].self, from: Data(reviewsJSON.utf8))
    let states = Dictionary(uniqueKeysWithValues: m.reviewerStates.map { ($0.login, $0.state) })
    #expect(states["alice"] == .pending)
    #expect(states["bob"] == nil)          // dismissed and not re-requested: not shown
    #expect(states["carol"] == .approved)      // a fresh approval still counts
}

@Test func switcherFiltersByTokensAndOrdersByRecency() {
    let a = SwitcherItem(repo: "/r/shop", wt: Worktree(path: "/r/shop", branch: "main"))
    let b = SwitcherItem(repo: "/r/shop", wt: Worktree(path: "/r/shop-checkout", branch: "feat/checkout"))
    let c = SwitcherItem(repo: "/r/loupe", wt: Worktree(path: "/r/loupe", branch: "main"))
    #expect(switcherMatches([a, b, c], query: "", recent: ["/r/loupe", "/r/shop-checkout"]) == [c, b, a])
    #expect(switcherMatches([a, b, c], query: "shop main", recent: []) == [a])
    #expect(switcherMatches([a, b, c], query: "CHECK", recent: []) == [b])
}

import Testing
import Foundation
@testable import Loupe

@Test func newPullBodyMarksDraftsPerServer() throws {
    let gh = ForgejoClient.newPull(head: "feat/x", base: "main", title: " Add x ", body: "b", draft: true, github: true)
    #expect(gh == .init(head: "feat/x", base: "main", title: "Add x", body: "b", draft: true))
    let ghJSON = String(decoding: try JSONEncoder().encode(ForgejoClient.newPull(head: "h", base: "m", title: "t", body: "", draft: false, github: true)), as: UTF8.self)
    #expect(!ghJSON.contains("draft"))   // not a draft: the field is left out

    let fj = ForgejoClient.newPull(head: "feat/x", base: "main", title: "Add x", body: "b", draft: true, github: false)
    #expect(fj.title == "WIP: Add x" && fj.draft == nil)
    #expect(ForgejoClient.newPull(head: "h", base: "m", title: "[WIP] Add x", body: "", draft: true, github: false).title == "[WIP] Add x")
    #expect(ForgejoClient.newPull(head: "h", base: "m", title: "Add x", body: "", draft: false, github: false).title == "Add x")
}

@Test func createErrorsTellAnExistingPullApart() {
    let gh = Data(#"{"message":"Validation Failed","errors":[{"resource":"PullRequest","code":"custom","message":"A pull request already exists for acme:feat."}]}"#.utf8)
    #expect(ForgejoClient.createError(status: 422, data: gh, server: "GitHub") is ForgejoClient.PullExists)
    let fj = Data(#"{"message":"pull request already exists for these targets [id: 3, issue_id: 7, head_repo_id: 1, base_repo_id: 1, head_branch: feat, base_branch: main]"}"#.utf8)
    #expect(ForgejoClient.createError(status: 409, data: fj, server: "Forgejo") is ForgejoClient.PullExists)
    let none = Data(#"{"message":"Validation Failed","errors":[{"resource":"PullRequest","code":"custom","message":"No commits between main and feat"}]}"#.utf8)
    let e = ForgejoClient.createError(status: 422, data: none, server: "GitHub")
    #expect(!(e is ForgejoClient.PullExists))
    #expect(e.localizedDescription == "GitHub 422: No commits between main and feat")
}

@Test func pullTitleFromOneCommitOrTheBranch() {
    let one = [PullDraft.Commit(sha: "a", subject: "Fix the login race", body: "Details.")]
    #expect(PullDraft.title(commits: one, branch: "fix/login") == "Fix the login race")
    #expect(PullDraft.body(commits: one, template: nil) == "Details.")
    #expect(PullDraft.body(commits: one, template: "## Why") == "## Why")
    let two = one + [PullDraft.Commit(sha: "b", subject: "Test it", body: "")]
    #expect(PullDraft.title(commits: two, branch: "feat/add-login_page") == "Add login page")
    #expect(PullDraft.body(commits: two, template: nil) == "")
    #expect(PullDraft.humanize("JIRA-12--tidy") == "JIRA 12 tidy")
    #expect(PullDraft.humanize("main") == "Main")
}

@Test func parsesCommitLog() {
    let log = "aaa\u{1f}First\u{1f}\u{1e}\nbbb\u{1f}Second\u{1f}Line one\nLine two\n\u{1e}\n"
    let c = PullDraft.parseLog(log)
    #expect(c.map(\.sha) == ["aaa", "bbb"])
    #expect(c[0].body == "" && c[1].subject == "Second" && c[1].body == "Line one\nLine two")
    #expect(PullDraft.parseLog("").isEmpty)
}

@Test func findsThePullRequestTemplateTheForgeWouldUse() throws {
    let dir = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-tpl-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: dir) }
    func write(_ p: String, _ s: String) throws {
        let u = dir.appendingPathComponent(p)
        try FileManager.default.createDirectory(at: u.deletingLastPathComponent(), withIntermediateDirectories: true)
        try s.write(to: u, atomically: true, encoding: .utf8)
    }
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    #expect(PullDraft.template(in: dir.path, github: true) == nil)
    try write("docs/pull_request_template.md", "docs")
    #expect(PullDraft.template(in: dir.path, github: false) == "docs")
    try write(".github/pull_request_template.md", "github\n")
    try write(".gitea/PULL_REQUEST_TEMPLATE.md", "gitea")
    #expect(PullDraft.template(in: dir.path, github: true) == "github")
    #expect(PullDraft.template(in: dir.path, github: false) == "gitea")
    try write(".forgejo/pull_request_template.md", "   ")   // empty: skipped
    #expect(PullDraft.template(in: dir.path, github: false) == "gitea")
    #expect(PullDraft.templatePaths(github: false).first == ".forgejo/pull_request_template.md")
}

@Test func splitsTheAIReply() {
    #expect(PullDraft.splitReply("Add login\n\n- does x\n- does y") == ("Add login", "- does x\n- does y"))
    #expect(PullDraft.splitReply("# \"Add login\"") == ("Add login", ""))
    #expect(PullDraft.splitReply("Title: Add login\nBody") == ("Add login", "Body"))
    let p = PullDraft.prompt(commits: [.init(sha: "a", subject: "S", body: "B")], base: "main", branch: "feat/x",
                             stat: "1 file", diff: String(repeating: "x", count: 70_000), template: "## Why")
    #expect(p.contains("[diff truncated]") && p.contains("## Why") && p.contains("S\n    B"))
}

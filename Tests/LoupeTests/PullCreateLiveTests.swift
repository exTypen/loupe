import Testing
import Foundation
@testable import Loupe

// Creates (then closes) a pull request on a local Forgejo, never a real server. Opt-in:
// LOUPE_FORGEJO_LIVE_TOKEN=<token> [LOUPE_FORGEJO_LIVE_URL=http://localhost:3300] [LOUPE_FORGEJO_LIVE_REPO=alex/dotfiles] swift test --filter PullCreateLive
@Suite(.enabled(if: ProcessInfo.processInfo.environment["LOUPE_FORGEJO_LIVE_TOKEN"] != nil))
struct PullCreateLive {
    let env = ProcessInfo.processInfo.environment
    var client: ForgejoClient { ForgejoClient(baseURL: env["LOUPE_FORGEJO_LIVE_URL"] ?? "http://localhost:3300", token: env["LOUPE_FORGEJO_LIVE_TOKEN"]!) }
    var repo: RemoteRepo { RemoteRepo(remoteURL: "http://localhost/\(env["LOUPE_FORGEJO_LIVE_REPO"] ?? "alex/dotfiles")")! }

    @Test func createsFindsAndRefusesADuplicate() async throws {
        let c = client, r = repo, branch = "loupe-test/pr-\(UUID().uuidString.prefix(6))"
        let base = try await c.defaultBranch(r)
        struct NewBranch: Encodable { var new_branch_name: String; var old_branch_name: String }
        struct NewFile: Encodable { var branch: String; var content: String; var message: String }
        _ = try await c.send(try c.request("POST", "/repos/\(r.owner)/\(r.name)/branches", body: NewBranch(new_branch_name: branch, old_branch_name: base)))
        _ = try await c.send(try c.request("POST", "/repos/\(r.owner)/\(r.name)/contents/\(branch.replacingOccurrences(of: "/", with: "-")).txt",
                                           body: NewFile(branch: branch, content: Data("hi\n".utf8).base64EncodedString(), message: "Add a test file")))
        do { try await check(c, r, branch: branch, base: base) } catch { Issue.record(error) }
        _ = try? await c.send(try c.request("DELETE", "/repos/\(r.owner)/\(r.name)/branches/\(branch.addingPercentEncoding(withAllowedCharacters: .alphanumerics.union(["-", "_", "."]))!)"))
    }

    private func check(_ c: ForgejoClient, _ r: RemoteRepo, branch: String, base: String) async throws {
        #expect(try await c.openPull(r, head: branch) == nil)

        let pr = try await c.createPull(r, ForgejoClient.newPull(head: branch, base: base, title: "Add a test file", body: "From Loupe's tests.", draft: true, github: false))
        #expect(pr.title == "WIP: Add a test file" && pr.head.ref == branch && pr.base.ref == base && pr.state == "open")
        #expect(try await c.openPull(r, head: branch)?.number == pr.number)

        await #expect(throws: ForgejoClient.PullExists.self) {
            try await c.createPull(r, ForgejoClient.newPull(head: branch, base: base, title: "Again", body: "", draft: false, github: false))
        }
        _ = try await c.send(try c.request("PATCH", "/repos/\(r.owner)/\(r.name)/pulls/\(pr.number)", body: ["state": "closed"]))
    }
}

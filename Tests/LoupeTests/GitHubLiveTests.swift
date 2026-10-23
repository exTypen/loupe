import Testing
import Foundation
@testable import Loupe

// Hits api.github.com with the `gh` CLI's token. Opt-in: LOUPE_GH_LIVE=1 swift test --filter GitHubLive
@Suite(.enabled(if: ProcessInfo.processInfo.environment["LOUPE_GH_LIVE"] != nil))
struct GitHubLive {
    let vscode = RemoteRepo(remoteURL: "https://github.com/microsoft/vscode")!
    func client() async throws -> ForgejoClient {
        ForgejoClient(baseURL: "https://github.com", token: try #require(await AddAccountSheet.ghToken(host: "github.com")), github: true)
    }

    @Test func pullsReviewsAndComments() async throws {
        let c = try await client()
        #expect(try await !c.me().login.isEmpty)
        #expect(try await c.pulls(vscode, state: "closed", page: 1).count == ForgejoClient.pullPage)

        let pr = try await c.pull(vscode, 280236)
        #expect(pr.number == 280236)
        let reviews = try await c.reviews(vscode, 280236)
        #expect(reviews.contains { $0.state == "REQUEST_CHANGES" })
        #expect(reviews.contains { $0.dismissed == true })
        #expect(!reviews.contains { ["CHANGES_REQUESTED", "COMMENTED", "PENDING"].contains($0.state) })

        let comments = try await c.reviewComments(vscode, 280236, reviews: reviews)
        #expect(comments.count > 30)   // more than one page
        #expect(comments.contains { $0.position > 0 || $0.original_position > 0 })
        _ = try await c.issueComments(vscode, 280236)
        _ = try await c.combinedStatus(vscode, sha: pr.head.sha)

        let diff = try await c.diff(vscode, 280236)
        #expect(!DiffParser.parse(diff).isEmpty)
    }

    @Test func tooLargeDiffIsReported() async throws {
        let react = RemoteRepo(remoteURL: "https://github.com/facebook/react")!
        await #expect(throws: ForgejoClient.DiffTooLarge.self) { try await client().diff(react, 36701) }
    }
}

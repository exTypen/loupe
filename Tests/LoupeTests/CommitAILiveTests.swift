import Testing
import Foundation
@testable import Loupe

// Runs the local `claude` CLI. Opt-in: LOUPE_AI_LIVE=1 swift test --filter CommitAILive
@Suite(.enabled(if: ProcessInfo.processInfo.environment["LOUPE_AI_LIVE"] != nil))
struct CommitAILive {
    @MainActor @Test func writesAMessageForTheStagedDiff() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-\(UUID().uuidString)").path
        try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(atPath: dir) }
        let id = ["-c", "user.name=t", "-c", "user.email=t@t"]
        try "fn main() {}\n".write(toFile: dir + "/main.rs", atomically: true, encoding: .utf8)
        try await Git.run(["init", "-q"], in: dir)
        try await Git.run(["add", "."], in: dir)
        try await Git.run(id + ["commit", "-qm", "Add an empty main"], in: dir)
        try "fn main() {\n    println!(\"hello, world\");\n}\n".write(toFile: dir + "/main.rs", atomically: true, encoding: .utf8)
        try await Git.run(["add", "."], in: dir)
        // LOUPE_AI_HARNESS=opencode LOUPE_AI_MODEL=opencode/big-pickle picks another tool.
        let env = ProcessInfo.processInfo.environment
        UserDefaults.standard.set(env["LOUPE_AI_HARNESS"] ?? "claude", forKey: "aiHarness")
        if let h = env["LOUPE_AI_HARNESS"], let m = env["LOUPE_AI_MODEL"] { UserDefaults.standard.set(m, forKey: "aiModel.\(h)") }
        await AIHarnesses.shared.scan()
        let msg = try await CommitAI.message(in: dir, amend: false, hasStaged: true)
        print("message:", msg)
        #expect(!msg.isEmpty && !msg.contains("```"))
    }
}

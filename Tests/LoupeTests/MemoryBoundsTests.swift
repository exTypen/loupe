import Testing
import Foundation
@testable import Loupe

/// Caps, eviction and cancellation that keep memory bounded (scripts/memtest found each of these growing without bound).
struct CodeIndexBoundsTests {
    @Test func sizeCaps() {
        #expect(!CodeIndex.isTooLarge("func a() {}\n"))
        #expect(CodeIndex.isTooLarge(String(repeating: "x", count: CodeIndex.maxParseBytes + 1)))
        // Minified: few lines, each very long.
        let minified = (0..<100).map { _ in String(repeating: "var a=1;", count: 200) }.joined(separator: "\n")
        #expect(CodeIndex.isTooLarge(minified))
        // Many short lines over the line cap.
        #expect(CodeIndex.isTooLarge(String(repeating: "x\n", count: CodeIndex.maxParseLines + 10)))
        // Ordinary source of a few thousand lines.
        #expect(!CodeIndex.isTooLarge((0..<3000).map { "func f\($0)() { return \($0) }" }.joined(separator: "\n")))
    }

    @Test func outlineStillWorks() async {
        let text = "struct A {}\nfunc b() {}\n"
        guard case .symbols(let s) = await CodeIndex.outline(of: text, path: "x.swift") else { Issue.record("too large?"); return }
        #expect(s.map(\.name).contains("b"))
        #expect(await CodeIndex.outline(of: String(repeating: "var a=1;", count: 400_000), path: "x.js") == .tooLarge)
    }

    @Test func concurrentCallsShareOneParse() async {
        let cache = OutlineCache()
        let text = (0..<2000).map { "func f\($0)() {}" }.joined(separator: "\n")
        async let a = cache.outline(text, path: "a.swift")
        async let b = cache.outline(text, path: "a.swift")
        let (x, y) = await (a, b)
        #expect(x == y && !x.isEmpty)
        #expect(cache.parses.withLock { $0 } == 1)
        _ = await cache.outline(text, path: "a.swift")   // cached
        #expect(cache.parses.withLock { $0 } == 1)
        #expect(cache.inFlight == 0)
    }

    @Test func cancellingTheCallerStopsTheParse() async {
        let cache = OutlineCache()
        // Under the caps but slow enough to still be parsing when cancelled.
        let text = (0..<45_000).map { "func f\($0)(a: Int, b: String) -> [Int: String] { return [a: b] }" }.joined(separator: "\n")
        let t = Task { await cache.outline(String(text.prefix(CodeIndex.maxParseBytes - 1)), path: "big.swift") }
        try? await Task.sleep(for: .milliseconds(50))
        t.cancel()
        let start = Date()
        let r = await t.value
        #expect(r.isEmpty)
        #expect(Date().timeIntervalSince(start) < 2)
        #expect(cache.inFlight == 0)
    }

    @Test func cancelledSymbolsReturnEmpty() async {
        let t = Task { () -> [CodeSymbol] in
            withUnsafeCurrentTask { $0?.cancel() }
            return CodeIndex.symbols(in: "func a() {}\n", path: "a.swift")
        }
        #expect(await t.value.isEmpty)
    }
}

struct GitCancellationTests {
    @Test func readOnlyDetection() {
        #expect(Git.isReadOnly(["show", "HEAD:a"]))
        #expect(Git.isReadOnly(["-c", "core.quotepath=off", "diff", "--no-color"]))
        #expect(!Git.isReadOnly(["commit", "-m", "x"]))
        #expect(!Git.isReadOnly(["-c", "core.editor=true", "rebase", "--continue"]))
    }

    /// Cancelling stops a running process at once.
    @Test func cancellingStopsTheProcess() throws {
        let running = Git.RunningProcess(killable: true)
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/sleep"); p.arguments = ["30"]
        #expect(try running.start(p))
        let start = Date()
        running.cancel()
        p.waitUntilExit()
        #expect(Date().timeIntervalSince(start) < 3)
        #expect(running.wasCancelled)
    }

    /// A read cancelled before it starts never launches git.
    @Test func cancelledBeforeLaunch() async {
        let t = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            return try await Git.run(["status"], in: FileManager.default.temporaryDirectory.path)
        }
        let r = await t.result
        if case .failure(let e) = r { #expect(e is CancellationError) } else { Issue.record("expected cancellation") }
    }

    @Test func writesAreNotKilled() async throws {
        let running = Git.RunningProcess(killable: false)
        running.cancel()
        #expect(!running.wasCancelled)
    }
}

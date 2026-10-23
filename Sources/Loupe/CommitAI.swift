import Foundation
import Observation

struct ModelOption: Hashable {
    var id: String
    var label: String
    init(_ id: String, _ label: String? = nil) { self.id = id; self.label = label ?? id }
}

/// A coding-agent CLI that can answer one prompt and exit. Loupe runs it in its read-only / ask mode with the diff
/// in the prompt, so it doesn't need tools and can't change the repository.
struct Harness: Identifiable, Hashable {
    let id: String
    let name: String
    let binaries: [String]
    let models: [ModelOption]            // suggestions; the field takes any id, empty = the CLI's own default
    var listModels: [String]? = nil      // a command that prints one model id per line
    /// argv for one prompt, and whether the prompt goes on stdin. `out` is a temp file for CLIs that write their
    /// final answer there (Codex), so progress output can't leak into the message.
    let invocation: (_ prompt: String, _ model: String, _ out: String) -> (args: [String], stdin: Bool)
    var readsOutputFile = false

    static func == (a: Harness, b: Harness) -> Bool { a.id == b.id }
    func hash(into h: inout Hasher) { h.combine(id) }

    static func flag(_ f: String, _ v: String) -> [String] { v.isEmpty ? [] : [f, v] }

    // Flags checked against each CLI's docs (2026-10); claude and opencode also against their --help here.
    static let all: [Harness] = [
        Harness(id: "claude", name: "Claude Code", binaries: ["claude"], models: [
            ModelOption("claude-haiku-4-5-20251001", "Claude Haiku 4.5 · fastest"), ModelOption("claude-sonnet-5", "Claude Sonnet 5"),
            ModelOption("claude-opus-5-5", "Claude Opus 5.5"), ModelOption("claude-fable-5-1", "Claude Fable 5.1 · most capable"),
        ]) { _, m, _ in
            (["-p", "--tools", "", "--setting-sources", "", "--strict-mcp-config", "--no-session-persistence", "--output-format", "text"]
             + flag("--model", m), true)
        },
        Harness(id: "codex", name: "Codex", binaries: ["codex"], models: [ModelOption("gpt-6.1-sol"), ModelOption("gpt-6-astra"), ModelOption("gpt-6-luna")],
                invocation: { _, m, out in
                    (["exec", "--skip-git-repo-check", "--ephemeral", "-s", "read-only", "-o", out] + flag("-m", m) + ["-"], true)
                }, readsOutputFile: true),
        Harness(id: "gemini", name: "Gemini CLI", binaries: ["gemini"], models: [ModelOption("flash"), ModelOption("pro"), ModelOption("flash-lite"), ModelOption("gemini-2.5-flash")]) { p, m, _ in
            (["--output-format", "text"] + flag("-m", m) + ["-p", p], false)
        },
        Harness(id: "copilot", name: "GitHub Copilot CLI", binaries: ["copilot"], models: [ModelOption("claude-sonnet-5"), ModelOption("gpt-6.1-sol"), ModelOption("gpt-5.3-codex")]) { p, m, _ in
            (["-p", p, "-s", "--no-ask-user", "--no-color"] + flag("--model", m), false)
        },
        Harness(id: "cursor", name: "Cursor CLI", binaries: ["cursor-agent"], models: [ModelOption("auto")], listModels: ["models"]) { p, m, _ in
            (["-p", "--output-format", "text", "--mode", "ask", "--trust"] + flag("--model", m) + [p], false)
        },
        Harness(id: "opencode", name: "opencode", binaries: ["opencode"], models: [], listModels: ["models"]) { p, m, _ in
            (["run"] + flag("-m", m) + [p], false)
        },
        Harness(id: "qwen", name: "Qwen Code", binaries: ["qwen"], models: [ModelOption("qwen3-coder-plus")]) { p, m, _ in
            (["-p", p, "--output-format", "text", "--approval-mode", "plan"] + flag("-m", m), false)
        },
        Harness(id: "amp", name: "Amp", binaries: ["amp"], models: []) { _, _, _ in (["-x"], true) },
        Harness(id: "droid", name: "Factory Droid", binaries: ["droid"], models: [ModelOption("claude-opus-5-5"), ModelOption("gpt-6-sol")]) { _, m, _ in
            (["exec", "-o", "text"] + flag("-m", m), true)
        },
    ]
}

/// How the message should read.
enum CommitStyle: String, CaseIterable, Identifiable {
    case repository, concise, standard, detailed, conventional
    var id: String { rawValue }
    var title: String {
        switch self {
        case .repository: "Match the repository"
        case .concise: "Concise"
        case .standard: "Standard"
        case .detailed: "Detailed"
        case .conventional: "Conventional Commits"
        }
    }
    var summary: String {
        switch self {
        case .repository: "Follows the style, length and language of the recent commits."
        case .concise: "A single summary line, no description."
        case .standard: "A summary line, and a short description only when the change needs explaining."
        case .detailed: "A summary line and a description of what changed and why, as bullet points."
        case .conventional: "type(scope): summary, as in feat(api): …, with a short description when needed."
        }
    }
    var instruction: String {
        switch self {
        case .repository:
            "Match the style, language, length and conventions of the recent commit subjects (use prefixes like \"feat:\" only if they do). Add a short description only if the change needs explaining."
        case .concise:
            "Write only a summary line of at most 60 characters, in the imperative mood. No description."
        case .standard:
            "Write a summary line of at most 72 characters in the imperative mood. Add a description of a few short lines only if the change needs explaining."
        case .detailed:
            "Write a summary line of at most 72 characters in the imperative mood, then a blank line, then a description: a few bullet points on what changed and why, mentioning anything a reviewer should know."
        case .conventional:
            "Use the Conventional Commits format: type(optional scope): summary, with type one of feat, fix, refactor, perf, docs, test, build, ci, chore, style. Summary at most 72 characters, imperative, lowercase after the colon. Add a short description only if the change needs explaining."
        }
    }
}

/// Which harnesses are installed; looked up once, again on request from Settings.
@MainActor @Observable
final class AIHarnesses {
    static let shared = AIHarnesses()
    private(set) var paths: [String: String] = [:]   // harness id → executable
    private(set) var scanned = false

    init() { Task { await scan() } }

    func scan() async {
        let names = Harness.all.flatMap(\.binaries)
        let found = await Task.detached(priority: .utility) { Self.locate(names) }.value
        var p: [String: String] = [:]
        for h in Harness.all { if let path = h.binaries.lazy.compactMap({ found[$0] }).first { p[h.id] = path } }
        paths = p
        scanned = true
    }

    /// The usual install locations, then the login shell's PATH (nvm, asdf, custom prefixes).
    nonisolated static func locate(_ names: [String]) -> [String: String] {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let dirs = ["/opt/homebrew/bin", "/usr/local/bin", "\(home)/.local/bin", "\(home)/.claude/local", "\(home)/.bun/bin",
                    "\(home)/.npm-global/bin", "\(home)/.opencode/bin", "\(home)/.cargo/bin", "\(home)/.amp/bin", "\(home)/.factory/bin"]
        var found: [String: String] = [:]
        for n in names { if let d = dirs.first(where: { FileManager.default.isExecutableFile(atPath: "\($0)/\(n)") }) { found[n] = "\(d)/\(n)" } }
        let missing = names.filter { found[$0] == nil }
        guard !missing.isEmpty else { return found }
        let p = Process(), out = Pipe()
        p.executableURL = URL(fileURLWithPath: "/bin/zsh")
        p.arguments = ["-lc", missing.map { "command -v \($0) 2>/dev/null | sed 's|^|\($0)=|'" }.joined(separator: "; ")]
        p.standardOutput = out; p.standardError = FileHandle.nullDevice
        guard (try? p.run()) != nil else { return found }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        for line in String(decoding: data, as: UTF8.self).split(separator: "\n") {
            let kv = line.split(separator: "=", maxSplits: 1)
            if kv.count == 2, kv[1].hasPrefix("/") { found[String(kv[0])] = String(kv[1]) }
        }
        return found
    }
}

/// Commit messages written by a local coding-agent CLI, on the user's own subscription: no API key in Loupe.
enum CommitAI {
    static let defaultHarness = "auto"   // the first installed one
    static var harnessID: String { UserDefaults.standard.string(forKey: "aiHarness") ?? defaultHarness }
    static var style: CommitStyle { CommitStyle(rawValue: UserDefaults.standard.string(forKey: "aiStyle") ?? "") ?? .repository }
    static var extraInstructions: String { UserDefaults.standard.string(forKey: "aiInstructions") ?? "" }
    static func model(for h: Harness) -> String {
        let saved = UserDefaults.standard.string(forKey: "aiModel.\(h.id)")
        guard h.id == "claude" else { return saved ?? "" }
        // Claude Code's aliases, saved by earlier builds, name no version; show the model they stand for.
        let aliases = ["haiku": "claude-haiku-4-5-20251001", "sonnet": "claude-sonnet-5", "opus": "claude-opus-5-5"]
        return saved.map { aliases[$0] ?? $0 } ?? "claude-haiku-4-5-20251001"
    }

    /// The harness in use, if it's installed.
    @MainActor static var selected: Harness? {
        let paths = AIHarnesses.shared.paths
        if harnessID == "off" { return nil }
        if harnessID == defaultHarness { return Harness.all.first { paths[$0.id] != nil } }
        return Harness.all.first { $0.id == harnessID && paths[$0.id] != nil }
    }

    struct Failure: LocalizedError { var errorDescription: String? }

    @MainActor static func message(in dir: String, amend: Bool, hasStaged: Bool) async throws -> String {
        guard let h = selected, let exe = AIHarnesses.shared.paths[h.id] else {
            throw Failure(errorDescription: "No AI tool is set up for commit messages. Pick one in Settings › General.")
        }
        let prompt = try await prompt(in: dir, amend: amend, hasStaged: hasStaged)
        let out = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-msg-\(UUID().uuidString).txt").path
        defer { try? FileManager.default.removeItem(atPath: out) }
        let inv = h.invocation(prompt, model(for: h), out)
        var text = try await run(exe, inv.args, stdin: inv.stdin ? prompt : nil, in: dir)
        if h.readsOutputFile, let f = try? String(contentsOfFile: out, encoding: .utf8), !f.isEmpty { text = f }
        let msg = clean(text)
        guard !msg.isEmpty else { throw Failure(errorDescription: "\(h.name) returned an empty message.") }
        return msg
    }

    /// The change: what's staged, or everything when nothing is (plus untracked file names).
    private static func prompt(in dir: String, amend: Bool, hasStaged: Bool) async throws -> String {
        var diffArgs = ["diff", "--no-color", "--no-ext-diff", "-M", "-U2"]
        if amend, (try? await Git.run(["rev-parse", "--verify", "-q", "HEAD~1"], in: dir)) != nil { diffArgs += ["--cached", "HEAD~1"] }
        else if hasStaged { diffArgs.append("--cached") }
        else if (try? await Git.run(["rev-parse", "--verify", "-q", "HEAD"], in: dir)) != nil { diffArgs.append("HEAD") }
        async let diff = Git.string(diffArgs, in: dir)
        async let stat = Git.string(diffArgs.filter { $0 != "-U2" } + ["--stat"], in: dir)
        async let recent = Git.run(["log", "-n", "15", "--format=%s"], in: dir, allowFailure: true)
        async let untracked = hasStaged || amend ? "" : Git.string(["ls-files", "--others", "--exclude-standard"], in: dir)
        var patch = try await diff
        let newFiles = (try? await untracked) ?? ""
        guard !patch.isEmpty || !newFiles.isEmpty else { throw Failure(errorDescription: "There are no changes to describe.") }
        let limit = 60_000
        if patch.utf8.count > limit { patch = String(patch.prefix(limit)) + "\n[diff truncated]" }
        let extra = extraInstructions.trimmingCharacters(in: .whitespacesAndNewlines)
        return """
        You write git commit messages. Below are this repository's recent commit subjects, then the change.

        \(style.instruction)\(extra.isEmpty ? "" : "\n\(extra)")

        Reply with only the commit message: the summary line, then (if any) a blank line and the description. No quotes, \
        no code fences, no preamble. Don't run tools or look at files; everything you need is here.

        Recent subjects:
        \(String(decoding: try await recent, as: UTF8.self))
        Summary:
        \(try await stat)\(newFiles.isEmpty ? "" : "\nNew files:\n" + newFiles)
        Diff:
        \(patch)
        """
    }

    /// Drops ANSI escapes, code fences and wrapping quotes some CLIs add.
    static func clean(_ s: String) -> String {
        var t = s.replacingOccurrences(of: "\u{1B}\\[[0-9;?]*[A-Za-z]", with: "", options: .regularExpression)
        t = t.trimmingCharacters(in: .whitespacesAndNewlines)
        if t.hasPrefix("```") { t = t.split(separator: "\n", omittingEmptySubsequences: false).dropFirst().filter { !$0.hasPrefix("```") }.joined(separator: "\n") }
        if t.count > 1, t.first == "\"", t.last == "\"" { t = String(t.dropFirst().dropLast()) }
        return t.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Model ids from the CLI itself, for the ones that can list them.
    static func availableModels(_ h: Harness, path: String) async -> [ModelOption] {
        guard let cmd = h.listModels else { return h.models }
        let out = (try? await run(path, cmd, stdin: nil, in: NSTemporaryDirectory(), timeout: 20)) ?? ""
        let ids = clean(out).split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty && !$0.contains(" ") }
        return ids.isEmpty ? h.models : ids.map { ModelOption($0) }
    }

    static func run(_ exe: String, _ args: [String], stdin: String?, in dir: String, timeout: TimeInterval = 120) async throws -> String {
        try await withCheckedThrowingContinuation { cont in
            DispatchQueue.global(qos: .userInitiated).async {
                let p = Process(), out = Pipe(), err = Pipe(), inp = Pipe()
                p.executableURL = URL(fileURLWithPath: exe)
                p.arguments = args
                p.currentDirectoryURL = URL(fileURLWithPath: dir)
                var env = ProcessInfo.processInfo.environment
                let home = FileManager.default.homeDirectoryForCurrentUser.path
                // Node-based CLIs need node on PATH; it usually sits next to them.
                env["PATH"] = "\((exe as NSString).deletingLastPathComponent):/opt/homebrew/bin:/usr/local/bin:\(home)/.local/bin:" + (env["PATH"] ?? "/usr/bin:/bin")
                env["NO_COLOR"] = "1"
                p.environment = env
                p.standardOutput = out; p.standardError = err
                p.standardInput = stdin == nil ? FileHandle.nullDevice : inp
                do { try p.run() } catch { cont.resume(throwing: error); return }
                if let stdin {
                    DispatchQueue.global().async { inp.fileHandleForWriting.write(Data(stdin.utf8)); try? inp.fileHandleForWriting.close() }
                }
                let killer = DispatchWorkItem { if p.isRunning { p.terminate() } }
                DispatchQueue.global().asyncAfter(deadline: .now() + timeout, execute: killer)
                var errData = Data()
                let g = DispatchGroup(); g.enter()
                DispatchQueue.global().async { errData = err.fileHandleForReading.readDataToEndOfFile(); g.leave() }
                let data = out.fileHandleForReading.readDataToEndOfFile()
                p.waitUntilExit(); g.wait(); killer.cancel()
                if p.terminationReason == .uncaughtSignal {
                    cont.resume(throwing: Failure(errorDescription: "\((exe as NSString).lastPathComponent) took longer than \(Int(timeout)) s and was stopped."))
                } else if p.terminationStatus == 0 {
                    cont.resume(returning: String(decoding: data, as: UTF8.self))
                } else {
                    let msg = clean(String(decoding: errData.isEmpty ? data : errData, as: UTF8.self))
                    cont.resume(throwing: Failure(errorDescription: msg.isEmpty ? "\((exe as NSString).lastPathComponent) exited with \(p.terminationStatus)" : String(msg.suffix(600))))
                }
            }
        }
    }
}

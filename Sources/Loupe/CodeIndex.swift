import Foundation
import Synchronization
import SwiftTreeSitter
import TreeSitterRust
import TreeSitterSwift
import TreeSitterGo
import TreeSitterTypeScript
import TreeSitterTSX
import TreeSitterJavaScript
import TreeSitterPython
import TreeSitterC
import TreeSitterCPP
import TreeSitterJava

/// A named definition or reference, as the grammars' tags queries report them.
struct CodeSymbol: Hashable, Sendable {
    enum Kind: String, Sendable { case function, method, `class`, interface, module, type, constant, macro, field, other }
    var name: String
    var kind: Kind
    var isDefinition: Bool
    var path: String
    var line: Int          // 1-based
}

/// Tree-sitter parsing for the languages Loupe has grammars for. Parsers aren't thread-safe, so each call makes
/// its own; compiled queries are immutable and shared.
enum CodeIndex {
    struct Grammar: @unchecked Sendable {   // Language and Query are immutable once built
        let language: SwiftTreeSitter.Language
        let tags: Query?
    }

    /// The grammar for a path's extension, if Loupe has one.
    static func grammar(for path: String) -> Grammar? {
        let ext = (path as NSString).pathExtension.lowercased()
        let id: String
        switch ext {
        case "rs": id = "rust"
        case "swift": id = "swift"
        case "go": id = "go"
        case "ts", "mts", "cts": id = "typescript"
        case "tsx": id = "tsx"
        case "js", "jsx", "mjs", "cjs": id = "javascript"
        case "py", "pyi": id = "python"
        case "c", "h": id = "c"
        case "cc", "cpp", "cxx", "hpp", "hh", "hxx", "mm": id = "cpp"
        case "java": id = "java"
        default: return nil
        }
        return cache.withLock { c in
            if let g = c[id] { return g }
            let ptr: OpaquePointer
            switch id {
            case "rust": ptr = tree_sitter_rust()
            case "swift": ptr = tree_sitter_swift()
            case "go": ptr = tree_sitter_go()
            case "typescript": ptr = tree_sitter_typescript()
            case "tsx": ptr = tree_sitter_tsx()
            case "javascript": ptr = tree_sitter_javascript()
            case "python": ptr = tree_sitter_python()
            case "c": ptr = tree_sitter_c()
            case "cpp": ptr = tree_sitter_cpp()
            default: ptr = tree_sitter_java()
            }
            let lang = SwiftTreeSitter.Language(ptr)
            let source = TagQueries.source[id == "tsx" ? "typescript" : id]
            let g = Grammar(language: lang, tags: source.flatMap { try? Query(language: lang, data: Data($0.utf8)) })
            c[id] = g
            return g
        }
    }
    private static let cache = Mutex<[String: Grammar]>([:])

    static func canNavigate(_ path: String) -> Bool { grammar(for: path)?.tags != nil }

    /// Every definition and reference the language's tags query finds in `text`.
    static func symbols(in text: String, path: String) -> [CodeSymbol] {
        guard let g = grammar(for: path), let query = g.tags else { return [] }
        let parser = Parser()
        guard (try? parser.setLanguage(g.language)) != nil, let tree = parser.parse(text) else { return [] }
        let ns = text as NSString
        var out: [CodeSymbol] = []
        for match in query.execute(in: tree) {
            guard let nameCapture = match.captures.first(where: { $0.name == "name" }) else { continue }
            let tag = match.captures.compactMap(\.name).first { $0.hasPrefix("definition.") || $0.hasPrefix("reference.") } ?? ""
            let parts = tag.split(separator: ".", maxSplits: 1)
            guard parts.count == 2 else { continue }
            let name = ns.substring(with: nameCapture.range)
            guard !name.isEmpty, name.count < 200 else { continue }
            out.append(CodeSymbol(name: name, kind: kind(String(parts[1])), isDefinition: parts[0] == "definition",
                                  path: path, line: Int(nameCapture.node.pointRange.lowerBound.row) + 1))
        }
        return out
    }

    /// The definitions in a file, in order, one per line: the file's outline.
    static func outline(_ text: String, path: String) -> [CodeSymbol] {
        var seen = Set<Int>()
        return symbols(in: text, path: path).filter { $0.isDefinition && seen.insert($0.line).inserted }.sorted { $0.line < $1.line }
    }

    private static func kind(_ s: String) -> CodeSymbol.Kind {
        switch s {
        case "function": .function
        case "method": .method
        case "class", "struct", "enum", "union": .class
        case "interface", "trait", "protocol": .interface
        case "module", "namespace": .module
        case "type": .type
        case "constant": .constant
        case "macro": .macro
        case "field", "property": .field
        default: .other
        }
    }
}

// MARK: - Repository-wide lookups (no checkout: everything is read at a commit)

struct CodeMatch: Hashable, Sendable {
    var path: String
    var line: Int      // 1-based
    var text: String
}

enum CodeNavigator {
    /// Definitions of `name` at `rev` (nil: the worktree as it is): `git grep -w` narrows the files, tree-sitter decides
    /// which hits define it. Ranked: the file you're in, then its directory, then the same language, then the rest.
    static func definitions(of name: String, at rev: String?, in dir: String, from path: String) async -> [CodeSymbol] {
        let files = await grepFiles(name, at: rev, in: dir)
        let candidates = files.filter(CodeIndex.canNavigate).sorted { rank($0, from: path) < rank($1, from: path) }.prefix(80)
        var found: [CodeSymbol] = []
        await withTaskGroup(of: [CodeSymbol].self) { g in
            for f in candidates {
                g.addTask {
                    let text = if let rev { try? await Git.string(["show", "\(rev):\(f)"], in: dir) }
                               else { try? String(contentsOfFile: (dir as NSString).appendingPathComponent(f), encoding: .utf8) }
                    guard let text else { return [] }
                    return CodeIndex.symbols(in: text, path: f).filter { $0.isDefinition && $0.name == name }
                }
            }
            for await s in g { found += s }
        }
        return found.sorted { (rank($0.path, from: path), $0.path, $0.line) < (rank($1.path, from: path), $1.path, $1.line) }
    }

    /// Every whole-word occurrence of `name` at `rev` (nil: the worktree, untracked files included).
    static func references(of name: String, at rev: String?, in dir: String, limit: Int = 2000) async -> [CodeMatch] {
        guard let out = try? await Git.string(["grep", "-n", "-I", "-w", "--fixed-strings", "-e", name] + scope(rev), in: dir) else { return [] }
        let prefix = (rev ?? "") + ":"
        var matches: [CodeMatch] = []
        for line in out.split(separator: "\n") {
            // "<rev>:<path>:<line>:<text>"
            var rest = Substring(line)
            if rev != nil, rest.hasPrefix(prefix) { rest = rest.dropFirst(prefix.count) }
            let f = rest.split(separator: ":", maxSplits: 2, omittingEmptySubsequences: false)
            guard f.count == 3, let n = Int(f[1]) else { continue }
            matches.append(CodeMatch(path: String(f[0]), line: n, text: f[2].trimmingCharacters(in: .whitespaces)))
            if matches.count >= limit { break }
        }
        return matches
    }

    private static func grepFiles(_ name: String, at rev: String?, in dir: String) async -> [String] {
        guard let out = try? await Git.string(["grep", "-l", "-I", "-w", "--fixed-strings", "-e", name] + scope(rev), in: dir) else { return [] }
        guard let rev else { return out.split(separator: "\n").map(String.init) }
        let prefix = rev + ":"
        return out.split(separator: "\n").map { $0.hasPrefix(prefix) ? String($0.dropFirst(prefix.count)) : String($0) }
    }

    /// What git grep searches: a revision, or the worktree with its untracked (not ignored) files.
    private static func scope(_ rev: String?) -> [String] { rev.map { [$0, "--"] } ?? ["--untracked", "--"] }

    private static func rank(_ candidate: String, from path: String) -> Int {
        if candidate == path { return 0 }
        let dir = (path as NSString).deletingLastPathComponent
        if (candidate as NSString).deletingLastPathComponent == dir { return 1 }
        if (candidate as NSString).pathExtension == (path as NSString).pathExtension { return 2 }
        return 3
    }
}

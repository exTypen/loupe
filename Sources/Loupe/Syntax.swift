import AppKit

/// Lightweight, dependency-free syntax highlighting for diff lines. It tokenizes line by line (diffs are
/// fragments, so a full parser would lack context anyway) and carries block-comment / multi-line-string
/// state across lines. ponytail: regex-free hand tokenizer, not a grammar — upgrade to tree-sitter if accuracy matters.
enum TokenKind: UInt8 { case keyword, string, comment, number, type, function, macro, attribute, property }

struct SyntaxSpan: Sendable { var start: Int32; var length: Int32; var kind: TokenKind }

struct Language: Sendable {
    var keywords: Set<String>
    var lineComments: [String] = ["//"]
    var block: (String, String)? = ("/*", "*/")
    var strings: [Character] = ["\"", "'"]
    var multilineStrings: [String] = []      // delimiters that may span lines: "\"\"\"", "`", "''"
    var macroBang = false                    // rust: name!
    var attributePrefix: [String] = []       // "#[", "@"
    var caseInsensitive = false              // SQL
    var keyValue = false                     // YAML/TOML/JSON-ish: highlight keys
    var dollarVars = false                   // shell, PHP

    static func forPath(_ path: String) -> Language? {
        let name = (path as NSString).lastPathComponent.lowercased()
        let ext = (name as NSString).pathExtension
        if name == "dockerfile" || name.hasPrefix("dockerfile.") { return .shell }
        if name == "makefile" || name == "justfile" { return .shell }
        switch ext {
        case "rs": return .rust
        case "swift": return .swift
        case "go": return .go
        case "ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts": return .typescript
        case "py", "pyi": return .python
        case "c", "h", "cc", "cpp", "cxx", "hpp", "m", "mm": return .c
        case "java", "kt", "kts", "scala", "cs", "dart": return .jvm
        case "nix": return .nix
        case "sql": return .sql
        case "sh", "bash", "zsh", "fish", "envrc": return .shell
        case "yml", "yaml": return .yaml
        case "toml", "ini", "cfg", "conf": return .toml
        case "json", "jsonc", "json5": return .json
        case "css", "scss", "sass", "less": return .css
        case "rb": return .ruby
        case "lua": return .lua
        case "sol": return .solidity
        case "proto", "graphql", "gql": return .proto
        case "php": return .php
        case "tf", "tfvars", "hcl": return .hcl
        default: return nil
        }
    }

    private static func k(_ s: String) -> Set<String> { Set(s.split(separator: " ").map(String.init)) }

    static let rust = Language(keywords: k("as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while yield Some None Ok Err"),
                               macroBang: true, attributePrefix: ["#[", "#!["])
    static let swift = Language(keywords: k("actor any as associatedtype async await break case catch class continue default defer deinit do else enum extension fallthrough false fileprivate final for func guard if import in init inout internal is lazy let mutating nil nonisolated open operator override private protocol public repeat rethrows return self Self some static struct subscript super switch throw throws true try typealias unowned var weak where while"),
                                multilineStrings: ["\"\"\""], attributePrefix: ["@"])
    static let go = Language(keywords: k("break case chan const continue default defer else fallthrough false for func go goto if import interface iota map nil package range return select struct switch true type var"),
                             strings: ["\"", "'"], multilineStrings: ["`"])
    static let typescript = Language(keywords: k("abstract as async await break case catch class const continue debugger declare default delete do else enum export extends false finally for from function get if implements import in instanceof interface keyof let new null of private protected public readonly return satisfies set static super switch this throw true try type typeof undefined var void while yield"),
                                     multilineStrings: ["`"], attributePrefix: ["@"])
    static let python = Language(keywords: k("and as assert async await break class continue def del elif else except False finally for from global if import in is lambda None nonlocal not or pass raise return self True try while with yield match case"),
                                 lineComments: ["#"], block: nil, multilineStrings: ["\"\"\"", "'''"], attributePrefix: ["@"])
    static let c = Language(keywords: k("auto break case char const continue default do double else enum extern float for goto if inline int long register return short signed sizeof static struct switch typedef union unsigned void volatile while bool true false NULL nullptr class namespace public private protected template typename virtual override new delete this using constexpr auto"),
                            attributePrefix: ["#"])
    static let jvm = Language(keywords: k("abstract and as break case catch class const continue data default do else enum extends false final finally for fun if implements import in interface is internal lateinit new null object open override package private protected public return sealed static super suspend switch this throw throws true try val var void when while"),
                              multilineStrings: ["\"\"\""], attributePrefix: ["@"])
    static let nix = Language(keywords: k("let in with rec inherit if then else assert import true false null or builtins"),
                              lineComments: ["#"], strings: ["\""], multilineStrings: ["''"])
    static let sql = Language(keywords: k("select from where and or not insert into values update set delete create table index unique primary key foreign references alter add drop column constraint default null is in as on join left right inner outer group by order having limit offset returning begin commit rollback if exists cascade check case when then else end distinct union all text integer bigint boolean timestamptz uuid jsonb varchar serial timestamp bytea date time interval numeric decimal real float double smallint char json array with view trigger function returns language sequence grant revoke type enum materialized concurrently using"),
                              lineComments: ["--"], strings: ["'"], caseInsensitive: true)
    static let shell = Language(keywords: k("if then else elif fi for while until do done case esac function in return export local readonly set unset source echo exit RUN FROM COPY ADD ENV ARG WORKDIR CMD ENTRYPOINT EXPOSE USER"),
                                lineComments: ["#"], block: nil, dollarVars: true)
    static let yaml = Language(keywords: k("true false null yes no on off"), lineComments: ["#"], block: nil, keyValue: true)
    static let toml = Language(keywords: k("true false"), lineComments: ["#", ";"], block: nil, multilineStrings: ["\"\"\""], keyValue: true)
    static let json = Language(keywords: k("true false null"), lineComments: ["//"], strings: ["\""], keyValue: true)
    static let css = Language(keywords: k("important inherit initial unset none auto"), lineComments: ["//"], attributePrefix: ["@"], keyValue: true)
    static let ruby = Language(keywords: k("alias and begin break case class def defined do else elsif end ensure false for if in module next nil not or redo rescue retry return self super then true undef unless until when while yield require attr_accessor"),
                               lineComments: ["#"], block: nil, attributePrefix: ["@"])
    static let lua = Language(keywords: k("and break do else elseif end false for function goto if in local nil not or repeat return then true until while"), lineComments: ["--"], block: ("--[[", "]]"))
    static let solidity = Language(keywords: k("contract interface library function modifier event emit struct enum mapping public private internal external view pure payable returns return if else for while memory storage calldata require revert assert uint256 uint address bool bytes32 string true false constructor override virtual immutable constant"))
    static let proto = Language(keywords: k("syntax package import option message enum service rpc returns repeated optional map oneof reserved true false type query mutation subscription fragment on input scalar interface union extend schema"),
                                lineComments: ["//", "#"])
    static let php = Language(keywords: k("abstract and array as break case catch class const continue default do echo else elseif extends false final for foreach function if implements interface namespace new null private protected public return static switch this throw true try use while"),
                              lineComments: ["//", "#"], dollarVars: true)
    static let hcl = Language(keywords: k("resource data variable output module provider locals terraform true false null for in if"), lineComments: ["#", "//"], keyValue: true)
}

enum Syntax {
    /// Text exactly as the row draws it (truncated, tabs expanded), so span offsets line up.
    static func displayText(_ text: Substring) -> String {
        var t = String(text.prefix(Theme.maxDrawnChars))
        if t.contains("\t") { t = t.replacingOccurrences(of: "\t", with: "    ") }
        return t
    }

    struct State { var block = false; var string: String? = nil }

    /// Spans for every line of the diff. Old and new sides keep separate state (a removed line opening
    /// a block comment must not swallow added lines).
    static func highlight(_ file: FileDiff, language: Language) -> [[SyntaxSpan]] {
        var old = State(), new = State()
        return file.lines.map { line in
            switch line.kind {
            case .hunk: old = State(); new = State(); return []
            case .meta: return []
            case .removed: return tokenize(displayText(line.text), language, &old)
            case .added: return tokenize(displayText(line.text), language, &new)
            case .context:
                var probe = old
                let spans = tokenize(displayText(line.text), language, &probe)
                old = probe; new = probe
                return spans
            }
        }
    }

    static func tokenize(_ text: String, _ lang: Language, _ state: inout State) -> [SyntaxSpan] {
        let u = Array(text.utf16)
        var spans: [SyntaxSpan] = []
        var i = 0
        func add(_ s: Int, _ e: Int, _ k: TokenKind) { if e > s { spans.append(SyntaxSpan(start: Int32(s), length: Int32(e - s), kind: k)) } }
        func starts(_ s: String, at p: Int) -> Bool {
            let x = Array(s.utf16)
            guard p + x.count <= u.count else { return false }
            for j in 0..<x.count where u[p + j] != x[j] { return false }
            return true
        }
        func find(_ s: String, from p: Int) -> Int? {
            var q = p
            while q < u.count { if starts(s, at: q) { return q }; q += 1 }
            return nil
        }
        func isIdentStart(_ c: UInt16) -> Bool { (c >= 65 && c <= 90) || (c >= 97 && c <= 122) || c == 95 || (lang.dollarVars ? false : c == 36) || c > 127 }
        func isIdent(_ c: UInt16) -> Bool { isIdentStart(c) || (c >= 48 && c <= 57) }

        // Continue constructs opened on a previous line.
        if state.block, let (_, close) = lang.block {
            if let e = find(close, from: 0) { add(0, e + close.utf16.count, .comment); i = e + close.utf16.count; state.block = false }
            else { add(0, u.count, .comment); return spans }
        }
        if let delim = state.string {
            if let e = find(delim, from: 0) { add(0, e + delim.utf16.count, .string); i = e + delim.utf16.count; state.string = nil }
            else { add(0, u.count, .string); return spans }
        }
        // YAML/TOML/JSON keys: `key:` / `key =` / `"key":` at the start of the line.
        if lang.keyValue {
            var p = i
            while p < u.count, u[p] == 32 || u[p] == 45 { p += 1 }   // indent and "- "
            if p < u.count, u[p] == 91 {                             // [section]
                if let e = find("]", from: p) { add(p, e + 1, .type); i = e + 1 }
            } else {
                let quoted = p < u.count && u[p] == 34
                var q = quoted ? p + 1 : p
                while q < u.count, quoted ? u[q] != 34 : (isIdent(u[q]) || u[q] == 45 || u[q] == 46) { q += 1 }
                let end = quoted ? q + 1 : q
                var r = end
                while r < u.count, u[r] == 32 { r += 1 }
                if q > p, r < u.count, u[r] == 58 || u[r] == 61 { add(p, end, .property); i = end }
            }
        }

        while i < u.count {
            let c = u[i]
            if c == 32 || c == 9 { i += 1; continue }
            if lang.lineComments.contains(where: { starts($0, at: i) }) { add(i, u.count, .comment); break }
            if let (open, close) = lang.block, starts(open, at: i) {
                if let e = find(close, from: i + open.utf16.count) { add(i, e + close.utf16.count, .comment); i = e + close.utf16.count; continue }
                add(i, u.count, .comment); state.block = true; break
            }
            if let m = lang.multilineStrings.first(where: { starts($0, at: i) }) {
                let n = m.utf16.count
                if let e = find(m, from: i + n) { add(i, e + n, .string); i = e + n; continue }
                add(i, u.count, .string); state.string = m; break
            }
            if let a = lang.attributePrefix.first(where: { starts($0, at: i) }) {
                var e = i + a.utf16.count
                if a.hasSuffix("[") { e = (find("]", from: e) ?? u.count - 1) + 1 } else { while e < u.count, isIdent(u[e]) || u[e] == 46 { e += 1 } }
                if e > i + a.utf16.count || a.hasSuffix("[") { add(i, e, .attribute); i = e; continue }
            }
            if lang.dollarVars, c == 36 {
                var e = i + 1
                if e < u.count, u[e] == 123 { e = (find("}", from: e) ?? u.count - 1) + 1 } else { while e < u.count, isIdent(u[e]) { e += 1 } }
                add(i, e, .type); i = max(e, i + 1); continue
            }
            if let ch = Character(UnicodeScalar(c) ?? " ") as Character?, lang.strings.contains(ch) {
                // Rust 'a lifetimes look like an unterminated char literal.
                if c == 39, lang.macroBang, !(i + 2 < u.count && (u[i + 2] == 39 || u[i + 1] == 92)) {
                    var e = i + 1
                    while e < u.count, isIdent(u[e]) { e += 1 }
                    add(i, e, .attribute); i = max(e, i + 1); continue
                }
                var e = i + 1
                while e < u.count, u[e] != c { e += u[e] == 92 ? 2 : 1 }
                e = min(e + 1, u.count)
                add(i, e, .string); i = e; continue
            }
            if c >= 48 && c <= 57 {
                var e = i + 1
                while e < u.count, isIdent(u[e]) || u[e] == 46 { e += 1 }
                add(i, e, .number); i = e; continue
            }
            if isIdentStart(c) {
                var e = i + 1
                while e < u.count, isIdent(u[e]) { e += 1 }
                let word = String(utf16CodeUnits: Array(u[i..<e]), count: e - i)
                let isKeyword = lang.caseInsensitive ? lang.keywords.contains(word.lowercased()) : lang.keywords.contains(word)
                var n = e
                while n < u.count, u[n] == 32 { n += 1 }
                if isKeyword { add(i, e, .keyword) }
                else if lang.macroBang, e < u.count, u[e] == 33 { add(i, e + 1, .macro); e += 1 }
                else if n < u.count, u[n] == 40 { add(i, e, .function) }
                else if c >= 65 && c <= 90, !lang.caseInsensitive { add(i, e, .type) }
                i = e; continue
            }
            i += 1
        }
        return spans
    }

    static func color(_ k: TokenKind) -> NSColor { palette[Int(k.rawValue)] }

    // Xcode-like defaults, separate light / dark values.
    private static func dyn(_ light: UInt32, _ dark: UInt32) -> NSColor {
        func c(_ v: UInt32) -> NSColor { NSColor(srgbRed: CGFloat((v >> 16) & 0xff) / 255, green: CGFloat((v >> 8) & 0xff) / 255, blue: CGFloat(v & 0xff) / 255, alpha: 1) }
        return NSColor(name: nil) { $0.isDark ? c(dark) : c(light) }
    }
    private static let palette: [NSColor] = [
        dyn(0x9B2393, 0xFF7AB2),   // keyword
        dyn(0xC41A16, 0xFF8170),   // string
        dyn(0x5D6C79, 0x7F8C98),   // comment
        dyn(0x1C00CF, 0xD9C97C),   // number
        dyn(0x0B4F79, 0x6BDFFF),   // type
        dyn(0x326D74, 0x67B7A4),   // function
        dyn(0x643820, 0xFD8F3F),   // macro
        dyn(0x815F03, 0xBF8555),   // attribute
        dyn(0x0F68A0, 0x4EB0CC),   // property / key
    ]
}

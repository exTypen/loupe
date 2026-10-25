import Foundation

/// A Postgres-flavoured DDL reader: enough of CREATE / ALTER / DROP to replay a migrations directory into a schema,
/// so two states of a repository can be compared as schemas rather than as text. Anything it doesn't understand is
/// reported, never guessed. ponytail: hand-written subset parser, not a full SQL grammar; add statements as
/// real migrations need them.
enum SQL {
    // MARK: Tokens

    struct Token: Equatable {
        enum Kind: Equatable { case word, quoted, string, number, symbol }
        var kind: Kind
        var text: String      // words lowercased; quoted identifiers as written
        var line: Int         // 1-based

        var isWord: Bool { kind == .word }
        func `is`(_ w: String) -> Bool { kind == .word && text == w }
        /// An identifier: a bare or quoted word.
        var ident: String? { kind == .word || kind == .quoted ? text : nil }
    }

    struct Statement {
        var tokens: [Token]
        var line: Int
        var endLine: Int
        var text: String      // whitespace-normalised source, for data statement comparison

        /// The statement as written, comments and layout kept, from the file's lines.
        func source(_ lines: [Substring]) -> String {
            guard line >= 1, line <= lines.count else { return text }
            return lines[(line - 1)...min(endLine, lines.count) - 1].joined(separator: "\n")
        }
    }

    /// Splits SQL into statements on top-level semicolons. Comments are dropped; strings, quoted identifiers and
    /// dollar-quoted bodies stay whole.
    static func statements(_ src: String) -> [Statement] {
        let u = Array(src.unicodeScalars)
        var i = 0, line = 1
        var toks: [Token] = [], out: [Statement] = []
        var raw = ""   // the current statement's text
        func flush() {
            if !toks.isEmpty {
                let last = toks[toks.count - 1]
                out.append(Statement(tokens: toks, line: toks[0].line, endLine: last.line + last.text.filter { $0 == "\n" }.count,
                                     text: raw.split(whereSeparator: \.isWhitespace).joined(separator: " ")))
            }
            toks = []; raw = ""
        }
        func isIdentStart(_ c: Unicode.Scalar) -> Bool { CharacterSet.letters.contains(c) || c == "_" }
        func isIdent(_ c: Unicode.Scalar) -> Bool { isIdentStart(c) || CharacterSet.decimalDigits.contains(c) || c == "$" }
        while i < u.count {
            let c = u[i]
            if c == "\n" { line += 1; raw.append(" "); i += 1; continue }
            if CharacterSet.whitespaces.contains(c) { raw.append(" "); i += 1; continue }
            if c == "-", i + 1 < u.count, u[i + 1] == "-" {   // -- comment
                while i < u.count, u[i] != "\n" { i += 1 }
                continue
            }
            if c == "/", i + 1 < u.count, u[i + 1] == "*" {   // /* comment */, nesting as Postgres does
                var depth = 0
                repeat {
                    if u[i] == "/", i + 1 < u.count, u[i + 1] == "*" { depth += 1; i += 2; continue }
                    if u[i] == "*", i + 1 < u.count, u[i + 1] == "/" { depth -= 1; i += 2; continue }
                    if u[i] == "\n" { line += 1 }
                    i += 1
                } while i < u.count && depth > 0
                continue
            }
            let start = i, startLine = line
            func text(_ from: Int, _ to: Int) -> String { String(String.UnicodeScalarView(u[from..<to])) }
            if c == ";" { i += 1; flush(); continue }
            if c == "'" || ((c == "e" || c == "E") && i + 1 < u.count && u[i + 1] == "'") {   // 'string', E'string'
                i += c == "'" ? 1 : 2
                while i < u.count {
                    if u[i] == "\\" && c != "'" { i += 2; continue }
                    if u[i] == "'" { if i + 1 < u.count, u[i + 1] == "'" { i += 2; continue }; i += 1; break }
                    if u[i] == "\n" { line += 1 }
                    i += 1
                }
                let t = text(start, min(i, u.count)); raw += t
                toks.append(Token(kind: .string, text: t, line: startLine)); continue
            }
            if c == "\"" {
                i += 1
                var name = ""
                while i < u.count {
                    if u[i] == "\"" { if i + 1 < u.count, u[i + 1] == "\"" { name.append("\""); i += 2; continue }; i += 1; break }
                    name.unicodeScalars.append(u[i]); i += 1
                }
                raw += text(start, min(i, u.count))
                toks.append(Token(kind: .quoted, text: name, line: startLine)); continue
            }
            if c == "$" {   // $tag$ … $tag$ (or $1 parameters)
                var j = i + 1
                while j < u.count, isIdent(u[j]), u[j] != "$" { j += 1 }
                if j < u.count, u[j] == "$" {
                    let tag = text(i, j + 1)
                    let tagScalars = Array(tag.unicodeScalars)
                    var k = j + 1
                    while k < u.count {
                        if u[k] == "$", k + tagScalars.count <= u.count, Array(u[k..<(k + tagScalars.count)]) == tagScalars { k += tagScalars.count; break }
                        if u[k] == "\n" { line += 1 }
                        k += 1
                    }
                    i = min(k, u.count)
                    let t = text(start, i); raw += t
                    toks.append(Token(kind: .string, text: t, line: startLine)); continue
                }
            }
            if isIdentStart(c) {
                while i < u.count, isIdent(u[i]) { i += 1 }
                let t = text(start, i); raw += t
                toks.append(Token(kind: .word, text: t.lowercased(), line: startLine)); continue
            }
            if CharacterSet.decimalDigits.contains(c) {
                while i < u.count, CharacterSet.decimalDigits.contains(u[i]) || u[i] == "." { i += 1 }
                let t = text(start, i); raw += t
                toks.append(Token(kind: .number, text: t, line: startLine)); continue
            }
            if c == ":", i + 1 < u.count, u[i + 1] == ":" { i += 2; raw += "::"; toks.append(Token(kind: .symbol, text: "::", line: startLine)); continue }
            i += 1
            raw.unicodeScalars.append(c)
            toks.append(Token(kind: .symbol, text: String(c), line: startLine))
        }
        flush()
        return out
    }

    // MARK: Schema

    struct Location: Hashable { var file: String; var line: Int }

    struct Column: Equatable {
        var name: String
        var type: String
        var notNull = false
        var defaultValue: String?
        var references: String?
        var at: Location?
        static func == (a: Column, b: Column) -> Bool {
            a.name == b.name && a.type == b.type && a.notNull == b.notNull && a.defaultValue == b.defaultValue && a.references == b.references
        }
    }

    struct Table {
        var name: String
        var columns: [Column] = []
        var constraints: [String: Location?] = [:]
        var primaryKey: [String] = []
        var at: Location?
        func column(_ n: String) -> Int? { columns.firstIndex { $0.name == n } }
    }

    struct Index: Equatable {
        var name: String
        var table: String
        var definition: String
        var unique: Bool
        var at: Location?
        static func == (a: Index, b: Index) -> Bool { a.table == b.table && a.definition == b.definition && a.unique == b.unique }
    }

    struct Schema {
        var tables: [String: Table] = [:]
        var indexes: [String: Index] = [:]
        var enums: [String: [String]] = [:]
        var enumAt: [String: Location] = [:]
        var views: [String: (body: String, at: Location)] = [:]
        var functions: [String: (body: String, at: Location)] = [:]
        var triggers: [String: (table: String, at: Location)] = [:]
        /// Statements the reader skipped, by kind ("DO block", "CREATE POLICY"…), with where they are.
        var skipped: [(kind: String, at: Location)] = []
    }

    // MARK: Replay

    /// Applies `files` (path, contents) in order.
    static func replay(_ files: [(path: String, text: String)]) -> Schema {
        var s = Schema()
        for f in files {
            for st in statements(f.text) { apply(st, file: f.path, to: &s) }
        }
        return s
    }

    /// Strips the default `public.` schema; other schemas stay part of the name.
    private static func name(_ t: [Token], _ i: inout Int) -> String? {
        guard i < t.count, var n = t[i].ident else { return nil }
        i += 1
        while i + 1 < t.count, t[i].text == ".", let part = t[i + 1].ident { n += "." + part; i += 2 }
        if n.hasPrefix("public.") { n.removeFirst(7) }
        return n
    }

    private static func skip(_ t: [Token], _ i: inout Int, _ words: String...) {
        while i < t.count, words.contains(where: { t[i].is($0) }) { i += 1 }
    }

    /// `IF EXISTS` / `IF NOT EXISTS`, if present.
    private static func ifClause(_ t: [Token], _ i: inout Int) -> Bool {
        guard i < t.count, t[i].is("if") else { return false }
        i += 1
        if i < t.count, t[i].is("not") { i += 1 }
        if i < t.count, t[i].is("exists") { i += 1 }
        return true
    }

    /// Splits tokens between parentheses (from an opening "(") at top-level commas; returns the parts and moves past ")".
    private static func parenList(_ t: [Token], _ i: inout Int) -> [[Token]] {
        guard i < t.count, t[i].text == "(" else { return [] }
        i += 1
        var parts: [[Token]] = [[]], depth = 0
        while i < t.count {
            let x = t[i]
            if x.kind == .symbol && x.text == "(" { depth += 1 }
            if x.kind == .symbol && x.text == ")" { if depth == 0 { i += 1; break }; depth -= 1 }
            if x.kind == .symbol && x.text == "," && depth == 0 { parts.append([]); i += 1; continue }
            parts[parts.count - 1].append(x)
            i += 1
        }
        return parts.filter { !$0.isEmpty }
    }

    /// Splits a token run at top-level commas.
    private static func commaSplit(_ t: ArraySlice<Token>) -> [[Token]] {
        var parts: [[Token]] = [[]], depth = 0
        for x in t {
            if x.kind == .symbol && x.text == "(" { depth += 1 }
            if x.kind == .symbol && x.text == ")" { depth -= 1 }
            if x.kind == .symbol && x.text == "," && depth == 0 { parts.append([]); continue }
            parts[parts.count - 1].append(x)
        }
        return parts.filter { !$0.isEmpty }
    }

    static func render(_ t: some Collection<Token>) -> String {
        var s = ""
        var prev: Token?
        for x in t {
            let glue = prev.map { p in
                (p.kind == .symbol && ["(", ".", "::"].contains(p.text)) || (x.kind == .symbol && [")", ",", ".", "(", "::", "[", "]"].contains(x.text))
                    || (p.kind == .symbol && x.kind == .symbol && "<>!=".contains(p.text) && "<>=".contains(x.text))   // <>, <=, !=
            } ?? true
            if !glue { s += " " }
            s += x.kind == .quoted ? "\"\(x.text)\"" : x.text
            prev = x
        }
        return s
    }

    private static let columnConstraintWords: Set<String> = ["not", "null", "default", "primary", "unique", "references", "check",
                                                              "constraint", "generated", "collate", "identity"]

    /// "name type [constraints]" → Column.
    private static func column(_ t: [Token], at: Location) -> Column? {
        var i = 0
        guard let n = t.first?.ident else { return nil }
        i = 1
        var typeEnd = i
        var depth = 0
        while typeEnd < t.count {
            let x = t[typeEnd]
            if x.kind == .symbol && x.text == "(" { depth += 1 } else if x.kind == .symbol && x.text == ")" { depth -= 1 }
            if depth == 0, x.isWord, columnConstraintWords.contains(x.text) { break }
            typeEnd += 1
        }
        var c = Column(name: n, type: render(t[i..<typeEnd]), at: at)
        i = typeEnd
        while i < t.count {
            let x = t[i]
            if x.is("not"), i + 1 < t.count, t[i + 1].is("null") { c.notNull = true; i += 2; continue }
            if x.is("primary") { c.notNull = true; i += 2; continue }
            if x.is("default") {
                var j = i + 1, d = 0
                while j < t.count {
                    let y = t[j]
                    if y.kind == .symbol && y.text == "(" { d += 1 } else if y.kind == .symbol && y.text == ")" { d -= 1 }
                    if d == 0, y.isWord, columnConstraintWords.contains(y.text), !(y.is("null") && j == i + 1) { break }
                    j += 1
                }
                c.defaultValue = render(t[(i + 1)..<j]); i = j; continue
            }
            if x.is("references") {
                var j = i + 1
                if let target = name(t, &j) {
                    var cols = ""
                    if j < t.count, t[j].text == "(" { let p = parenList(t, &j); cols = "(" + p.map { render($0) }.joined(separator: ", ") + ")" }
                    c.references = target + cols
                }
                i = j; continue
            }
            i += 1
        }
        return c
    }

    private static func apply(_ st: Statement, file: String, to s: inout Schema) {
        let t = st.tokens
        let at = Location(file: file, line: st.line)
        guard let first = t.first, first.isWord else { return }
        var i = 1
        switch first.text {
        case "create":
            skip(t, &i, "or", "replace", "unlogged", "temporary", "temp", "global", "local")
            guard i < t.count else { return }
            if t[i].is("table") {
                i += 1
                let exists = ifClause(t, &i)
                guard let n = name(t, &i) else { return }
                if exists, s.tables[n] != nil { return }
                var table = Table(name: n, at: at)
                for part in parenList(t, &i) where !part.isEmpty {
                    let head = part[0]
                    if head.is("constraint") || head.is("primary") || head.is("unique") || head.is("foreign") || head.is("check") || head.is("exclude") || head.is("like") {
                        var cname = head.is("constraint") && part.count > 1 ? part[1].ident ?? "" : ""
                        if let pk = part.firstIndex(where: { $0.is("primary") }) {
                            var j = pk + 2
                            table.primaryKey = parenList(part, &j).compactMap { $0.first?.ident }
                            for k in table.primaryKey { if let ci = table.column(k) { table.columns[ci].notNull = true } }
                            if cname.isEmpty { cname = "\(n)_pkey" }
                        }
                        if !cname.isEmpty { table.constraints[cname] = at }
                        continue
                    }
                    if let c = column(part, at: Location(file: file, line: head.line)) {
                        table.columns.append(c)
                        if part.contains(where: { $0.is("primary") }) { table.primaryKey = [c.name] }
                    }
                }
                s.tables[n] = table
            } else if t[i].is("index") || (t[i].is("unique") && i + 1 < t.count && t[i + 1].is("index")) {
                let unique = t[i].is("unique")
                i += unique ? 2 : 1
                skip(t, &i, "concurrently")
                let exists = ifClause(t, &i)
                var n: String?
                if i < t.count, !t[i].is("on") { n = name(t, &i) }
                guard i < t.count, t[i].is("on") else { return }
                i += 1
                skip(t, &i, "only")
                guard let table = name(t, &i) else { return }
                let def = render(t[i...])
                let idx = n ?? "\(table)_\(def.filter { $0.isLetter || $0.isNumber || $0 == "_" }.prefix(30))_idx"
                if exists, s.indexes[idx] != nil { return }
                s.indexes[idx] = Index(name: idx, table: table, definition: def, unique: unique, at: at)
            } else if t[i].is("type") {
                i += 1
                guard let n = name(t, &i) else { return }
                if i + 1 < t.count, t[i].is("as"), t[i + 1].is("enum") {
                    i += 2
                    s.enums[n] = parenList(t, &i).compactMap { $0.first.map { $0.text.trimmingCharacters(in: CharacterSet(charactersIn: "'")) } }
                    s.enumAt[n] = at
                } else {
                    s.enums[n] = s.enums[n] ?? []
                    s.enumAt[n] = at
                }
            } else if t[i].is("view") || t[i].is("materialized") {
                i += t[i].is("materialized") ? 2 : 1
                _ = ifClause(t, &i)
                guard let n = name(t, &i) else { return }
                s.views[n] = (st.text, at)
            } else if t[i].is("function") || t[i].is("procedure") {
                i += 1
                guard let n = name(t, &i) else { return }
                s.functions[n] = (st.text, at)
            } else if t[i].is("trigger") || (t[i].is("constraint") && i + 1 < t.count && t[i + 1].is("trigger")) {
                i += t[i].is("constraint") ? 2 : 1
                guard let n = name(t, &i), let on = t.firstIndex(where: { $0.is("on") }) else { return }
                var j = on + 1
                s.triggers[n] = (name(t, &j) ?? "", at)
            } else if !t[i].is("extension") && !t[i].is("schema") && !t[i].is("sequence") {
                s.skipped.append(("CREATE " + t[i].text.uppercased(), at))
            }
        case "alter":
            guard i < t.count else { return }
            if t[i].is("table") {
                i += 1
                _ = ifClause(t, &i)
                skip(t, &i, "only")
                guard var n = name(t, &i), s.tables[n] != nil else { return }
                for action in commaSplit(t[i...]) {
                    alterTable(&n, action, at: at, in: &s)
                }
            } else if t[i].is("type") {
                i += 1
                guard let n = name(t, &i), i < t.count else { return }
                if t[i].is("add"), i + 1 < t.count, t[i + 1].is("value") {
                    i += 2; _ = ifClause(t, &i)
                    if i < t.count {
                        let v = t[i].text.trimmingCharacters(in: CharacterSet(charactersIn: "'"))
                        if !(s.enums[n] ?? []).contains(v) { s.enums[n, default: []].append(v) }
                    }
                } else if t[i].is("rename"), i + 1 < t.count {
                    if t[i + 1].is("value"), i + 4 < t.count {
                        let from = t[i + 2].text.trimmingCharacters(in: CharacterSet(charactersIn: "'"))
                        let to = t[i + 4].text.trimmingCharacters(in: CharacterSet(charactersIn: "'"))
                        s.enums[n] = s.enums[n]?.map { $0 == from ? to : $0 }
                    } else if t[i + 1].is("to") {
                        var j = i + 2
                        if let to = name(t, &j) { s.enums[to] = s.enums.removeValue(forKey: n); s.enumAt[to] = s.enumAt.removeValue(forKey: n) }
                    }
                }
            } else if t[i].is("index") {
                i += 1; _ = ifClause(t, &i)
                guard let n = name(t, &i), i + 1 < t.count, t[i].is("rename"), t[i + 1].is("to") else { return }
                var j = i + 2
                if let to = name(t, &j), var idx = s.indexes.removeValue(forKey: n) { idx.name = to; s.indexes[to] = idx }
            }
        case "drop":
            guard i < t.count else { return }
            let what = t[i].text
            i += 1
            if what == "materialized" { i += 1 }
            _ = ifClause(t, &i)
            if what == "index" { skip(t, &i, "concurrently"); _ = ifClause(t, &i) }
            var names: [String] = []
            for part in commaSplit(t[i...]) {
                var j = 0
                if let n = name(part, &j) { names.append(n) }
            }
            for n in names {
                switch what {
                case "table":
                    s.tables[n] = nil
                    s.indexes = s.indexes.filter { $0.value.table != n }
                    s.triggers = s.triggers.filter { $0.value.table != n }
                case "index": s.indexes[n] = nil
                case "type": s.enums[n] = nil; s.enumAt[n] = nil
                case "view", "materialized": s.views[n] = nil
                case "function", "procedure": s.functions[n] = nil
                case "trigger": s.triggers[n] = nil
                default: break
                }
            }
        case "do":
            s.skipped.append(("DO block", at))
        case "insert", "update", "delete", "select", "comment", "grant", "revoke", "set", "begin", "commit", "with", "truncate", "copy":
            break   // data and housekeeping: compared per file, not replayed
        default:
            s.skipped.append((first.text.uppercased(), at))
        }
    }

    private static func alterTable(_ n: inout String, _ a: [Token], at: Location, in s: inout Schema) {
        guard var table = s.tables[n], let verb = a.first else { return }
        var i = 1
        let colAt = Location(file: at.file, line: verb.line)
        switch verb.text {
        case "add":
            if i < a.count, a[i].is("constraint") || a[i].is("primary") || a[i].is("unique") || a[i].is("foreign") || a[i].is("check") || a[i].is("exclude") {
                var cname = a[i].is("constraint") && i + 1 < a.count ? a[i + 1].ident ?? "" : ""
                if let pk = a.firstIndex(where: { $0.is("primary") }) {
                    var j = pk + 2
                    table.primaryKey = parenList(a, &j).compactMap { $0.first?.ident }
                    for k in table.primaryKey { if let ci = table.column(k) { table.columns[ci].notNull = true } }
                    if cname.isEmpty { cname = "\(n)_pkey" }
                }
                if !cname.isEmpty { table.constraints[cname] = colAt }
            } else {
                skip(a, &i, "column")
                let exists = ifClause(a, &i)
                if let c = column(Array(a[i...]), at: colAt) {
                    if let ci = table.column(c.name) { if !exists { table.columns[ci] = c } } else { table.columns.append(c) }
                }
            }
        case "drop":
            if i < a.count, a[i].is("constraint") {
                i += 1; _ = ifClause(a, &i)
                if i < a.count, let c = a[i].ident { table.constraints[c] = nil; if c == "\(n)_pkey" { table.primaryKey = [] } }
            } else {
                skip(a, &i, "column")
                _ = ifClause(a, &i)
                if i < a.count, let c = a[i].ident { table.columns.removeAll { $0.name == c } }
            }
        case "alter":
            skip(a, &i, "column")
            guard i < a.count, let c = a[i].ident, let ci = table.column(c) else { break }
            i += 1
            guard i < a.count else { break }
            if a[i].is("type") || (a[i].is("set") && i + 2 < a.count && a[i + 1].is("data")) {
                i += a[i].is("type") ? 1 : 3
                let end = a[i...].firstIndex { $0.is("using") || $0.is("collate") } ?? a.count
                table.columns[ci].type = render(a[i..<end])
            } else if a[i].is("set"), i + 2 < a.count, a[i + 1].is("not"), a[i + 2].is("null") {
                table.columns[ci].notNull = true
            } else if a[i].is("drop"), i + 2 < a.count, a[i + 1].is("not"), a[i + 2].is("null") {
                table.columns[ci].notNull = false
            } else if a[i].is("set"), i + 1 < a.count, a[i + 1].is("default") {
                table.columns[ci].defaultValue = render(a[(i + 2)...])
            } else if a[i].is("drop"), i + 1 < a.count, a[i + 1].is("default") {
                table.columns[ci].defaultValue = nil
            }
            table.columns[ci].at = colAt
        case "rename":
            if i < a.count, a[i].is("to") {
                var j = i + 1
                if let to = name(a, &j) {
                    s.tables[n] = nil
                    table.name = to
                    for (k, v) in s.indexes where v.table == n { s.indexes[k]?.table = to }
                    for (k, v) in s.triggers where v.table == n { s.triggers[k]?.table = to }
                    n = to
                }
            } else if i < a.count, a[i].is("constraint") {
                if i + 3 < a.count, let from = a[i + 1].ident, let to = a[i + 3].ident, let loc = table.constraints.removeValue(forKey: from) { table.constraints[to] = loc }
            } else {
                skip(a, &i, "column")
                if i + 2 < a.count, let from = a[i].ident, a[i + 1].is("to"), let to = a[i + 2].ident, let ci = table.column(from) {
                    table.columns[ci].name = to
                    table.columns[ci].at = colAt
                }
            }
        default:
            break   // OWNER TO, ENABLE TRIGGER, SET (…): no shape change
        }
        s.tables[n] = table
    }
}

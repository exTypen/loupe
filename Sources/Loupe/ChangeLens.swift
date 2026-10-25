import Foundation

// MARK: - States A and B

/// One side of a change: a commit, the working tree, or nothing (before a root commit).
enum Snapshot: Sendable {
    case rev(String)
    case worktree
    case empty

    /// Every file path in this state.
    func list(in dir: String) async -> [String] {
        switch self {
        case .rev(let r):
            let out = (try? await Git.run(["ls-tree", "-r", "-z", "--name-only", r], in: dir)) ?? Data()
            return out.split(separator: 0).map { String(decoding: $0, as: UTF8.self) }
        case .worktree:
            let out = (try? await Git.run(["ls-files", "-z", "--cached", "--others", "--exclude-standard"], in: dir)) ?? Data()
            return out.split(separator: 0).map { String(decoding: $0, as: UTF8.self) }
                .filter { FileManager.default.fileExists(atPath: (dir as NSString).appendingPathComponent($0)) }
        case .empty:
            return []
        }
    }

    /// File contents (text files only; binary and missing ones are left out). One git process for any number of files.
    func read(_ paths: [String], in dir: String, maxSize: Int = 2_000_000) async -> [String: String] {
        guard !paths.isEmpty else { return [:] }
        switch self {
        case .empty: return [:]
        case .worktree:
            return await Task.detached {
                var out: [String: String] = [:]
                for p in paths {
                    let url = URL(fileURLWithPath: dir).appendingPathComponent(p)
                    guard let d = try? Data(contentsOf: url), d.count <= maxSize, !d.prefix(8000).contains(0) else { continue }
                    out[p] = String(decoding: d, as: UTF8.self)
                }
                return out
            }.value
        case .rev(let r):
            let input = Data(paths.map { "\(r):\($0)" }.joined(separator: "\n").utf8 + [10])
            guard let data = try? await Git.run(["cat-file", "--batch"], in: dir, stdin: input) else { return [:] }
            return await Task.detached { Self.parseBatch(data, paths: paths, maxSize: maxSize) }.value
        }
    }

    /// `git cat-file --batch` output: "<sha> <type> <size>\n<content>\n" per request, "<req> missing\n" when absent.
    nonisolated static func parseBatch(_ data: Data, paths: [String], maxSize: Int) -> [String: String] {
        var out: [String: String] = [:]
        var i = data.startIndex, k = 0
        while i < data.endIndex, k < paths.count {
            guard let nl = data[i...].firstIndex(of: 10) else { break }
            let header = String(decoding: data[i..<nl], as: UTF8.self).split(separator: " ")
            i = data.index(after: nl)
            defer { k += 1 }
            guard header.count == 3, let size = Int(header[2]) else { continue }   // "missing"
            let end = data.index(i, offsetBy: size)
            let body = data[i..<end]
            if header[1] == "blob", size <= maxSize, !body.prefix(8000).contains(0) { out[paths[k]] = String(decoding: body, as: UTF8.self) }
            i = data.index(after: end)   // the trailing newline
        }
        return out
    }
}

/// A file the change touches; nil on the side where it doesn't exist.
struct ChangedPath: Hashable, Sendable {
    var old: String?
    var new: String?
    var path: String { new ?? old ?? "" }
}

// MARK: - Facts

struct LensFact: Identifiable, Hashable, Sendable {
    enum Kind: Sendable { case added, removed, changed, renamed, info, warning }
    var id = UUID()
    var kind: Kind
    var title: String          // the thing: a table, column, package, function, key
    var detail: String? = nil  // what happened to it
    var path: String? = nil    // where to look
    var line: Int? = nil
}

struct LensSection: Identifiable, Sendable {
    var id = UUID()
    var title: String
    var facts: [LensFact]
}

struct LensReport: Identifiable, Sendable {
    var id: String { title }
    var title: String
    var icon: String           // SF Symbol
    var summary: String        // one line for the collapsed card
    var sections: [LensSection]
}

/// The structural story of a change, without AI: each lens recognises one kind of file and reports exact facts.
enum ChangeLens {
    static func analyze(_ changes: [ChangedPath], a: Snapshot, b: Snapshot, in dir: String) async -> [LensReport] {
        async let sql = SQLLens.report(changes, a: a, b: b, in: dir)
        async let deps = DependencyLens.report(changes, a: a, b: b, in: dir)
        async let code = CodeLens.report(changes, a: a, b: b, in: dir)
        async let config = ConfigLens.report(changes, a: a, b: b, in: dir)
        let files = FilesLens.report(changes)
        return [files] + [await sql, await deps, await config, await code].compactMap { $0 }
    }
}

// MARK: - Files

enum FilesLens {
    static func category(_ p: String) -> String {
        let l = p.lowercased(), name = (l as NSString).lastPathComponent, ext = (l as NSString).pathExtension
        if l.contains("test") || l.contains("spec") || l.hasPrefix("e2e/") || l.contains("/e2e/") || l.contains("__tests__") { return "Tests" }
        if ["md", "mdx", "rst", "txt", "adoc"].contains(ext) || l.hasPrefix("docs/") || l.contains("/docs/") { return "Docs" }
        if ext == "sql" { return "SQL" }
        if name.contains("lock") || ["cargo.toml", "package.json", "go.mod", "go.sum", "package.swift", "pyproject.toml", "requirements.txt", "flake.nix"].contains(name) { return "Dependencies" }
        if l.hasPrefix(".github/") || l.hasPrefix(".forgejo/") || l.hasPrefix(".gitea/") || name == "dockerfile" || name.hasPrefix("dockerfile.") || name.hasSuffix(".nix") { return "Build & CI" }
        if ["json", "yaml", "yml", "toml", "ini", "cfg", "conf", "env", "plist", "xml", "properties"].contains(ext) || name.hasPrefix(".env") { return "Config" }
        if ["png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "pdf", "ttf", "otf", "woff", "woff2", "mp4", "mov", "zip"].contains(ext) { return "Assets" }
        return "Code"
    }

    static func report(_ changes: [ChangedPath]) -> LensReport {
        let added = changes.filter { $0.old == nil }.count, removed = changes.filter { $0.new == nil }.count
        let renamed = changes.filter { $0.old != nil && $0.new != nil && $0.old != $0.new }.count
        let modified = changes.count - added - removed - renamed
        var groups: [String: [ChangedPath]] = [:]
        for c in changes { groups[category(c.path), default: []].append(c) }
        let order = ["Code", "Tests", "SQL", "Dependencies", "Config", "Build & CI", "Docs", "Assets"]
        let facts = order.compactMap { k -> LensFact? in
            guard let g = groups[k] else { return nil }
            let a = g.filter { $0.old == nil }.count, r = g.filter { $0.new == nil }.count
            var parts = ["\(g.count) file\(g.count == 1 ? "" : "s")"]
            if a > 0 { parts.append("\(a) new") }
            if r > 0 { parts.append("\(r) deleted") }
            return LensFact(kind: .info, title: k, detail: parts.joined(separator: " · "))
        }
        var summary = ["\(changes.count) file\(changes.count == 1 ? "" : "s")"]
        if added > 0 { summary.append("\(added) added") }
        if removed > 0 { summary.append("\(removed) deleted") }
        if renamed > 0 { summary.append("\(renamed) renamed") }
        if modified > 0 { summary.append("\(modified) modified") }
        return LensReport(title: "Files", icon: "doc.on.doc", summary: summary.joined(separator: " · "), sections: [LensSection(title: "By kind", facts: facts)])
    }
}

// MARK: - SQL

enum SQLLens {
    /// A directory of ordered SQL files that make up a schema, and which files in it are migrations to replay.
    struct Group { var root: String; var pattern: (String) -> Bool }

    static func isDown(_ p: String) -> Bool {
        let n = (p as NSString).lastPathComponent.lowercased()
        return n == "down.sql" || n.hasSuffix(".down.sql") || n.hasSuffix("_down.sql") || n.hasPrefix("u") && n.dropFirst().first?.isNumber == true   // flyway undo
    }

    static func report(_ changes: [ChangedPath], a: Snapshot, b: Snapshot, in dir: String) async -> LensReport? {
        let sqlChanges = changes.filter { $0.path.lowercased().hasSuffix(".sql") }
        guard !sqlChanges.isEmpty else { return nil }
        // Group: Diesel-style "<root>/<migration>/up.sql" replays every "<root>/*/up.sql"; otherwise a directory's .sql files.
        var roots: [String: Bool] = [:]   // root → diesel layout
        for c in sqlChanges {
            for p in [c.old, c.new].compactMap({ $0 }) {
                let parent = (p as NSString).deletingLastPathComponent
                let name = (p as NSString).lastPathComponent.lowercased()
                if name == "up.sql" || name == "down.sql" { roots[(parent as NSString).deletingLastPathComponent] = true }
                else { roots[parent] = roots[parent] ?? false }
            }
        }
        let listA = await a.list(in: dir), listB = await b.list(in: dir)
        var sections: [LensSection] = []
        var counts = (tables: 0, columns: 0, other: 0, data: 0)
        for (root, diesel) in roots.sorted(by: { $0.key < $1.key }) {
            func members(_ list: [String]) -> [String] {
                list.filter { p in
                    let parent = (p as NSString).deletingLastPathComponent
                    if diesel { return (parent as NSString).deletingLastPathComponent == root && (p as NSString).lastPathComponent.lowercased() == "up.sql" }
                    return parent == root && p.lowercased().hasSuffix(".sql") && !isDown(p)
                }.sorted()
            }
            let ma = members(listA), mb = members(listB)
            async let ta = a.read(ma, in: dir)
            async let tb = b.read(mb, in: dir)
            let (textA, textB) = (await ta, await tb)
            let (facts, warnings) = await Task.detached {
                let sa = SQL.replay(ma.compactMap { p in textA[p].map { (p, $0) } })
                let sb = SQL.replay(mb.compactMap { p in textB[p].map { (p, $0) } })
                var facts = diff(sa, sb)
                let changedHere = Set(sqlChanges.flatMap { [$0.old, $0.new].compactMap { $0 } })
                facts += dataChanges(changedHere.sorted().filter { members(listA + listB).contains($0) || ($0 as NSString).deletingLastPathComponent == root },
                                     a: textA, b: textB)
                let warnings = sb.skipped.filter { changedHere.contains($0.at.file) }
                return (facts, warnings)
            }.value
            for f in facts {
                if f.detail?.hasPrefix("table") == true { counts.tables += 1 }
                else if f.detail?.contains("column") == true { counts.columns += 1 }
                else if f.title.hasPrefix("INSERT") || f.title.hasPrefix("UPDATE") || f.title.hasPrefix("DELETE") || f.detail?.contains("row") == true { counts.data += 1 }
                else { counts.other += 1 }
            }
            var all = facts
            let byKind = Dictionary(grouping: warnings, by: \.kind)
            for (kind, ws) in byKind.sorted(by: { $0.key < $1.key }) {
                all.append(LensFact(kind: .warning, title: kind, detail: "\(ws.count) statement\(ws.count == 1 ? "" : "s") not replayed; read them in the diff",
                                    path: ws[0].at.file, line: ws[0].at.line))
            }
            if !all.isEmpty { sections.append(LensSection(title: root.isEmpty ? "/" : root, facts: all)) }
        }
        guard !sections.isEmpty else { return nil }
        var summary: [String] = []
        if counts.tables > 0 { summary.append("\(counts.tables) table change\(counts.tables == 1 ? "" : "s")") }
        if counts.columns > 0 { summary.append("\(counts.columns) column change\(counts.columns == 1 ? "" : "s")") }
        if counts.other > 0 { summary.append("\(counts.other) other schema change\(counts.other == 1 ? "" : "s")") }
        if counts.data > 0 { summary.append("\(counts.data) data change\(counts.data == 1 ? "" : "s")") }
        return LensReport(title: "Database", icon: "cylinder.split.1x2", summary: summary.isEmpty ? "SQL changed, schema the same" : summary.joined(separator: " · "), sections: sections)
    }

    /// Schema A → schema B, as facts.
    static func diff(_ a: SQL.Schema, _ b: SQL.Schema) -> [LensFact] {
        var out: [LensFact] = []
        func loc(_ l: SQL.Location?) -> (String?, Int?) { (l?.file, l?.line) }
        // Tables: a dropped and an added table with the same columns is a rename.
        var dropped = Set(a.tables.keys).subtracting(b.tables.keys), added = Set(b.tables.keys).subtracting(a.tables.keys)
        for old in dropped.sorted() {
            if let new = added.sorted().first(where: { b.tables[$0]!.columns == a.tables[old]!.columns && !a.tables[old]!.columns.isEmpty }) {
                let (p, l) = loc(b.tables[new]?.at)
                out.append(LensFact(kind: .renamed, title: "\(old) → \(new)", detail: "table renamed", path: p, line: l))
                dropped.remove(old); added.remove(new)
            }
        }
        for n in added.sorted() {
            let t = b.tables[n]!, (p, l) = loc(t.at)
            out.append(LensFact(kind: .added, title: n, detail: "table added · \(t.columns.count) column\(t.columns.count == 1 ? "" : "s"): " + t.columns.map(\.name).joined(separator: ", "), path: p, line: l))
        }
        for n in dropped.sorted() {
            let t = a.tables[n]!
            out.append(LensFact(kind: .removed, title: n, detail: "table dropped · had \(t.columns.count) column\(t.columns.count == 1 ? "" : "s")", path: t.at?.file, line: t.at?.line))
        }
        for n in Set(a.tables.keys).intersection(b.tables.keys).sorted() {
            let ta = a.tables[n]!, tb = b.tables[n]!
            let ca = Dictionary(ta.columns.map { ($0.name, $0) }, uniquingKeysWith: { x, _ in x })
            let cb = Dictionary(tb.columns.map { ($0.name, $0) }, uniquingKeysWith: { x, _ in x })
            for c in tb.columns where ca[c.name] == nil {
                var d = "column added · \(c.type)"
                if c.notNull { d += " NOT NULL" }
                if let v = c.defaultValue { d += " DEFAULT \(v)" }
                if let r = c.references { d += " → \(r)" }
                out.append(LensFact(kind: .added, title: "\(n).\(c.name)", detail: d, path: c.at?.file, line: c.at?.line))
            }
            for c in ta.columns where cb[c.name] == nil {
                out.append(LensFact(kind: .removed, title: "\(n).\(c.name)", detail: "column dropped · was \(c.type)", path: c.at?.file, line: c.at?.line))
            }
            for c in tb.columns {
                guard let o = ca[c.name], o != c else { continue }
                var what: [String] = []
                if o.type != c.type { what.append("type \(o.type) → \(c.type)") }
                if o.notNull != c.notNull { what.append(c.notNull ? "now NOT NULL" : "now nullable") }
                if o.defaultValue != c.defaultValue { what.append("default \(o.defaultValue ?? "none") → \(c.defaultValue ?? "none")") }
                if o.references != c.references { what.append("reference \(o.references ?? "none") → \(c.references ?? "none")") }
                out.append(LensFact(kind: .changed, title: "\(n).\(c.name)", detail: "column changed · " + what.joined(separator: ", "), path: c.at?.file, line: c.at?.line))
            }
            if ta.primaryKey != tb.primaryKey {
                out.append(LensFact(kind: .changed, title: n, detail: "primary key (\(ta.primaryKey.joined(separator: ", "))) → (\(tb.primaryKey.joined(separator: ", ")))", path: tb.at?.file, line: tb.at?.line))
            }
            for c in Set(tb.constraints.keys).subtracting(ta.constraints.keys).sorted() {
                let at = tb.constraints[c] ?? nil
                out.append(LensFact(kind: .added, title: "\(n): \(c)", detail: "constraint added", path: at?.file, line: at?.line))
            }
            for c in Set(ta.constraints.keys).subtracting(tb.constraints.keys).sorted() {
                out.append(LensFact(kind: .removed, title: "\(n): \(c)", detail: "constraint dropped"))
            }
        }
        // Indexes on tables that still exist (a dropped table's indexes go with it).
        for (n, i) in b.indexes.sorted(by: { $0.key < $1.key }) {
            if let o = a.indexes[n] {
                if o != i { out.append(LensFact(kind: .changed, title: n, detail: "index changed · \(i.unique ? "UNIQUE " : "")ON \(i.table) \(i.definition)", path: i.at?.file, line: i.at?.line)) }
            } else if a.tables[i.table] != nil || !added.contains(i.table) {
                out.append(LensFact(kind: .added, title: n, detail: "\(i.unique ? "unique " : "")index added · ON \(i.table) \(i.definition)", path: i.at?.file, line: i.at?.line))
            }
        }
        for (n, i) in a.indexes.sorted(by: { $0.key < $1.key }) where b.indexes[n] == nil && b.tables[i.table] != nil {
            out.append(LensFact(kind: .removed, title: n, detail: "index dropped · was ON \(i.table)"))
        }
        for (n, vals) in b.enums.sorted(by: { $0.key < $1.key }) {
            let at = b.enumAt[n]
            if let o = a.enums[n] {
                let add = vals.filter { !o.contains($0) }, rem = o.filter { !vals.contains($0) }
                if !add.isEmpty || !rem.isEmpty {
                    var d: [String] = []
                    if !add.isEmpty { d.append("+ " + add.joined(separator: ", ")) }
                    if !rem.isEmpty { d.append("− " + rem.joined(separator: ", ")) }
                    out.append(LensFact(kind: .changed, title: n, detail: "enum values " + d.joined(separator: "  "), path: at?.file, line: at?.line))
                }
            } else {
                out.append(LensFact(kind: .added, title: n, detail: vals.isEmpty ? "type added" : "enum added · " + vals.joined(separator: ", "), path: at?.file, line: at?.line))
            }
        }
        for n in Set(a.enums.keys).subtracting(b.enums.keys).sorted() { out.append(LensFact(kind: .removed, title: n, detail: "type dropped")) }
        func named(_ x: [String: (body: String, at: SQL.Location)], _ y: [String: (body: String, at: SQL.Location)], _ what: String) {
            for n in Set(y.keys).subtracting(x.keys).sorted() { out.append(LensFact(kind: .added, title: n, detail: "\(what) added", path: y[n]!.at.file, line: y[n]!.at.line)) }
            for n in Set(x.keys).subtracting(y.keys).sorted() { out.append(LensFact(kind: .removed, title: n, detail: "\(what) dropped")) }
            for n in Set(x.keys).intersection(y.keys).sorted() where x[n]!.body != y[n]!.body {
                out.append(LensFact(kind: .changed, title: n, detail: "\(what) redefined", path: y[n]!.at.file, line: y[n]!.at.line))
            }
        }
        named(a.views, b.views, "view")
        named(a.functions, b.functions, "function")
        for n in Set(b.triggers.keys).subtracting(a.triggers.keys).sorted() {
            out.append(LensFact(kind: .added, title: n, detail: "trigger added · ON \(b.triggers[n]!.table)", path: b.triggers[n]!.at.file, line: b.triggers[n]!.at.line))
        }
        for n in Set(a.triggers.keys).subtracting(b.triggers.keys).sorted() where b.tables[a.triggers[n]!.table] != nil {
            out.append(LensFact(kind: .removed, title: n, detail: "trigger dropped · was ON \(a.triggers[n]!.table)"))
        }
        return out
    }

    /// Data statements (INSERT / UPDATE / DELETE) that appear or disappear in a changed file, per table.
    static func dataChanges(_ paths: [String], a: [String: String], b: [String: String]) -> [LensFact] {
        var out: [LensFact] = []
        for p in paths {
            let sa = SQL.statements(a[p] ?? ""), sb = SQL.statements(b[p] ?? "")
            func data(_ s: [SQL.Statement]) -> [(key: String, st: SQL.Statement)] {
                s.compactMap { st in
                    guard let f = st.tokens.first, f.isWord else { return nil }
                    var i = 1
                    switch f.text {
                    case "insert": if i < st.tokens.count, st.tokens[i].is("into") { i += 1 }
                    case "delete": if i < st.tokens.count, st.tokens[i].is("from") { i += 1 }
                    case "update": break
                    default: return nil
                    }
                    guard i < st.tokens.count, var t = st.tokens[i].ident else { return nil }
                    if t == "public", i + 2 < st.tokens.count, st.tokens[i + 1].text == ".", let n = st.tokens[i + 2].ident { t = n }
                    return ("\(f.text.uppercased()) \(f.text == "insert" ? "into" : f.text == "delete" ? "from" : "") \(t)".replacingOccurrences(of: "  ", with: " "), st)
                }
            }
            let da = data(sa), db = data(sb)
            let textsA = Set(da.map(\.st.text)), textsB = Set(db.map(\.st.text))
            var removed: [String: (Int, Int)] = [:], added: [String: (Int, Int)] = [:], changed: [String: (Int, Int)] = [:]
            // Pair by key: a statement on the same table that exists on both sides but differs is "changed".
            let goneA = da.filter { !textsB.contains($0.st.text) }, newB = db.filter { !textsA.contains($0.st.text) }
            var newByKey = Dictionary(grouping: newB, by: \.key)
            for g in goneA {
                if var list = newByKey[g.key], !list.isEmpty {
                    let n = list.removeFirst(); newByKey[g.key] = list
                    changed[g.key, default: (0, n.st.line)].0 += 1
                } else { removed[g.key, default: (0, g.st.line)].0 += 1 }
            }
            for (k, list) in newByKey where !list.isEmpty { added[k, default: (0, list[0].st.line)].0 += list.count }
            for (k, v) in added.sorted(by: { $0.key < $1.key }) { out.append(LensFact(kind: .added, title: k, detail: "\(v.0) data statement\(v.0 == 1 ? "" : "s") added", path: p, line: v.1)) }
            for (k, v) in removed.sorted(by: { $0.key < $1.key }) { out.append(LensFact(kind: .removed, title: k, detail: "\(v.0) data statement\(v.0 == 1 ? "" : "s") removed", path: p, line: nil)) }
            for (k, v) in changed.sorted(by: { $0.key < $1.key }) { out.append(LensFact(kind: .changed, title: k, detail: "\(v.0) data statement\(v.0 == 1 ? "" : "s") changed", path: p, line: v.1)) }
        }
        return out
    }
}

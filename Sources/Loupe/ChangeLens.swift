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
    var tag: String? = nil     // what sort of thing it is ("function", "index") or how big ("major")
    var before: String? = nil  // the old value or code, when it's worth showing
    var after: String? = nil
    var count: Int? = nil
    var notes: [String] = []   // more about it, a line each: what a trigger runs, what a function raises
}

struct LensSection: Identifiable, Sendable {
    var id = UUID()
    var title: String
    var facts: [LensFact]
}

/// The headline number of a report, for the strip at the top of the overview.
struct LensMetric: Sendable {
    var value: Int
    var unit: String
    var added = 0, removed = 0, changed = 0
}

struct LensReport: Identifiable, Sendable {
    enum Kind: Sendable { case files, database, code, dependencies, configuration, followUps }
    var id: String { title }
    var kind: Kind
    var title: String
    var icon: String           // SF Symbol
    var summary: String        // one line for the collapsed card
    var sections: [LensSection]
    var metric: LensMetric
    var database: [DBGroup] = []
    var kinds: [String: [ChangedPath]] = [:]   // the files lens: the files of each kind
}

func plural(_ n: Int, _ word: String, _ many: String? = nil) -> String { "\(n) \(n == 1 ? word : many ?? word + "s")" }

/// The structural story of a change, without AI: each lens recognises one kind of file and reports exact facts.
enum ChangeLens {
    /// `upstream`: the tip of the branch the change goes into, when that's further on than A (a pull request's base
    /// branch): migrations are checked against what it already has.
    static func analyze(_ changes: [ChangedPath], a: Snapshot, b: Snapshot, upstream: String? = nil, in dir: String) async -> [LensReport] {
        async let sql = SQLLens.report(changes, a: a, b: b, upstream: upstream, in: dir)
        async let deps = DependencyLens.report(changes, a: a, b: b, in: dir)
        async let config = ConfigLens.report(changes, a: a, b: b, in: dir)
        async let lines = ChangedLines.read(changes, a: a, b: b, in: dir)
        let generated = await FilesLens.generated(changes, b: b, in: dir)
        let files = FilesLens.report(changes, generated: generated)
        let diff = await lines
        let code = await CodeLens.report(diff, changes: changes, a: a, b: b, in: dir)
        let notes = FollowUpLens.report(diff.added, skip: { p in generated.contains(p) || ["Generated", "Docs"].contains(FilesLens.category(p)) })
        return [files] + [await sql, code, await deps, await config, notes].compactMap { $0 }
    }
}

// MARK: - Files

enum FilesLens {
    static let order = ["Code", "Tests", "SQL", "Dependencies", "Config", "Build & CI", "Docs", "Assets", "Generated"]

    /// What sort of file a path is. `generated`: the file says a tool wrote it (see `generated(_:b:in:)`).
    static func category(_ p: String, generated: Bool = false) -> String {
        let l = p.lowercased(), name = (l as NSString).lastPathComponent, ext = (l as NSString).pathExtension
        let dirs = l.split(separator: "/").dropLast()
        if generated || generatedName(l) { return "Generated" }
        if dirs.contains(where: { ["test", "tests", "__tests__", "__mocks__", "spec", "specs", "e2e", "acceptance", "testdata", "testing", "fixtures"].contains($0) }) || ext == "feature" { return "Tests" }
        if ["md", "mdx", "rst", "txt", "adoc"].contains(ext) || dirs.contains("docs") { return "Docs" }
        // foo_test.go, foo.test.ts, test_foo.py, foo_e2e.rs, FooTests.swift; not latest.rs or Inspector.swift.
        let stem = String((p as NSString).lastPathComponent.split(separator: ".").first ?? "")
        if name.split(whereSeparator: { "._-".contains($0) }).contains(where: { ["test", "tests", "spec", "e2e"].contains($0) })
            || stem.hasSuffix("Test") || stem.hasSuffix("Tests") || stem.hasSuffix("Spec") { return "Tests" }
        if ext == "sql" { return "SQL" }
        if ["cargo.toml", "package.json", "go.mod", "package.swift", "pyproject.toml", "requirements.txt", "flake.nix", "gemfile", "composer.json"].contains(name) { return "Dependencies" }
        if l.hasPrefix(".github/") || l.hasPrefix(".forgejo/") || l.hasPrefix(".gitea/") || name == "dockerfile" || name.hasPrefix("dockerfile.") || name.hasSuffix(".nix") { return "Build & CI" }
        // An animation's JSON under assets/ is content the app ships, not configuration.
        if ["json", "xml"].contains(ext), dirs.contains(where: { ["assets", "public", "static", "res", "resources"].contains($0) }) { return "Assets" }
        if ["json", "yaml", "yml", "toml", "ini", "cfg", "conf", "env", "plist", "xml", "properties"].contains(ext) || name.hasPrefix(".env") { return "Config" }
        if ["png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "pdf", "ttf", "otf", "woff", "woff2", "mp4", "mov", "zip"].contains(ext) { return "Assets" }
        return "Code"
    }

    /// Lock files and the usual names of generated code (a lowercased path).
    static func generatedName(_ l: String) -> Bool {
        let name = (l as NSString).lastPathComponent, ext = (name as NSString).pathExtension
        if ["lock", "lockb", "lockfile"].contains(ext) || name.hasSuffix("-lock.json") || name.hasSuffix("-lock.yaml")
            || ["go.sum", "package.resolved", "cartfile.resolved", "packages.lock.json", "npm-shrinkwrap.json"].contains(name) { return true }
        if name.contains(".min.") || name.contains(".generated.") || [".pb.go", "_pb2.py", ".pb.swift", ".g.dart", ".freezed.dart"].contains(where: { name.hasSuffix($0) }) { return true }
        return l.split(separator: "/").dropLast().contains { $0 == "generated" || $0 == "__generated__" }
    }

    /// Changed files that say in their first lines that a tool wrote them.
    static func generated(_ changes: [ChangedPath], b: Snapshot, in dir: String) async -> Set<String> {
        let paths = changes.compactMap(\.new).filter { !generatedName($0.lowercased()) && category($0) != "Assets" }
        let texts = await b.read(Array(paths.prefix(1000)), in: dir, maxSize: 1_000_000)
        return Set(texts.compactMap { p, t in
            t.split(separator: "\n", maxSplits: 5, omittingEmptySubsequences: false).prefix(5).contains {
                $0.range(of: #"@generated|do not edit|code generated|auto-?generated|automatically generated"#, options: [.regularExpression, .caseInsensitive]) != nil
            } ? p : nil
        })
    }

    static func report(_ changes: [ChangedPath], generated: Set<String> = []) -> LensReport {
        let added = changes.filter { $0.old == nil }.count, removed = changes.filter { $0.new == nil }.count
        let renamed = changes.filter { $0.old != nil && $0.new != nil && $0.old != $0.new }.count
        let modified = changes.count - added - removed - renamed
        var groups: [String: [ChangedPath]] = [:]
        for c in changes { groups[category(c.path, generated: generated.contains(c.path)), default: []].append(c) }
        let facts = order.compactMap { k -> LensFact? in
            guard let g = groups[k] else { return nil }
            let a = g.filter { $0.old == nil }.count, r = g.filter { $0.new == nil }.count
            var parts = [plural(g.count, "file")]
            if a > 0 { parts.append("\(a) new") }
            if r > 0 { parts.append("\(r) deleted") }
            return LensFact(kind: .info, title: k, detail: parts.joined(separator: " · "), count: g.count)
        }
        var summary = [plural(changes.count, "file")]
        if added > 0 { summary.append("\(added) added") }
        if removed > 0 { summary.append("\(removed) deleted") }
        if renamed > 0 { summary.append("\(renamed) renamed") }
        if modified > 0 { summary.append("\(modified) modified") }
        return LensReport(kind: .files, title: "Files", icon: "doc.on.doc", summary: summary.joined(separator: " · "),
                          sections: [LensSection(title: "By kind", facts: facts)],
                          metric: LensMetric(value: changes.count, unit: changes.count == 1 ? "file" : "files", added: added, removed: removed, changed: modified + renamed),
                          kinds: groups)
    }
}

// MARK: - SQL

struct DBColumn: Identifiable, Sendable {
    var id: String { name }
    var name: String
    var type: String
    var oldType: String? = nil     // when the type changed
    var notNull = false
    var defaultValue: String? = nil
    var references: String? = nil
    var primary = false
    var kind: LensFact.Kind = .info  // .info: unchanged
    var notes: [String] = []         // what changed, in words
    var path: String? = nil
    var line: Int? = nil
}

struct DBTable: Identifiable, Sendable {
    var id: String { name }
    var name: String
    var oldName: String? = nil
    var kind: LensFact.Kind
    var columns: [DBColumn]
    var extras: [LensFact] = []      // keys, constraints, indexes, triggers
    var sql: String? = nil           // the CREATE TABLE as written; the old one for a dropped table
    var path: String? = nil
    var line: Int? = nil
}

/// Enum types, views, functions, and indexes or triggers on tables the schema doesn't define.
struct DBObject: Identifiable, Sendable {
    var id: String { "\(type) \(name)" }
    var type: String
    var name: String
    var kind: LensFact.Kind
    var detail: String
    var before: String? = nil
    var after: String? = nil
    var notes: [String] = []
    var path: String? = nil
    var line: Int? = nil
}

/// INSERT / UPDATE / DELETE statements on one table that appear, disappear or change in one file.
struct DBData: Identifiable, Sendable {
    var id: String { "\(path) \(kind) \(verb) \(table)" }
    var verb: String
    var table: String
    var kind: LensFact.Kind
    var before: [String] = []
    var after: [String] = []
    var path: String
    var line: Int? = nil
    var title: String { [verb, verb == "INSERT" ? "into" : verb == "DELETE" ? "from" : "", table].filter { !$0.isEmpty }.joined(separator: " ") }
    var count: Int { max(before.count, after.count) }
}

/// One schema (a migrations directory), A → B.
struct DBGroup: Identifiable, Sendable {
    var id: String { root }
    var root: String
    var tables: [DBTable] = []
    var objects: [DBObject] = []
    var data: [DBData] = []
    var warnings: [LensFact] = []

    /// The same, as flat facts: for the command line and tests.
    var facts: [LensFact] {
        var out: [LensFact] = []
        for t in tables {
            switch t.kind {
            case .added: out.append(LensFact(kind: .added, title: t.name, detail: "table added · \(plural(t.columns.count, "column")): " + t.columns.map(\.name).joined(separator: ", "), path: t.path, line: t.line))
            case .removed: out.append(LensFact(kind: .removed, title: t.name, detail: "table dropped · had \(plural(t.columns.count, "column"))", path: t.path, line: t.line))
            case .renamed: out.append(LensFact(kind: .renamed, title: "\(t.oldName ?? "?") → \(t.name)", detail: "table renamed", path: t.path, line: t.line))
            default:
                for c in t.columns {
                    switch c.kind {
                    case .added:
                        var d = "column added · \(c.type)"
                        if c.notNull { d += " NOT NULL" }
                        if let v = c.defaultValue { d += " DEFAULT \(v)" }
                        if let r = c.references { d += " → \(r)" }
                        out.append(LensFact(kind: .added, title: "\(t.name).\(c.name)", detail: d, path: c.path, line: c.line))
                    case .removed: out.append(LensFact(kind: .removed, title: "\(t.name).\(c.name)", detail: "column dropped · was \(c.type)", path: c.path, line: c.line))
                    case .changed: out.append(LensFact(kind: .changed, title: "\(t.name).\(c.name)", detail: "column changed · " + c.notes.joined(separator: ", "), path: c.path, line: c.line))
                    default: break
                    }
                }
            }
            out += t.extras.map { var f = $0; f.title = "\(t.name): \(f.title)"; f.detail = ([f.tag, f.detail].compactMap { $0 } + f.notes).joined(separator: " · "); return f }
        }
        out += objects.map { LensFact(kind: $0.kind, title: $0.name, detail: $0.detail, path: $0.path, line: $0.line) }
        out += data.map { d in
            let what = d.kind == .added ? "added" : d.kind == .removed ? "removed" : "changed"
            return LensFact(kind: d.kind, title: d.title, detail: "\(plural(d.count, "data statement")) \(what)", path: d.path, line: d.line)
        }
        return out + warnings
    }
}

enum SQLLens {
    static func isDown(_ p: String) -> Bool {
        let n = (p as NSString).lastPathComponent.lowercased()
        return n == "down.sql" || n.hasSuffix(".down.sql") || n.hasSuffix("_down.sql") || n.hasPrefix("u") && n.dropFirst().first?.isNumber == true   // flyway undo
    }

    /// A migration's version, from the leading number of its name: "2026-11-03-100000_x" → [2026, 11, 3, 100000],
    /// "V2_1__x.sql" → [2, 1], "0042_x.up.sql" → [42]; nil when the name doesn't start with one.
    static func version(_ name: String) -> [Int]? {
        var s = Substring(name)
        if let f = s.first, "VvUuRr".contains(f), s.dropFirst().first?.isNumber == true { s = s.dropFirst() }
        let v = s.prefix { $0.isNumber || "-_.".contains($0) }.split { !$0.isNumber }.compactMap { Int($0) }
        return v.isEmpty ? nil : v
    }

    static func report(_ changes: [ChangedPath], a: Snapshot, b: Snapshot, upstream: String? = nil, in dir: String) async -> LensReport? {
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
        let listU = upstream == nil ? [] : await Snapshot.rev(upstream!).list(in: dir)
        var groups: [DBGroup] = []
        var renames: [String: String] = [:]   // old → new table name, so seed files elsewhere pair statements across a rename
        // Migrations before seed directories, so their renames are known when the seeds are read.
        let ordered = roots.sorted { ($0.value ? 0 : $0.key.lowercased().contains("migrat") ? 1 : 2, $0.key) < ($1.value ? 0 : $1.key.lowercased().contains("migrat") ? 1 : 2, $1.key) }
        for (root, diesel) in ordered {
            func members(_ list: [String]) -> [String] {
                list.filter { p in
                    let parent = (p as NSString).deletingLastPathComponent
                    if diesel { return (parent as NSString).deletingLastPathComponent == root && (p as NSString).lastPathComponent.lowercased() == "up.sql" }
                    return parent == root && p.lowercased().hasSuffix(".sql") && !isDown(p)
                }.sorted()
            }
            let ma = members(listA), mb = members(listB), mu = members(listU)
            async let ta = a.read(ma, in: dir)
            async let tb = b.read(mb, in: dir)
            let (textA, textB) = (await ta, await tb)
            let known = renames
            let g = await Task.detached { () -> DBGroup in
                let sa = SQL.replay(ma.compactMap { p in textA[p].map { (p, $0) } })
                let sb = SQL.replay(mb.compactMap { p in textB[p].map { (p, $0) } })
                var g = DBGroup(root: root.isEmpty ? "/" : root)
                (g.tables, g.objects) = model(sa, sb, textA: textA, textB: textB)
                let changedHere = Set(sqlChanges.flatMap { [$0.old, $0.new].compactMap { $0 } })
                let all = Set(members(listA + listB))
                let renames = known.merging(g.tables.compactMap { t in t.oldName.map { ($0, t.name) } }) { x, _ in x }
                g.data = dataChanges(changedHere.sorted().filter { all.contains($0) || ($0 as NSString).deletingLastPathComponent == root }, a: textA, b: textB, renames: renames)
                // New migrations against the ones already there: the same version, or older than some of them.
                func id(_ p: String) -> String { ((diesel ? (p as NSString).deletingLastPathComponent : p) as NSString).lastPathComponent }
                let inA = Set(ma)
                let new = mb.filter { !inA.contains($0) && changedHere.contains($0) }
                let there = Set(ma + mu).subtracting(new).compactMap { p in version(id(p)).map { (id: id(p), v: $0) } }
                for p in new {
                    guard let v = version(id(p)) else { continue }
                    let others = there.filter { $0.id != id(p) }
                    if let same = others.first(where: { $0.v == v }) {
                        g.warnings.append(LensFact(kind: .warning, title: "Migration version already used", path: p, line: 1,
                                                   notes: ["\(id(p)) has the same version as \(same.id). Tools that track migrations by version take one of them as already run."]))
                    }
                    let later = others.filter { v.lexicographicallyPrecedes($0.v) }.sorted { $0.v.lexicographicallyPrecedes($1.v) }
                    if let last = later.last {
                        g.warnings.append(LensFact(kind: .warning, title: "Migration older than \(plural(later.count, "existing one"))", path: p, line: 1,
                                                   notes: ["\(id(p)) sorts before \(last.id)\(later.count > 1 ? " and \(later.count - 1) more" : ""). Databases that already ran those run this one out of order."]))
                    }
                }
                // What the changed migrations cost to run: locks, rewrites, lost data.
                let existing = Set(sa.tables.keys)
                let migration = diesel || root.lowercased().contains("migrat")
                for p in mb where changedHere.contains(p) {
                    guard let text = textB[p] else { continue }
                    let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
                    let old = Set(textA[p].map { SQL.statements($0).map(\.text) } ?? [])
                    for st in SQL.statements(text) where !old.contains(st.text) {
                        for r in SQL.risks(st, existing: existing, migration: migration) {
                            g.warnings.append(LensFact(kind: .warning, title: r.title, path: p, line: st.line, after: st.source(lines), notes: [r.detail]))
                        }
                    }
                }
                let skipped = sb.skipped.filter { changedHere.contains($0.at.file) }
                for (kind, ws) in Dictionary(grouping: skipped, by: \.kind).sorted(by: { $0.key < $1.key }) {
                    let at = ws[0].at
                    let src = textB[at.file].flatMap { t in SQL.statements(t).first { $0.line == at.line }?.source(t.split(separator: "\n", omittingEmptySubsequences: false)) }
                    g.warnings.append(LensFact(kind: .warning, title: "\(kind) not replayed", detail: "\(plural(ws.count, "statement")) not replayed",
                                               path: at.file, line: at.line, after: src, count: ws.count,
                                               notes: ["Loupe doesn't run procedural SQL, so the schema shown doesn't include \(ws.count == 1 ? "its" : "their") effect."]))
                }
                return g
            }.value
            for t in g.tables { if let o = t.oldName { renames[o] = t.name } }
            if !(g.tables.isEmpty && g.objects.isEmpty && g.data.isEmpty && g.warnings.isEmpty) { groups.append(g) }
        }
        guard !groups.isEmpty else { return nil }
        let tables = groups.flatMap(\.tables), objects = groups.flatMap(\.objects), data = groups.flatMap(\.data)
        let n = (added: tables.filter { $0.kind == .added }.count, removed: tables.filter { $0.kind == .removed }.count,
                 renamed: tables.filter { $0.kind == .renamed }.count, changed: tables.filter { $0.kind == .changed }.count)
        var summary: [String] = []
        if n.added > 0 { summary.append(plural(n.added, "new table")) }
        if n.removed > 0 { summary.append(plural(n.removed, "table") + " dropped") }
        if n.renamed > 0 { summary.append(plural(n.renamed, "table") + " renamed") }
        if n.changed > 0 { summary.append(plural(n.changed, "table") + " changed") }
        for (type, list) in Dictionary(grouping: objects, by: \.type).sorted(by: { $0.key < $1.key }) {
            let verbs = Set(list.map(\.kind))
            let verb = verbs == [.added] ? " added" : verbs == [.removed] ? " dropped" : " changed"
            summary.append(plural(list.count, type) + verb)
        }
        if !data.isEmpty { summary.append(plural(data.reduce(0) { $0 + $1.count }, "data statement")) }
        let metric = tables.isEmpty
            ? LensMetric(value: objects.count + data.count, unit: "schema changes", added: objects.filter { $0.kind == .added }.count + data.filter { $0.kind == .added }.count,
                         removed: objects.filter { $0.kind == .removed }.count + data.filter { $0.kind == .removed }.count,
                         changed: objects.filter { $0.kind == .changed }.count + data.filter { $0.kind == .changed }.count)
            : LensMetric(value: tables.count, unit: tables.count == 1 ? "table" : "tables", added: n.added, removed: n.removed, changed: n.changed + n.renamed)
        return LensReport(kind: .database, title: "Database", icon: "cylinder.split.1x2",
                          summary: summary.isEmpty ? "SQL changed, schema the same" : summary.joined(separator: " · "),
                          sections: groups.map { LensSection(title: $0.root, facts: $0.facts) }, metric: metric, database: groups)
    }

    /// Schema A → schema B, as flat facts.
    static func diff(_ a: SQL.Schema, _ b: SQL.Schema) -> [LensFact] {
        var g = DBGroup(root: "")
        (g.tables, g.objects) = model(a, b)
        return g.facts
    }

    /// Schema A → schema B: every table that changed with all its columns (unchanged ones too, for context), and the
    /// other objects. `textA` / `textB` are the files, for showing definitions as written.
    static func model(_ a: SQL.Schema, _ b: SQL.Schema, textA: [String: String] = [:], textB: [String: String] = [:]) -> (tables: [DBTable], objects: [DBObject]) {
        var parsed: [String: (lines: [Substring], statements: [SQL.Statement])] = [:]
        func source(_ at: SQL.Location?, old: Bool) -> String? {
            guard let at, let t = (old ? textA : textB)[at.file] else { return nil }
            let key = (old ? "a:" : "b:") + at.file
            if parsed[key] == nil { parsed[key] = (t.split(separator: "\n", omittingEmptySubsequences: false), SQL.statements(t)) }
            let p = parsed[key]!
            return p.statements.first { $0.line == at.line }?.source(p.lines)
        }
        /// A constraint as written: its lines from the migration, without the indentation they share.
        func written(_ c: SQL.Constraint, old: Bool) -> String {
            guard let t = (old ? textA : textB)[c.at.file] else { return c.definition }
            let lines = t.split(separator: "\n", omittingEmptySubsequences: false)
            guard c.at.line >= 1, c.endLine <= lines.count, c.at.line <= c.endLine else { return c.definition }
            var ls = lines[(c.at.line - 1)...(c.endLine - 1)].map(String.init)
            // Just the constraint: not the ALTER TABLE … ADD or CONSTRAINT <name> it was written after.
            ls[0] = ls[0].replacingOccurrences(of: #"(?i)^(\s*)(ALTER\s+TABLE\s+(ONLY\s+)?\S+\s+ADD\s+)?(CONSTRAINT\s+\S+\s+)?"#, with: "$1", options: .regularExpression)
            let indent = ls.filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }.map { $0.prefix { $0 == " " }.count }.min() ?? 0
            return ls.map { String($0.dropFirst(min(indent, $0.prefix { $0 == " " }.count))) }.joined(separator: "\n")
                .trimmingCharacters(in: CharacterSet(charactersIn: ",;").union(.whitespacesAndNewlines))
        }
        func col(_ c: SQL.Column, _ t: SQL.Table, _ kind: LensFact.Kind) -> DBColumn {
            DBColumn(name: c.name, type: c.type, notNull: c.notNull, defaultValue: c.defaultValue, references: c.references,
                     primary: t.primaryKey.contains(c.name), kind: kind, path: c.at?.file, line: c.at?.line)
        }
        var tables: [String: DBTable] = [:]
        var context: [String: DBTable] = [:]   // tables that exist on both sides unchanged, in case an index or trigger on them changed
        // A dropped and an added table with the same columns is a rename.
        var dropped = Set(a.tables.keys).subtracting(b.tables.keys), added = Set(b.tables.keys).subtracting(a.tables.keys)
        for old in dropped.sorted() {
            guard let new = added.sorted().first(where: { b.tables[$0]!.columns == a.tables[old]!.columns && !a.tables[old]!.columns.isEmpty }) else { continue }
            let t = b.tables[new]!
            tables[new] = DBTable(name: new, oldName: old, kind: .renamed, columns: t.columns.map { col($0, t, .info) }, path: t.at?.file, line: t.at?.line)
            dropped.remove(old); added.remove(new)
        }
        for n in added {
            let t = b.tables[n]!
            tables[n] = DBTable(name: n, kind: .added, columns: t.columns.map { col($0, t, .added) }, sql: source(t.at, old: false), path: t.at?.file, line: t.at?.line)
        }
        for n in dropped {
            let t = a.tables[n]!
            tables[n] = DBTable(name: n, kind: .removed, columns: t.columns.map { col($0, t, .removed) }, sql: source(t.at, old: true), path: t.at?.file, line: t.at?.line)
        }
        var extras: [String: [LensFact]] = [:]
        for n in Set(a.tables.keys).intersection(b.tables.keys) {
            let ta = a.tables[n]!, tb = b.tables[n]!
            let ca = Dictionary(ta.columns.map { ($0.name, $0) }, uniquingKeysWith: { x, _ in x })
            let cb = Dictionary(tb.columns.map { ($0.name, $0) }, uniquingKeysWith: { x, _ in x })
            var cols: [DBColumn] = [], changed = false
            for c in tb.columns {
                guard let o = ca[c.name] else { cols.append(col(c, tb, .added)); changed = true; continue }
                var d = col(c, tb, .info)
                if o != c {
                    if o.type != c.type { d.oldType = o.type; d.notes.append("type \(o.type) → \(c.type)") }
                    if o.notNull != c.notNull { d.notes.append(c.notNull ? "now NOT NULL" : "now nullable") }
                    if o.defaultValue != c.defaultValue { d.notes.append("default \(o.defaultValue ?? "none") → \(c.defaultValue ?? "none")") }
                    if o.references != c.references { d.notes.append("reference \(o.references ?? "none") → \(c.references ?? "none")") }
                    d.kind = .changed; changed = true
                }
                cols.append(d)
            }
            // Dropped columns go back where they were: after the column that preceded them.
            for (i, c) in ta.columns.enumerated() where cb[c.name] == nil {
                let prev = ta.columns[..<i].last { x in cols.contains { $0.name == x.name } }
                let at = prev.flatMap { p in cols.firstIndex { $0.name == p.name } }.map { $0 + 1 } ?? 0
                cols.insert(col(c, ta, .removed), at: at); changed = true
            }
            if ta.primaryKey != tb.primaryKey {
                extras[n, default: []].append(LensFact(kind: .changed, title: "primary key", path: tb.at?.file, line: tb.at?.line, tag: "key",
                                                       before: "(\(ta.primaryKey.joined(separator: ", ")))", after: "(\(tb.primaryKey.joined(separator: ", ")))"))
            }
            // Constraints by name: added, dropped, or re-created with another definition (DROP + ADD of one name).
            for (c, con) in tb.constraints.sorted(by: { $0.key < $1.key }) {
                if let o = ta.constraints[c] {
                    guard o.definition != con.definition else { continue }
                    extras[n, default: []].append(LensFact(kind: .changed, title: c, detail: "constraint changed", path: con.at.file, line: con.at.line,
                                                           tag: "constraint", before: written(o, old: true), after: written(con, old: false)))
                } else {
                    extras[n, default: []].append(LensFact(kind: .added, title: c, detail: "constraint added", path: con.at.file, line: con.at.line,
                                                           tag: "constraint", after: written(con, old: false)))
                }
            }
            for (c, con) in ta.constraints.sorted(by: { $0.key < $1.key }) where tb.constraints[c] == nil {
                extras[n, default: []].append(LensFact(kind: .removed, title: c, detail: "constraint dropped", tag: "constraint", before: written(con, old: true)))
            }
            let t = DBTable(name: n, kind: .changed, columns: cols, path: tb.at?.file, line: tb.at?.line)
            if changed { tables[n] = t } else { context[n] = t }
        }
        func unique(_ i: SQL.Index) -> String { "\(i.unique ? "UNIQUE " : "")ON \(i.table) \(i.definition)" }
        for (n, i) in b.indexes {
            if let o = a.indexes[n] {
                if o != i { extras[i.table, default: []].append(LensFact(kind: .changed, title: n, detail: "index changed", path: i.at?.file, line: i.at?.line, tag: "index", before: unique(o), after: unique(i))) }
            } else {
                extras[i.table, default: []].append(LensFact(kind: .added, title: n, detail: "\(i.unique ? "unique " : "")index added", path: i.at?.file, line: i.at?.line, tag: "index", after: unique(i)))
            }
        }
        for (n, i) in a.indexes where b.indexes[n] == nil && b.tables[i.table] != nil {
            extras[i.table, default: []].append(LensFact(kind: .removed, title: n, detail: "index dropped", tag: "index", before: unique(i)))
        }
        /// When a trigger fires, what it runs, and what that raises.
        func about(_ t: SQL.Trigger, _ s: SQL.Schema) -> [String] {
            [t.detail] + (t.function.flatMap { s.functions[$0] }.map { SQL.raises($0.body).map { "Raises \($0)" } } ?? [])
        }
        for (n, t) in b.triggers {
            if let o = a.triggers[n] {
                guard o.detail != t.detail || o.table != t.table else { continue }
                extras[t.table, default: []].append(LensFact(kind: .changed, title: n, detail: "trigger changed", path: t.at.file, line: t.at.line, tag: "trigger",
                                                             before: source(o.at, old: true), after: source(t.at, old: false), notes: ["was " + o.detail] + about(t, b)))
            } else {
                extras[t.table, default: []].append(LensFact(kind: .added, title: n, detail: "trigger added", path: t.at.file, line: t.at.line, tag: "trigger",
                                                             after: source(t.at, old: false), notes: about(t, b)))
            }
        }
        for (n, t) in a.triggers where b.triggers[n] == nil && b.tables[t.table] != nil {
            extras[t.table, default: []].append(LensFact(kind: .removed, title: n, detail: "trigger dropped", tag: "trigger", before: source(t.at, old: true), notes: about(t, a)))
        }
        var objects: [DBObject] = []
        for (table, facts) in extras {
            let sorted = facts.sorted { ($0.tag ?? "", $0.title) < ($1.tag ?? "", $1.title) }
            if tables[table] != nil { tables[table]!.extras = sorted }
            else if var t = context[table] { t.extras = sorted; tables[table] = t }
            else { objects += sorted.map { DBObject(type: $0.tag ?? "index", name: $0.title, kind: $0.kind, detail: "\($0.detail ?? "") · ON \(table)", before: $0.before, after: $0.after, notes: $0.notes, path: $0.path, line: $0.line) } }
        }
        for (n, vals) in b.enums.sorted(by: { $0.key < $1.key }) {
            let at = b.enumAt[n]
            if let o = a.enums[n] {
                let add = vals.filter { !o.contains($0) }, rem = o.filter { !vals.contains($0) }
                guard !add.isEmpty || !rem.isEmpty else { continue }
                var d: [String] = []
                if !add.isEmpty { d.append("+ " + add.joined(separator: ", ")) }
                if !rem.isEmpty { d.append("− " + rem.joined(separator: ", ")) }
                objects.append(DBObject(type: "type", name: n, kind: .changed, detail: "enum values " + d.joined(separator: "  "),
                                        before: o.joined(separator: ", "), after: vals.joined(separator: ", "), path: at?.file, line: at?.line))
            } else {
                objects.append(DBObject(type: "type", name: n, kind: .added, detail: vals.isEmpty ? "type added" : "enum added · " + vals.joined(separator: ", "),
                                        after: vals.isEmpty ? nil : vals.joined(separator: ", "), path: at?.file, line: at?.line))
            }
        }
        for n in Set(a.enums.keys).subtracting(b.enums.keys).sorted() {
            objects.append(DBObject(type: "type", name: n, kind: .removed, detail: "type dropped", before: a.enums[n]!.joined(separator: ", ")))
        }
        func named(_ x: [String: (body: String, at: SQL.Location)], _ y: [String: (body: String, at: SQL.Location)], _ what: String) {
            func raises(_ body: String) -> [String] { SQL.raises(body).map { "Raises \($0)" } }
            for n in Set(y.keys).subtracting(x.keys).sorted() {
                objects.append(DBObject(type: what, name: n, kind: .added, detail: "\(what) added", after: source(y[n]!.at, old: false), notes: raises(y[n]!.body),
                                        path: y[n]!.at.file, line: y[n]!.at.line))
            }
            for n in Set(x.keys).subtracting(y.keys).sorted() {
                objects.append(DBObject(type: what, name: n, kind: .removed, detail: "\(what) dropped", before: source(x[n]!.at, old: true)))
            }
            for n in Set(x.keys).intersection(y.keys).sorted() where x[n]!.body != y[n]!.body {
                objects.append(DBObject(type: what, name: n, kind: .changed, detail: "\(what) redefined", before: source(x[n]!.at, old: true), after: source(y[n]!.at, old: false),
                                        notes: raises(y[n]!.body), path: y[n]!.at.file, line: y[n]!.at.line))
            }
        }
        named(a.views, b.views, "view")
        named(a.functions, b.functions, "function")
        let rank: [LensFact.Kind: Int] = [.removed: 0, .added: 1, .renamed: 2, .changed: 3]
        return (tables.values.sorted { (rank[$0.kind] ?? 9, $0.name) < (rank[$1.kind] ?? 9, $1.name) }, objects)
    }

    /// Data statements (INSERT / UPDATE / DELETE) that appear, disappear or change in a changed file, per table.
    /// `renames` (old → new table name) pairs a statement on a renamed table with its counterpart.
    static func dataChanges(_ paths: [String], a: [String: String], b: [String: String], renames: [String: String] = [:]) -> [DBData] {
        var out: [DBData] = []
        for p in paths {
            let ta = a[p] ?? "", tb = b[p] ?? ""
            let la = ta.split(separator: "\n", omittingEmptySubsequences: false), lb = tb.split(separator: "\n", omittingEmptySubsequences: false)
            func data(_ s: [SQL.Statement]) -> [(verb: String, table: String, st: SQL.Statement)] {
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
                    return (f.text.uppercased(), t, st)
                }
            }
            let da = data(SQL.statements(ta)).map { (verb: $0.verb, table: renames[$0.table] ?? $0.table, st: $0.st) }, db = data(SQL.statements(tb))
            let textsA = Set(da.map(\.st.text)), textsB = Set(db.map(\.st.text))
            // Pair by table: a statement on the same table that exists on both sides but differs is "changed".
            let goneA = da.filter { !textsB.contains($0.st.text) }, newB = db.filter { !textsA.contains($0.st.text) }
            var newByKey = Dictionary(grouping: newB) { "\($0.verb) \($0.table)" }
            var groups: [String: DBData] = [:]
            for g in goneA {
                let k = "\(g.verb) \(g.table)"
                if var list = newByKey[k], !list.isEmpty {
                    let n = list.removeFirst(); newByKey[k] = list
                    groups["c " + k, default: DBData(verb: g.verb, table: g.table, kind: .changed, path: p, line: n.st.line)].before.append(g.st.source(la))
                    groups["c " + k]!.after.append(n.st.source(lb))
                } else {
                    groups["r " + k, default: DBData(verb: g.verb, table: g.table, kind: .removed, path: p)].before.append(g.st.source(la))
                }
            }
            for (k, list) in newByKey {
                for n in list { groups["a " + k, default: DBData(verb: n.verb, table: n.table, kind: .added, path: p, line: list[0].st.line)].after.append(n.st.source(lb)) }
            }
            let rank: [LensFact.Kind: Int] = [.removed: 0, .added: 1, .changed: 2]
            out += groups.values.sorted { (rank[$0.kind] ?? 9, $0.table, $0.verb) < (rank[$1.kind] ?? 9, $1.table, $1.verb) }
        }
        return out
    }
}

import Foundation

// MARK: - Code: definitions added, removed, changed (tree-sitter)

enum CodeLens {
    static func report(_ changes: [ChangedPath], a: Snapshot, b: Snapshot, in dir: String) async -> LensReport? {
        let code = changes.filter { CodeIndex.canNavigate($0.path) }
        guard !code.isEmpty else { return nil }
        let olds = code.compactMap(\.old), news = code.compactMap(\.new)
        async let ta = a.read(olds, in: dir, maxSize: 1_000_000)
        async let tb = b.read(news, in: dir, maxSize: 1_000_000)
        let (textA, textB) = (await ta, await tb)
        return await Task.detached { () -> LensReport? in
            var sections: [LensSection] = []
            var n = (added: 0, removed: 0, changed: 0)
            for c in code {
                let da = c.old.flatMap { p in textA[p].map { CodeIndex.definitions(in: $0, path: p) } } ?? []
                let db = c.new.flatMap { p in textB[p].map { CodeIndex.definitions(in: $0, path: p) } } ?? []
                // Same name and kind, in order: the second `new` in a file pairs with the second one in the other version.
                func keyed(_ d: [(symbol: CodeSymbol, signature: String, body: String)]) -> [String: (symbol: CodeSymbol, signature: String, body: String)] {
                    var seen: [String: Int] = [:], out: [String: (symbol: CodeSymbol, signature: String, body: String)] = [:]
                    for x in d { let k = "\(x.symbol.kind.rawValue) \(x.symbol.name)"; let i = seen[k, default: 0]; seen[k] = i + 1; out["\(k)#\(i)"] = x }
                    return out
                }
                let ka = keyed(da), kb = keyed(db)
                var facts: [LensFact] = []
                let label = { (s: CodeSymbol) in s.kind == .other ? s.name : "\(s.kind.rawValue) \(s.name)" }
                for (k, x) in kb.sorted(by: { $0.value.symbol.line < $1.value.symbol.line }) {
                    if let o = ka[k] {
                        guard o.body != x.body else { continue }
                        let sig = o.signature != x.signature
                        facts.append(LensFact(kind: .changed, title: label(x.symbol), detail: sig ? "signature changed: \(x.signature)" : "body changed",
                                              path: c.new, line: x.symbol.line))
                        n.changed += 1
                    } else {
                        facts.append(LensFact(kind: .added, title: label(x.symbol), detail: x.signature, path: c.new, line: x.symbol.line)); n.added += 1
                    }
                }
                for (k, x) in ka.sorted(by: { $0.value.symbol.line < $1.value.symbol.line }) where kb[k] == nil {
                    facts.append(LensFact(kind: .removed, title: label(x.symbol), detail: x.signature, path: c.old, line: nil)); n.removed += 1
                }
                if !facts.isEmpty { sections.append(LensSection(title: c.path, facts: facts)) }
            }
            guard !sections.isEmpty else { return nil }
            var s: [String] = []
            if n.added > 0 { s.append("\(n.added) added") }
            if n.removed > 0 { s.append("\(n.removed) removed") }
            if n.changed > 0 { s.append("\(n.changed) changed") }
            return LensReport(title: "Code", icon: "curlybraces", summary: s.joined(separator: " · ") + " definitions in \(sections.count) file\(sections.count == 1 ? "" : "s")", sections: sections)
        }.value
    }
}

// MARK: - Dependencies

enum DependencyLens {
    /// name → version (or requirement), per file kind.
    static func parse(_ path: String, _ text: String) -> [String: String]? {
        let name = (path as NSString).lastPathComponent
        switch name {
        case "Cargo.lock":
            var out: [String: String] = [:], cur: String?
            for l in text.split(separator: "\n") {
                if l.hasPrefix("name = ") { cur = unquote(l.dropFirst(7)) }
                else if l.hasPrefix("version = "), let c = cur { out[out[c] == nil ? c : "\(c) (\(unquote(l.dropFirst(10))))"] = unquote(l.dropFirst(10)); cur = nil }
            }
            return out
        case "Cargo.toml", "pyproject.toml":
            return tomlDeps(text, cargo: name == "Cargo.toml")
        case "package.json":
            guard let j = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any] else { return nil }
            var out: [String: String] = [:]
            for (sec, label) in [("dependencies", ""), ("devDependencies", " (dev)"), ("peerDependencies", " (peer)"), ("optionalDependencies", " (optional)")] {
                for (k, v) in j[sec] as? [String: String] ?? [:] { out[k + label] = v }
            }
            return out
        case "package-lock.json":
            guard let j = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any], let pk = j["packages"] as? [String: Any] else { return nil }
            var out: [String: String] = [:]
            for (k, v) in pk where k.hasPrefix("node_modules/") {
                guard let d = v as? [String: Any], let ver = d["version"] as? String else { continue }
                out[String(k.dropFirst(13)).replacingOccurrences(of: "/node_modules/", with: " › ")] = ver
            }
            return out
        case "go.mod":
            var out: [String: String] = [:], inBlock = false
            for raw in text.split(separator: "\n") {
                let l = raw.split(separator: "//").first.map { $0.trimmingCharacters(in: .whitespaces) } ?? ""
                if l.hasPrefix("require (") { inBlock = true; continue }
                if inBlock && l == ")" { inBlock = false; continue }
                let body = inBlock ? l : l.hasPrefix("require ") ? String(l.dropFirst(8)) : ""
                let f = body.split(separator: " ")
                if f.count >= 2 { out[String(f[0])] = String(f[1]) }
            }
            return out
        case "Package.resolved":
            guard let j = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any] else { return nil }
            let pins = (j["pins"] as? [[String: Any]]) ?? ((j["object"] as? [String: Any])?["pins"] as? [[String: Any]]) ?? []
            var out: [String: String] = [:]
            for p in pins {
                let n = (p["identity"] as? String) ?? (p["package"] as? String) ?? "?"
                let st = p["state"] as? [String: Any]
                out[n] = (st?["version"] as? String) ?? (st?["revision"] as? String).map { String($0.prefix(8)) } ?? "?"
            }
            return out
        case "flake.lock":
            guard let j = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any], let nodes = j["nodes"] as? [String: Any] else { return nil }
            var out: [String: String] = [:]
            for (k, v) in nodes where k != "root" {
                guard let locked = (v as? [String: Any])?["locked"] as? [String: Any] else { continue }
                let rev = (locked["rev"] as? String).map { String($0.prefix(8)) } ?? (locked["narHash"] as? String).map { String($0.prefix(12)) } ?? "?"
                let date = (locked["lastModified"] as? Double).map { Date(timeIntervalSince1970: $0).formatted(date: .abbreviated, time: .omitted) }
                out[k] = date.map { "\(rev) (\($0))" } ?? rev
            }
            return out
        default:
            if name.hasPrefix("requirements") && name.hasSuffix(".txt") {
                var out: [String: String] = [:]
                for raw in text.split(separator: "\n") {
                    let l = raw.split(separator: "#").first.map { $0.trimmingCharacters(in: .whitespaces) } ?? ""
                    guard !l.isEmpty, !l.hasPrefix("-") else { continue }
                    let idx = l.firstIndex { "<>=!~;[ ".contains($0) } ?? l.endIndex
                    out[String(l[..<idx]).lowercased()] = String(l[idx...]).trimmingCharacters(in: .whitespaces)
                }
                return out
            }
            return nil
        }
    }

    private static func unquote<S: StringProtocol>(_ s: S) -> String { s.trimmingCharacters(in: CharacterSet(charactersIn: "\" ")) }

    /// [dependencies]-style tables in Cargo.toml, and PEP 621 / Poetry dependencies in pyproject.toml.
    private static func tomlDeps(_ text: String, cargo: Bool) -> [String: String] {
        var out: [String: String] = [:]
        var section = ""
        var inArray = false
        for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let l = raw.trimmingCharacters(in: .whitespaces)
            if l.hasPrefix("[") { section = l.trimmingCharacters(in: CharacterSet(charactersIn: "[] ")); inArray = false; continue }
            if l.isEmpty || l.hasPrefix("#") { continue }
            if cargo {
                guard section.hasSuffix("dependencies") else { continue }
                let label = section == "dependencies" || section == "workspace.dependencies" ? "" : " (\(section.replacingOccurrences(of: "dependencies", with: "").trimmingCharacters(in: CharacterSet(charactersIn: ".-"))))"
                guard let eq = l.firstIndex(of: "=") else { continue }
                let k = unquote(l[..<eq]), v = l[l.index(after: eq)...].trimmingCharacters(in: .whitespaces)
                if v.hasPrefix("{") {
                    let ver = v.range(of: #"version\s*=\s*"([^"]+)""#, options: .regularExpression).map { unquote(v[$0].split(separator: "=")[1]) }
                    let ws = v.contains("workspace = true") ? "workspace" : nil
                    let path = v.range(of: #"path\s*=\s*"([^"]+)""#, options: .regularExpression).map { "path " + unquote(v[$0].split(separator: "=")[1]) }
                    out[k + label] = ver ?? ws ?? path ?? v
                } else { out[k + label] = unquote(v) }
            } else {
                if section == "project" && l.hasPrefix("dependencies") && l.contains("[") { inArray = true }
                if inArray {
                    for m in l.matches(of: #/"([^"]+)"/#) {
                        let spec = String(m.1)
                        let idx = spec.firstIndex { "<>=!~;[ ".contains($0) } ?? spec.endIndex
                        out[String(spec[..<idx]).lowercased()] = String(spec[idx...]).trimmingCharacters(in: .whitespaces)
                    }
                    if l.contains("]") { inArray = false }
                } else if section.hasPrefix("tool.poetry") && section.hasSuffix("dependencies"), let eq = l.firstIndex(of: "=") {
                    out[unquote(l[..<eq]).lowercased()] = unquote(l[l.index(after: eq)...])
                }
            }
        }
        return out
    }

    /// "1.4.2" → "1.10.0": major / minor / patch, or a downgrade.
    static func bump(_ a: String, _ b: String) -> String? {
        func nums(_ s: String) -> [Int]? {
            let core = s.drop { !$0.isNumber }.prefix { $0.isNumber || $0 == "." }
            let n = core.split(separator: ".").compactMap { Int($0) }
            return n.isEmpty ? nil : n + Array(repeating: 0, count: max(0, 3 - n.count))
        }
        guard let x = nums(a), let y = nums(b) else { return nil }
        if y.lexicographicallyPrecedes(x) { return "downgrade" }
        if y[0] != x[0] { return "major" }
        if y[1] != x[1] { return "minor" }
        if y[2] != x[2] { return "patch" }
        return nil
    }

    static func report(_ changes: [ChangedPath], a: Snapshot, b: Snapshot, in dir: String) async -> LensReport? {
        let deps = changes.filter { parse($0.path, "") != nil || ["Cargo.toml", "pyproject.toml", "go.mod"].contains(($0.path as NSString).lastPathComponent) || ($0.path as NSString).lastPathComponent.hasPrefix("requirements") }
        guard !deps.isEmpty else { return nil }
        async let ta = a.read(deps.compactMap(\.old), in: dir, maxSize: 20_000_000)
        async let tb = b.read(deps.compactMap(\.new), in: dir, maxSize: 20_000_000)
        let (textA, textB) = (await ta, await tb)
        var sections: [LensSection] = []
        var n = (added: 0, removed: 0, changed: 0, major: 0)
        for c in deps {
            let pa = c.old.flatMap { p in textA[p].flatMap { parse(p, $0) } } ?? [:]
            let pb = c.new.flatMap { p in textB[p].flatMap { parse(p, $0) } } ?? [:]
            var facts: [LensFact] = []
            for k in Set(pb.keys).subtracting(pa.keys).sorted() { facts.append(LensFact(kind: .added, title: k, detail: pb[k], path: c.new)); n.added += 1 }
            for k in Set(pa.keys).subtracting(pb.keys).sorted() { facts.append(LensFact(kind: .removed, title: k, detail: pa[k], path: c.old)); n.removed += 1 }
            for k in Set(pa.keys).intersection(pb.keys).sorted() where pa[k] != pb[k] {
                let kind = bump(pa[k]!, pb[k]!)
                if kind == "major" { n.major += 1 }
                facts.append(LensFact(kind: .changed, title: k, detail: "\(pa[k]!) → \(pb[k]!)" + (kind.map { " · \($0)" } ?? ""), path: c.new)); n.changed += 1
            }
            if !facts.isEmpty { sections.append(LensSection(title: c.path, facts: facts)) }
        }
        guard !sections.isEmpty else { return nil }
        var s: [String] = []
        if n.added > 0 { s.append("\(n.added) added") }
        if n.removed > 0 { s.append("\(n.removed) removed") }
        if n.changed > 0 { s.append("\(n.changed) updated" + (n.major > 0 ? " (\(n.major) major)" : "")) }
        return LensReport(title: "Dependencies", icon: "shippingbox", summary: s.joined(separator: " · "), sections: sections)
    }
}

// MARK: - Config: key paths in JSON / TOML / YAML / .env

enum ConfigLens {
    static let handledElsewhere: Set<String> = ["package.json", "package-lock.json", "Cargo.toml", "Cargo.lock", "pyproject.toml", "flake.lock",
                                                "Package.resolved", "composer.lock", "yarn.lock", "pnpm-lock.yaml", "tsconfig.json"]

    static func isConfig(_ p: String) -> Bool {
        let name = (p as NSString).lastPathComponent, ext = (p as NSString).pathExtension.lowercased()
        if handledElsewhere.contains(name) { return false }
        return ["json", "toml", "yaml", "yml"].contains(ext) || name.hasPrefix(".env") || ext == "env"
    }

    /// Judged on the last key component: "design-tokens-parity.steps[3].run" isn't a secret, "with.token" is.
    static func secret(_ key: String) -> Bool {
        let last = key.split(separator: ".").last.map(String.init) ?? key
        let k = last.replacingOccurrences(of: #"\[\d+\]"#, with: "", options: .regularExpression).lowercased()
        return ["secret", "token", "password", "passwd", "apikey", "api_key", "private", "credential", "auth"].contains { k.contains($0) }
    }

    /// Flattens a file into "a.b[0].c" → value.
    static func flatten(_ path: String, _ text: String) -> [String: String] {
        let name = (path as NSString).lastPathComponent, ext = (path as NSString).pathExtension.lowercased()
        var out: [String: String] = [:]
        if ext == "json" {
            guard let j = try? JSONSerialization.jsonObject(with: Data(text.utf8), options: [.fragmentsAllowed]) else { return [:] }
            func walk(_ v: Any, _ k: String) {
                if let d = v as? [String: Any] { for (kk, vv) in d { walk(vv, k.isEmpty ? kk : "\(k).\(kk)") } }
                else if let a = v as? [Any] { for (i, vv) in a.enumerated() { walk(vv, "\(k)[\(i)]") } }
                else { out[k] = "\(v)" }
            }
            walk(j, "")
        } else if ext == "toml" {
            var section = ""
            for raw in text.split(separator: "\n") {
                let l = raw.trimmingCharacters(in: .whitespaces)
                if l.isEmpty || l.hasPrefix("#") { continue }
                if l.hasPrefix("[") { section = l.trimmingCharacters(in: CharacterSet(charactersIn: "[] ")); continue }
                guard let eq = l.firstIndex(of: "=") else { continue }
                let k = l[..<eq].trimmingCharacters(in: .whitespaces)
                out[section.isEmpty ? k : "\(section).\(k)"] = l[l.index(after: eq)...].trimmingCharacters(in: .whitespaces)
            }
        } else if name.hasPrefix(".env") || ext == "env" {
            for raw in text.split(separator: "\n") {
                var l = raw.trimmingCharacters(in: .whitespaces)
                if l.isEmpty || l.hasPrefix("#") { continue }
                if l.hasPrefix("export ") { l.removeFirst(7) }
                guard let eq = l.firstIndex(of: "=") else { continue }
                out[String(l[..<eq])] = String(l[l.index(after: eq)...])
            }
        } else {
            out = yaml(text)
        }
        return out
    }

    /// Mappings and sequences by indentation, enough for key-level diffs of config and CI files. Block scalars are
    /// kept whole as one value. ponytail: not a YAML parser (anchors, flow style and multi-line keys are approximated).
    static func yaml(_ text: String) -> [String: String] {
        var out: [String: String] = [:]
        var stack: [(indent: Int, key: String)] = []
        var seqIndex: [String: Int] = [:]
        var doc = 0
        var block: (key: String, indent: Int, lines: [String])?
        func flushBlock() { if let b = block { out[b.key] = b.lines.joined(separator: "\n") }; block = nil }
        for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(raw)
            let indent = line.prefix { $0 == " " }.count
            let l = line.trimmingCharacters(in: .whitespaces)
            if let b = block {
                if l.isEmpty || indent > b.indent { block?.lines.append(l); continue }
                flushBlock()
            }
            if l.isEmpty || l.hasPrefix("#") { continue }
            if l == "---" { doc += 1; stack = []; continue }
            while let top = stack.last, top.indent >= indent { stack.removeLast() }
            var parent = stack.last?.key ?? (doc > 0 ? "[\(doc)]" : "")
            var body = Substring(l)
            var itemIndent = indent
            if body.hasPrefix("- ") || body == "-" {
                let i = seqIndex[parent, default: -1] + 1
                seqIndex[parent] = i
                parent = "\(parent)[\(i)]"
                body = body.dropFirst(2)
                itemIndent = indent + 2
                stack.append((indent, parent))
                if body.isEmpty { continue }
            }
            guard let colon = body.range(of: ": ") ?? (body.hasSuffix(":") ? body.index(before: body.endIndex)..<body.endIndex : nil) else {
                out[parent] = String(body); continue
            }
            let k = body[..<colon.lowerBound].trimmingCharacters(in: CharacterSet(charactersIn: "\"' "))
            let v = body[colon.upperBound...].trimmingCharacters(in: .whitespaces)
            let full = parent.isEmpty ? k : "\(parent).\(k)"
            if v.isEmpty { stack.append((itemIndent, full)); seqIndex[full] = nil }
            else if v == "|" || v == ">" || v.hasPrefix("|") || v.hasPrefix(">") { block = (full, itemIndent, []) }
            else { out[full] = v.split(separator: " #").first.map(String.init) ?? v }
        }
        flushBlock()
        return out
    }

    static func report(_ changes: [ChangedPath], a: Snapshot, b: Snapshot, in dir: String) async -> LensReport? {
        let cfg = changes.filter { isConfig($0.path) }
        guard !cfg.isEmpty else { return nil }
        async let ta = a.read(cfg.compactMap(\.old), in: dir, maxSize: 1_000_000)
        async let tb = b.read(cfg.compactMap(\.new), in: dir, maxSize: 1_000_000)
        let (textA, textB) = (await ta, await tb)
        var sections: [LensSection] = []
        var total = 0
        for c in cfg {
            let fa = c.old.flatMap { p in textA[p].map { flatten(p, $0) } } ?? [:]
            let fb = c.new.flatMap { p in textB[p].map { flatten(p, $0) } } ?? [:]
            func one(_ v: String) -> String {
                let lines = v.split(separator: "\n", omittingEmptySubsequences: true)
                let first = String((lines.first ?? "").prefix(80))
                return lines.count > 1 ? "\(first) … (\(lines.count) lines)" : first
            }
            func show(_ k: String, _ v: String) -> String { secret(k) ? "(value hidden)" : one(v) }
            /// Keys under a parent that exists only on one side collapse into the parent: a new CI job is one fact.
            func collapse(_ keys: [String], absentFrom other: [String: String]) -> [(key: String, count: Int)] {
                let otherKeys = Array(other.keys)
                func prefixes(_ k: String) -> [String] {
                    var out: [String] = [], cur = ""
                    for part in k.split(separator: ".", omittingEmptySubsequences: false) {
                        cur = cur.isEmpty ? String(part) : "\(cur).\(part)"
                        out.append(cur)
                    }
                    return out
                }
                var groups: [String: Int] = [:], order: [String] = []
                for k in keys {
                    // The shortest prefix that the other side lacks entirely and that holds more than this key.
                    let p = prefixes(k).dropLast().first { pre in
                        !otherKeys.contains { $0 == pre || $0.hasPrefix(pre + ".") || $0.hasPrefix(pre + "[") }
                            && keys.filter { $0.hasPrefix(pre + ".") || $0.hasPrefix(pre + "[") }.count > 1
                    } ?? k
                    if groups[p] == nil { order.append(p) }
                    groups[p, default: 0] += 1
                }
                return order.map { ($0, groups[$0]!) }
            }
            var facts: [LensFact] = []
            // Whole new or deleted files: one line, not every key.
            if c.old == nil { facts.append(LensFact(kind: .added, title: c.path, detail: "new file · \(fb.count) keys", path: c.new)) }
            else if c.new == nil { facts.append(LensFact(kind: .removed, title: c.path, detail: "deleted · had \(fa.count) keys")) }
            else {
                for (k, n) in collapse(Set(fb.keys).subtracting(fa.keys).sorted(), absentFrom: fa) {
                    facts.append(LensFact(kind: .added, title: k, detail: n > 1 ? "added · \(n) keys" : show(k, fb[k] ?? ""), path: c.new))
                }
                for (k, n) in collapse(Set(fa.keys).subtracting(fb.keys).sorted(), absentFrom: fb) {
                    facts.append(LensFact(kind: .removed, title: k, detail: n > 1 ? "removed · \(n) keys" : show(k, fa[k] ?? "")))
                }
                for k in Set(fa.keys).intersection(fb.keys).sorted() where fa[k] != fb[k] {
                    let multi = fa[k]!.contains("\n") || fb[k]!.contains("\n")
                    let d = secret(k) ? "(value hidden)" : multi ? "changed (\(fb[k]!.split(separator: "\n").count) lines)" : "\(one(fa[k]!)) → \(one(fb[k]!))"
                    facts.append(LensFact(kind: .changed, title: k, detail: d, path: c.new))
                }
            }
            total += facts.count
            if !facts.isEmpty { sections.append(LensSection(title: c.path, facts: facts)) }
        }
        guard !sections.isEmpty else { return nil }
        return LensReport(title: "Configuration", icon: "slider.horizontal.3", summary: "\(total) key change\(total == 1 ? "" : "s") in \(sections.count) file\(sections.count == 1 ? "" : "s")", sections: sections)
    }
}

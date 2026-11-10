import Foundation

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
        // By name: an empty text doesn't parse as JSON, so asking `parse` would drop package.json and the other JSON files.
        let names: Set<String> = ["Cargo.lock", "Cargo.toml", "pyproject.toml", "package.json", "package-lock.json", "go.mod", "Package.resolved", "flake.lock"]
        let deps = changes.filter { let n = ($0.path as NSString).lastPathComponent; return names.contains(n) || (n.hasPrefix("requirements") && n.hasSuffix(".txt")) }
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
            for k in Set(pb.keys).subtracting(pa.keys).sorted() { facts.append(LensFact(kind: .added, title: k, detail: pb[k], path: c.new, after: pb[k])); n.added += 1 }
            for k in Set(pa.keys).subtracting(pb.keys).sorted() { facts.append(LensFact(kind: .removed, title: k, detail: pa[k], before: pa[k])); n.removed += 1 }
            for k in Set(pa.keys).intersection(pb.keys).sorted() where pa[k] != pb[k] {
                let kind = bump(pa[k]!, pb[k]!)
                if kind == "major" { n.major += 1 }
                facts.append(LensFact(kind: .changed, title: k, detail: "\(pa[k]!) → \(pb[k]!)" + (kind.map { " · \($0)" } ?? ""), path: c.new, tag: kind, before: pa[k], after: pb[k])); n.changed += 1
            }
            if !facts.isEmpty { sections.append(LensSection(title: c.path, facts: facts)) }
        }
        guard !sections.isEmpty else { return nil }
        var s: [String] = []
        if n.added > 0 { s.append("\(n.added) added") }
        if n.removed > 0 { s.append("\(n.removed) removed") }
        if n.changed > 0 { s.append("\(n.changed) updated" + (n.major > 0 ? " (\(n.major) major)" : "")) }
        return LensReport(kind: .dependencies, title: "Dependencies", icon: "shippingbox", summary: s.joined(separator: " · "), sections: sections,
                          metric: LensMetric(value: n.added + n.removed + n.changed, unit: n.added + n.removed + n.changed == 1 ? "package" : "packages", added: n.added, removed: n.removed, changed: n.changed))
    }
}

// MARK: - Config: key paths in JSON / TOML / YAML / .env

enum ConfigLens {
    static let handledElsewhere: Set<String> = ["package.json", "package-lock.json", "Cargo.toml", "Cargo.lock", "pyproject.toml", "flake.lock",
                                                "Package.resolved", "composer.lock", "yarn.lock", "pnpm-lock.yaml", "tsconfig.json"]

    static func isConfig(_ p: String) -> Bool {
        let name = (p as NSString).lastPathComponent, ext = (p as NSString).pathExtension.lowercased()
        if handledElsewhere.contains(name) || ["Assets", "Tests", "Generated"].contains(FilesLens.category(p)) { return false }
        return ["json", "toml", "yaml", "yml", "properties"].contains(ext) || name.hasPrefix(".env") || ext == "env"
    }

    /// Judged on the last key component: "design-tokens-parity.steps[3].run" isn't a secret, "with.token" is.
    static func secret(_ key: String) -> Bool {
        let last = components(key).last ?? key
        let k = last.replacingOccurrences(of: #"\[\d+\]"#, with: "", options: .regularExpression).lowercased()
        return ["secret", "token", "password", "passwd", "apikey", "api_key", "private", "credential", "auth"].contains { k.contains($0) }
    }

    /// A key path's parts: "rules[apps: deployments].verbs" → ["rules[apps: deployments]", "verbs"]. Dots inside
    /// brackets (an item named "fleetdriver.com: projects") don't split.
    static func components(_ key: String) -> [String] {
        var out: [String] = [], cur = "", depth = 0
        for c in key {
            if c == "[" { depth += 1 } else if c == "]" { depth -= 1 }
            if c == ".", depth == 0 { out.append(cur); cur = ""; continue }
            cur.append(c)
        }
        out.append(cur)
        return out
    }

    /// Flattens a file into "a.b[web].c" → value.
    static func flatten(_ path: String, _ text: String) -> [String: String] {
        let name = (path as NSString).lastPathComponent, ext = (path as NSString).pathExtension.lowercased()
        var out: [String: String] = [:]
        if ext == "json" {
            guard let j = try? JSONSerialization.jsonObject(with: Data(text.utf8), options: [.fragmentsAllowed]) else { return [:] }
            walk(j, "", &out)
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
        } else if name.hasPrefix(".env") || ext == "env" || ext == "properties" {
            for raw in text.split(separator: "\n") {
                var l = raw.trimmingCharacters(in: .whitespaces)
                if l.isEmpty || l.hasPrefix("#") || l.hasPrefix("!") { continue }
                if l.hasPrefix("export ") { l.removeFirst(7) }
                guard let eq = l.firstIndex(of: "=") else { continue }
                out[String(l[..<eq])] = String(l[l.index(after: eq)...])
            }
        } else {
            let docs = YAML.documents(text)
            for (i, d) in docs.enumerated() {
                // Several manifests in one file are told apart by what they are, not where they sit.
                let m = d as? [String: Any]
                let id = [m?["kind"] as? String, (m?["metadata"] as? [String: Any])?["name"] as? String].compactMap { $0 }.joined(separator: " ")
                walk(d, docs.count > 1 ? "[\(id.isEmpty ? String(i) : id)]" : "", &out)
            }
        }
        return out
    }

    /// Lists of plain values stay one value ("[get, create]"). The items of a list of mappings are named by what
    /// identifies them (`containers[web]`, `rules[configmaps]`) when every item has a distinct one, so inserting an
    /// item doesn't make every later one look changed.
    static func walk(_ v: Any, _ k: String, _ out: inout [String: String]) {
        if let d = v as? [String: Any] {
            for (kk, vv) in d { walk(vv, k.isEmpty ? kk : "\(k).\(kk)", &out) }
        } else if let a = v as? [Any] {
            if a.allSatisfy({ !($0 is [String: Any]) && !($0 is [Any]) }) {
                out[k] = "[" + a.map { "\($0)" }.joined(separator: ", ") + "]"
                return
            }
            // A list of [name, options] pairs and bare names (Expo's plugins, Babel's): each under its name.
            let named = a.map { v -> (String, Any?)? in
                if let s = v as? String { return (s, nil) }
                if let p = v as? [Any], let n = p.first as? String, p.count == 1 || (p.count == 2 && p[1] is [String: Any]) { return (n, p.count == 2 ? p[1] : nil) }
                return nil
            }
            if a.contains(where: { $0 is [Any] }), named.allSatisfy({ $0 != nil }), Set(named.map { $0!.0 }).count == a.count {
                for case let (n, v)? in named { if let v { walk(v, "\(k)[\(n)]", &out) } else { out["\(k)[\(n)]"] = "on" } }
                return
            }
            let ids = identities(a)
            for (i, vv) in a.enumerated() { walk(vv, "\(k)[\(ids?[i] ?? String(i))]", &out) }
        } else {
            out[k] = "\(v)"
        }
    }

    static func identities(_ a: [Any]) -> [String]? {
        let ds = a.compactMap { $0 as? [String: Any] }
        guard ds.count == a.count else { return nil }
        func text(_ v: Any?) -> String? { (v as? [Any]).map { $0.map { "\($0)" }.joined(separator: ", ") } ?? (v as? String) }
        let namers: [([String: Any]) -> String?] = [
            { $0["name"] as? String }, { $0["id"] as? String }, { $0["key"] as? String }, { $0["type"] as? String },
            // RBAC rules, by what they grant on: "configmaps", "fleetdriver.com: projects, objectstorages".
            { d in text(d["resources"]).map { r in let g = text(d["apiGroups"]) ?? ""; return g.isEmpty ? r : "\(g): \(r)" } },
        ]
        for name in namers {
            let ids = ds.compactMap(name)
            if ids.count == ds.count, Set(ids).count == ids.count, !ids.contains(where: \.isEmpty) { return ids }
        }
        return nil
    }

    /// Keys under a parent that exists only on one side collapse into the parent: a new CI job is one fact.
    static func collapse(_ keys: [String], absentFrom other: [String: String]) -> [(key: String, count: Int)] {
        let otherKeys = Array(other.keys)
        func prefixes(_ k: String) -> [String] {
            var out: [String] = [], cur = ""
            for part in components(k) {
                cur = cur.isEmpty ? part : "\(cur).\(part)"
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

    /// Key-level facts between two flattened versions of a file.
    static func keyFacts(_ fa: [String: String], _ fb: [String: String], path: String?) -> [LensFact] {
        func one(_ v: String) -> String {
            let lines = v.split(separator: "\n", omittingEmptySubsequences: true)
            let first = String((lines.first ?? "").prefix(80))
            return lines.count > 1 ? "\(first) … (\(lines.count) lines)" : first
        }
        func show(_ k: String, _ v: String) -> String? { secret(k) ? nil : one(v) }
        /// A small new or removed group reads better as its values than as "3 keys".
        func inline(_ k: String, _ f: [String: String]) -> String? {
            // Values the name already says (an RBAC rule's resources) or that say nothing (the core API group's [""]) are left out.
            let kids = f.keys.filter { $0.hasPrefix(k + ".") && !["", "[]"].contains(f[$0]!) && !k.contains("[\(f[$0]!.trimmingCharacters(in: CharacterSet(charactersIn: "[]")))]") }.sorted()
            guard kids.count <= 4, kids.allSatisfy({ !secret($0) && !f[$0]!.contains("\n") }) else { return nil }
            return kids.map { "\($0.dropFirst(k.count + 1)): \(f[$0]!)" }.joined(separator: " · ")
        }
        func list(_ v: String) -> [String]? {
            guard v.hasPrefix("["), v.hasSuffix("]") else { return nil }
            return v.dropFirst().dropLast().split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        }
        var facts: [LensFact] = []
        for (k, n) in collapse(Set(fb.keys).subtracting(fa.keys).sorted(), absentFrom: fa) {
            let small = n > 1 ? inline(k, fb) : nil
            facts.append(LensFact(kind: .added, title: k, detail: n > 1 ? small ?? "added · \(n) keys" : show(k, fb[k] ?? "") ?? "(value hidden)", path: path,
                                  tag: n > 1 && small == nil ? "\(n) keys" : nil, after: n > 1 ? small : show(k, fb[k] ?? ""), count: n))
        }
        for (k, n) in collapse(Set(fa.keys).subtracting(fb.keys).sorted(), absentFrom: fb) {
            let small = n > 1 ? inline(k, fa) : nil
            facts.append(LensFact(kind: .removed, title: k, detail: n > 1 ? small ?? "removed · \(n) keys" : show(k, fa[k] ?? "") ?? "(value hidden)",
                                  tag: n > 1 && small == nil ? "\(n) keys" : nil, before: n > 1 ? small : show(k, fa[k] ?? ""), count: n))
        }
        for k in Set(fa.keys).intersection(fb.keys).sorted() where fa[k] != fb[k] {
            if !secret(k), let la = list(fa[k]!), let lb = list(fb[k]!) {
                // A list of values: what joined and what left, not the whole list twice.
                let d = (lb.filter { !la.contains($0) }.map { "+\($0)" } + la.filter { !lb.contains($0) }.map { "−\($0)" }).joined(separator: "  ")
                facts.append(LensFact(kind: .changed, title: k, detail: d.isEmpty ? "reordered" : d, path: path))
                continue
            }
            let multi = fa[k]!.contains("\n") || fb[k]!.contains("\n")
            let d = secret(k) ? "(value hidden)" : multi ? "changed (\(fb[k]!.split(separator: "\n").count) lines)" : "\(one(fa[k]!)) → \(one(fb[k]!))"
            let plain = !secret(k) && !multi
            facts.append(LensFact(kind: .changed, title: k, detail: d, path: path, before: plain ? one(fa[k]!) : nil, after: plain ? one(fb[k]!) : nil))
        }
        return facts
    }

    // MARK: Custom resource definitions

    /// A CRD as its resource sees it: each schema field with its type ("status.phase" → "Pending | Bound | Lost ·
    /// required") and the columns `kubectl get` prints.
    struct CRD { var kind: String; var fields: [String: String]; var columns: [String] }

    static func crd(_ text: String) -> CRD? {
        // A file of one manifest: beside other manifests, the CRD's name would head changes that aren't its own.
        let docs = YAML.documents(text)
        guard docs.count == 1, let d = docs[0] as? [String: Any], d["kind"] as? String == "CustomResourceDefinition",
              let spec = d["spec"] as? [String: Any] else { return nil }
        let versions = (spec["versions"] as? [Any] ?? []).compactMap { $0 as? [String: Any] }
        var fields: [String: String] = [:], columns: [String] = []
        for v in versions {
            let pre = versions.count > 1 ? "\(v["name"] as? String ?? "?") " : ""
            if let s = (v["schema"] as? [String: Any])?["openAPIV3Schema"] as? [String: Any] { schema(s, pre, required: false, &fields) }
            columns += (v["additionalPrinterColumns"] as? [Any] ?? []).compactMap { (($0 as? [String: Any])?["name"] as? String).map { pre + $0 } }
        }
        return CRD(kind: ((spec["names"] as? [String: Any])?["kind"] as? String) ?? "Resource", fields: fields, columns: columns)
    }

    private static func schema(_ s: [String: Any], _ path: String, required: Bool, _ out: inout [String: String]) {
        func type(_ s: [String: Any]) -> String {
            if let e = s["enum"] as? [Any] { return e.map { "\($0)" }.joined(separator: " | ") }
            if s["x-kubernetes-int-or-string"] as? String == "true" { return "int or string" }
            let t = s["type"] as? String ?? "object"
            return s["format"].map { "\(t) (\($0))" } ?? t
        }
        var node = s, t = type(s)
        if t == "array", let item = s["items"] as? [String: Any] { t = "list of \(type(item))"; node = item }
        let root = path.isEmpty || path.hasSuffix(" ")
        if !root { out[path] = t + (required ? " · required" : "") }
        let req = Set((node["required"] as? [Any] ?? []).map { "\($0)" })
        for (k, v) in node["properties"] as? [String: Any] ?? [:] {
            if let v = v as? [String: Any] { schema(v, root ? path + k : "\(path).\(k)", required: req.contains(k), &out) }
        }
    }

    static func crdFacts(_ a: CRD, _ b: CRD, path: String?) -> [LensFact] {
        /// A new or removed object's own fields, by name: "type, status, reason, message".
        func fields(of k: String, _ f: [String: String]) -> String {
            f.keys.filter { $0.hasPrefix(k + ".") && !$0.dropFirst(k.count + 1).contains(".") }.map { String($0.dropFirst(k.count + 1)) }.sorted().joined(separator: ", ")
        }
        func plain(_ t: String) -> String { t.replacingOccurrences(of: " · required", with: "") }
        var facts: [LensFact] = []
        /// New (or gone) fields whose parent isn't: a new object is one fact, its fields listed in it.
        func tops(_ x: [String: String], _ y: [String: String]) -> [String] {
            let only = Set(x.keys).subtracting(y.keys)
            return only.filter { k in !only.contains { k.hasPrefix($0 + ".") } }.sorted()
        }
        for k in tops(b.fields, a.fields) {
            let kids = fields(of: k, b.fields)
            let t = (b.fields[k] ?? "") + (kids.isEmpty ? "" : " · \(kids)")
            facts.append(LensFact(kind: .added, title: k, detail: t, path: path, after: t))
        }
        for k in tops(a.fields, b.fields) {
            let kids = fields(of: k, a.fields)
            let t = (a.fields[k] ?? "") + (kids.isEmpty ? "" : " · \(kids)")
            facts.append(LensFact(kind: .removed, title: k, detail: t, tag: "breaking", before: t))
        }
        for k in Set(a.fields.keys).intersection(b.fields.keys).sorted() where a.fields[k] != b.fields[k] {
            let (x, y) = (a.fields[k]!, b.fields[k]!)
            // Readers break when a type changes or a field becomes required; a field becoming optional breaks no one.
            let breaking = plain(x) != plain(y) || (y.hasSuffix("required") && !x.hasSuffix("required"))
            facts.append(LensFact(kind: .changed, title: k, detail: "\(x) → \(y)", path: path, tag: breaking ? "breaking" : nil, before: x, after: y))
        }
        if a.columns != b.columns {
            func row(_ c: [String]) -> String { c.isEmpty ? "NAME · AGE" : (["NAME"] + c).joined(separator: " · ") }
            facts.append(LensFact(kind: .changed, title: "kubectl get", detail: "\(row(a.columns)) → \(row(b.columns))", path: path, tag: "columns", before: row(a.columns), after: row(b.columns)))
        }
        return facts
    }

    static func report(_ changes: [ChangedPath], a: Snapshot, b: Snapshot, in dir: String) async -> LensReport? {
        let cfg = changes.filter { isConfig($0.path) }
        guard !cfg.isEmpty else { return nil }
        async let ta = a.read(cfg.compactMap(\.old), in: dir, maxSize: 1_000_000)
        async let tb = b.read(cfg.compactMap(\.new), in: dir, maxSize: 1_000_000)
        let (textA, textB) = (await ta, await tb)
        var sections: [LensSection] = []
        var crds: [(kind: String, path: String, facts: [LensFact])] = []
        for c in cfg {
            let rawA = c.old.flatMap { textA[$0] }, rawB = c.new.flatMap { textB[$0] }
            var fa = c.old.flatMap { p in rawA.map { flatten(p, $0) } } ?? [:]
            var fb = c.new.flatMap { p in rawB.map { flatten(p, $0) } } ?? [:]
            // Whole new or deleted files: one line, not every key.
            if c.old == nil { sections.append(LensSection(title: c.path, facts: [LensFact(kind: .added, title: c.path, detail: "new file · \(fb.count) keys", path: c.new, tag: "new file", count: fb.count)])); continue }
            if c.new == nil { sections.append(LensSection(title: c.path, facts: [LensFact(kind: .removed, title: c.path, detail: "deleted · had \(fa.count) keys", tag: "deleted", count: fa.count)])); continue }
            if let x = rawA.flatMap(crd), let y = rawB.flatMap(crd) {
                // The schema and columns are told as fields; the rest of the file as keys.
                func rest(_ f: [String: String]) -> [String: String] { f.filter { !$0.key.contains(".schema.openAPIV3Schema") && !$0.key.contains(".additionalPrinterColumns") } }
                fa = rest(fa); fb = rest(fb)
                let facts = crdFacts(x, y, path: c.new) + keyFacts(fa, fb, path: c.new)
                if !facts.isEmpty { crds.append((y.kind, c.path, facts)) }
                continue
            }
            let facts = keyFacts(fa, fb, path: c.new)
            if !facts.isEmpty { sections.append(LensSection(title: c.path, facts: facts)) }
        }
        sections.insert(contentsOf: crdSections(crds), at: 0)
        guard !sections.isEmpty else { return nil }
        let all = sections.flatMap(\.facts)
        let total = all.count
        return LensReport(kind: .configuration, title: "Configuration", icon: "slider.horizontal.3", summary: plural(total, "change") + " in " + plural(cfg.count, "file"), sections: sections,
                          metric: LensMetric(value: total, unit: total == 1 ? "config change" : "config changes", added: all.filter { $0.kind == .added }.count,
                                             removed: all.filter { $0.kind == .removed }.count, changed: all.filter { $0.kind == .changed }.count))
    }

    /// One section for what every changed CRD shares (the same field added to three resources is one fact), then
    /// one per resource for the rest.
    static func crdSections(_ crds: [(kind: String, path: String, facts: [LensFact])]) -> [LensSection] {
        func sig(_ f: LensFact) -> String { "\(f.kind)|\(f.title)|\(f.detail ?? "")|\(f.before ?? "")|\(f.after ?? "")|\(f.tag ?? "")" }
        let shared = crds.count > 1 ? Set(crds[0].facts.map(sig)).filter { s in crds.dropFirst().allSatisfy { $0.facts.contains { sig($0) == s } } } : []
        var out: [LensSection] = []
        if !shared.isEmpty {
            out.append(LensSection(title: crds.map(\.kind).sorted().joined(separator: ", ") + " · the same in each", facts: crds[0].facts.filter { shared.contains(sig($0)) }))
        }
        for c in crds.sorted(by: { $0.kind < $1.kind }) {
            let own = c.facts.filter { !shared.contains(sig($0)) }
            if !own.isEmpty { out.append(LensSection(title: "\(c.kind) · \((c.path as NSString).lastPathComponent)", facts: own)) }
        }
        return out
    }
}

// MARK: - Lines a change adds and removes

enum ChangedLines {
    struct Line: Sendable { var path: String; var line: Int; var text: String }
    struct Copy: Sendable { var from: String; var to: String; var similarity: Int }
    struct Result: Sendable { var added: [Line] = []; var removed: [Line] = []; var copies: [Copy] = [] }

    /// The diff from A to B without context, copies found (`-C -C`) so a copied file's lines are only what differs
    /// from its source; and the whole of each new file the diff doesn't cover (untracked, or all of them when A is empty).
    static func read(_ changes: [ChangedPath], a: Snapshot, b: Snapshot, in dir: String) async -> Result {
        let revs: [String]? = switch (a, b) {
        case (.rev(let x), .rev(let y)): [x, y]
        case (.rev(let x), .worktree): [x]
        default: nil
        }
        var r = Result(), seen = Set<String>()
        if let revs, let data = try? await Git.run(["diff", "-U0", "-M", "-C", "-C", "--no-color", "--no-ext-diff"] + revs, in: dir) {
            (r, seen) = await Task.detached {
                var r = Result(), seen = Set<String>()
                for f in DiffParser.parse(data) {
                    seen.insert(f.path)
                    if let from = f.copiedFrom { r.copies.append(Copy(from: from, to: f.path, similarity: f.similarity ?? 100)) }
                    guard !FilesLens.generatedName(f.path.lowercased()) else { continue }
                    for l in f.lines {
                        if l.kind == .added { r.added.append(Line(path: f.path, line: l.newLine ?? 1, text: String(l.text))) }
                        else if l.kind == .removed { r.removed.append(Line(path: f.path, line: l.oldLine ?? 1, text: String(l.text))) }
                    }
                }
                return (r, seen)
            }.value
        }
        let rest = changes.filter { $0.old == nil || revs == nil }.compactMap(\.new)
            .filter { !seen.contains($0) && !FilesLens.generatedName($0.lowercased()) }
        for (p, t) in await b.read(Array(rest.prefix(300)), in: dir) {
            for (i, l) in t.split(separator: "\n", omittingEmptySubsequences: false).enumerated() { r.added.append(Line(path: p, line: i + 1, text: String(l))) }
        }
        return r
    }
}

// MARK: - Follow-ups: what a change leaves for later or switches off

enum FollowUpLens {
    static let kinds: [(title: String, noun: String, pattern: String)] = [
        ("To Do", "to-do", #"\b(TODO|FIXME|XXX|HACK)\b"#),
        ("Skipped and Focused Tests", "skipped test", #"#\[ignore\b|\b(it|test|describe|context)\.(skip|only)\(|\b(xit|xdescribe|xtest)\(|@(Disabled|Ignore)\b|\bt\.Skip(Now|f)?\(|\b(pytest\.mark|unittest)\.skip|\bXCTSkip"#),
        ("Silenced Checks", "silenced check", #"@ts-(ignore|expect-error|nocheck)|eslint-disable|biome-ignore|#!?\[allow\(|\bnoqa\b|type:\s*ignore|\bnolint\b|NOLINT|swiftlint:disable|rubocop:disable|pylint:\s*disable|#pragma warning disable|@SuppressWarnings"#),
    ]

    /// The added lines that match, per kind; `skip` leaves out files not worth it (generated code, docs).
    static func report(_ lines: [ChangedLines.Line], skip: (String) -> Bool) -> LensReport? {
        var skipped: [String: Bool] = [:], sections: [LensSection] = [], counts: [String] = []
        for k in kinds {
            let re = try! NSRegularExpression(pattern: k.pattern)
            var facts: [LensFact] = []
            for l in lines {
                let ns = l.text as NSString
                guard let m = re.firstMatch(in: l.text, range: NSRange(location: 0, length: ns.length)), !inString(l.text, before: m.range.location) else { continue }
                if skipped[l.path] == nil { skipped[l.path] = skip(l.path) }
                guard skipped[l.path] == false else { continue }
                // The line from a little before the match, so a long one still shows it.
                let from = m.range.location > 60 ? m.range.location - 20 : 0
                let text = (from > 0 ? "…" : "") + ns.substring(from: from).trimmingCharacters(in: .whitespaces)
                facts.append(LensFact(kind: .warning, title: text, path: l.path, line: l.line, tag: ns.substring(with: m.range)))
                if facts.count == 200 { break }
            }
            if !facts.isEmpty { sections.append(LensSection(title: k.title, facts: facts)); counts.append(plural(facts.count, k.noun)) }
        }
        guard !sections.isEmpty else { return nil }
        let total = sections.reduce(0) { $0 + $1.facts.count }
        return LensReport(kind: .followUps, title: "Follow-ups", icon: "checklist", summary: counts.joined(separator: " · "), sections: sections,
                          metric: LensMetric(value: total, unit: total == 1 ? "follow-up" : "follow-ups"))
    }

    /// Whether a match at UTF-16 offset `at` sits inside a string literal: a pattern the code looks for, or test data,
    /// isn't a to-do of its own. An apostrophe between letters (a comment's "don't") opens no string.
    static func inString(_ text: String, before at: Int) -> Bool {
        let s = Array((text as NSString).substring(to: at))
        var open: Character?
        for (i, c) in s.enumerated() where c == "\"" || c == "'" || c == "`" {
            if i > 0, s[i - 1] == "\\" { continue }
            if c == "'", i > 0, i + 1 < s.count, s[i - 1].isLetter, s[i + 1].isLetter { continue }
            if open == nil { open = c } else if open == c { open = nil }
        }
        return open != nil
    }
}

// MARK: - Code: what reading the diff file by file hides

/// Not every definition (the diff shows those): only what a reader going file by file would miss. Files copied from
/// elsewhere, an import swapped for another across files (a refactor's story), and native code in an Expo app whose
/// runtimeVersion stayed put.
enum CodeLens {
    static func report(_ d: ChangedLines.Result, changes: [ChangedPath], a: Snapshot, b: Snapshot, in dir: String) async -> LensReport? {
        var sections: [LensSection] = []
        if let w = await nativeWarning(changes, a: a, b: b, in: dir) { sections.append(LensSection(title: "App build", facts: [w])) }
        let copies = copyGroups(d.copies)
        if !copies.isEmpty { sections.append(LensSection(title: "Copied", facts: copies)) }
        let modified = Set(changes.filter { $0.old != nil && $0.new != nil }.compactMap(\.new))
        let added = Set(changes.filter { $0.old == nil }.compactMap(\.new))
        let shifts = importShifts(added: d.added, removed: d.removed, modified: modified, newFiles: added)
        if !shifts.isEmpty { sections.append(LensSection(title: "Imports across files", facts: shifts)) }
        guard !sections.isEmpty else { return nil }
        var summary: [String] = []
        if !copies.isEmpty { summary.append(plural(d.copies.count, "file") + " copied") }
        if !shifts.isEmpty { summary.append(plural(shifts.count, "import change", "import changes") + " across files") }
        if sections.first?.title == "App build" { summary.insert("native code, same runtime", at: 0) }
        let all = sections.flatMap(\.facts)
        return LensReport(kind: .code, title: "Code", icon: "chevron.left.forwardslash.chevron.right", summary: summary.joined(separator: " · "), sections: sections,
                          metric: LensMetric(value: all.count, unit: all.count == 1 ? "finding" : "findings", added: all.filter { $0.kind == .added }.count,
                                             removed: all.filter { $0.kind == .removed }.count, changed: all.filter { $0.kind == .changed }.count))
    }

    /// Copies by where they came from and went: "client/src/ → client/src/classic/ · 26 files".
    static func copyGroups(_ copies: [ChangedLines.Copy]) -> [LensFact] {
        var groups: [String: [ChangedLines.Copy]] = [:], order: [String] = []
        for c in copies {
            // The common tail is the file's own path; what differs before it is where it went.
            let f = c.from.split(separator: "/"), t = c.to.split(separator: "/")
            var n = 0
            while n < min(f.count, t.count), f[f.count - 1 - n] == t[t.count - 1 - n] { n += 1 }
            func dir(_ p: [Substring]) -> String { p.dropLast(n).isEmpty ? "./" : p.dropLast(n).joined(separator: "/") + "/" }
            let key = n == 0 ? "\(c.from) → \(c.to)" : "\(dir(f)) → \(dir(t))"
            if groups[key] == nil { order.append(key) }
            groups[key, default: []].append(c)
        }
        return order.map { k in
            let g = groups[k]!, s = g.map(\.similarity)
            let same = s.min() == s.max() ? "\(s[0])%" : "\(s.min()!)–\(s.max()!)%"
            return LensFact(kind: .info, title: k, detail: g.count == 1 ? "\(same) the same" : "\(g.count) files · \(same) the same", path: g[0].to, count: g.count)
        }
    }

    /// An import line's module and what it brings in, for JavaScript/TypeScript, Python and Rust.
    private static let patterns: [String: Regex<AnyRegexOutput>] = Dictionary(uniqueKeysWithValues: [
        #"^\s*import\s+(?:type\s+)?(.+?)\s+from\s+['"]([^'"]+)['"]"#, #"^\s*\}\s*from\s+['"]([^'"]+)['"]"#, #"^\s*import\s+['"]([^'"]+)['"]"#,
        #"^\s*from\s+(\S+)\s+import\s+(.+)"#, #"^\s*import\s+([\w.]+)"#, #"^\s*(?:pub\s+)?use\s+([\w:]+?)(?:::\{(.*)\})?\s*;"#,
    ].map { ($0, try! Regex($0)) })

    static func importOf(_ text: String, path: String) -> (module: String, names: String)? {
        // Most lines aren't imports: a look at how the line starts before any regex.
        let head = text.drop { $0 == " " || $0 == "\t" }
        guard ["import", "from", "use ", "pub use", "}"].contains(where: { head.hasPrefix($0) }) else { return nil }
        let ext = (path as NSString).pathExtension.lowercased()
        func first(_ pattern: String) -> [String]? {
            guard let m = text.firstMatch(of: patterns[pattern]!) else { return nil }
            return (1..<m.output.count).map { m.output[$0].substring.map(String.init) ?? "" }
        }
        func clean(_ s: String) -> String { s.trimmingCharacters(in: CharacterSet(charactersIn: "{} ")).replacingOccurrences(of: #"\s+"#, with: " ", options: .regularExpression) }
        switch ext {
        case "js", "jsx", "ts", "tsx", "mjs", "cjs", "vue", "svelte":
            if let g = first(#"^\s*import\s+(?:type\s+)?(.+?)\s+from\s+['"]([^'"]+)['"]"#) { return (g[1], clean(g[0])) }
            if let g = first(#"^\s*\}\s*from\s+['"]([^'"]+)['"]"#) ?? first(#"^\s*import\s+['"]([^'"]+)['"]"#) { return (g[0], "") }
        case "py":
            if let g = first(#"^\s*from\s+(\S+)\s+import\s+(.+)"#) { return (g[0], clean(g[1])) }
            if let g = first(#"^\s*import\s+([\w.]+)"#) { return (g[0], "") }
        case "rs":
            if let g = first(#"^\s*(?:pub\s+)?use\s+([\w:]+?)(?:::\{(.*)\})?\s*;"#) { return (g[0], clean(g[1])) }
        default: break
        }
        return nil
    }

    /// Imports that came or went in several files the change edits: one swapped for another ("Button → PillButton ·
    /// 5 files"), or a module now used by several. A module whose import only lists other names isn't a change, and
    /// new files are left out: each imports its usual libraries.
    static func importShifts(added: [ChangedLines.Line], removed: [ChangedLines.Line], modified: Set<String>, newFiles: Set<String>) -> [LensFact] {
        func collect(_ lines: [ChangedLines.Line]) -> [String: (files: Set<String>, names: String)] {
            var out: [String: (files: Set<String>, names: String)] = [:]
            for l in lines where modified.contains(l.path) {
                guard let i = importOf(l.text, path: l.path) else { continue }
                out[i.module, default: ([], i.names)].files.insert(l.path)
                if out[i.module]!.names.isEmpty { out[i.module]!.names = i.names }
            }
            return out
        }
        var plus = collect(added), minus = collect(removed)
        for m in Set(plus.keys).intersection(minus.keys) {
            let both = plus[m]!.files.intersection(minus[m]!.files)
            plus[m]!.files.subtract(both); minus[m]!.files.subtract(both)
        }
        func label(_ m: String, _ names: String) -> String {
            names.isEmpty || names.count > 40 ? (m.components(separatedBy: "::").last ?? m).split(separator: "/").last.map(String.init) ?? m : names
        }
        // A module that's a file this change adds counts from two importers; any other from three.
        func isNew(_ m: String) -> Bool {
            let stem = m.split(separator: "/").filter { $0 != "." && $0 != ".." && $0 != "~" && $0 != "@" }.joined(separator: "/")
            return !stem.isEmpty && newFiles.contains { ($0 as NSString).deletingPathExtension.hasSuffix(stem) || $0.hasSuffix(stem) }
        }
        var facts: [LensFact] = [], used = Set<String>()
        // Swaps: a removed and an added module in the same files, best overlaps first.
        var pairs: [(r: String, a: String, files: Set<String>)] = []
        for (r, rv) in minus where rv.files.count >= 2 {
            for (a, av) in plus where av.files.count >= 2 {
                let shared = rv.files.intersection(av.files)
                if shared.count >= 2 { pairs.append((r, a, shared)) }
            }
        }
        for p in pairs.sorted(by: { $0.files.count > $1.files.count }) where !used.contains(p.r) && !used.contains(p.a) {
            used.insert(p.r); used.insert(p.a)
            facts.append(LensFact(kind: .changed, title: "\(label(p.r, minus[p.r]!.names)) → \(label(p.a, plus[p.a]!.names))",
                                  detail: "\(plural(p.files.count, "file")) · \(p.a)", path: p.files.sorted().first, count: p.files.count))
        }
        for (m, v) in plus.sorted(by: { $0.value.files.count > $1.value.files.count }) where !used.contains(m) && v.files.count >= (isNew(m) ? 2 : 3) {
            facts.append(LensFact(kind: .added, title: label(m, v.names), detail: "now imported in \(plural(v.files.count, "file")) · \(m)", path: v.files.sorted().first, tag: isNew(m) ? "new" : nil, count: v.files.count))
        }
        for (m, v) in minus.sorted(by: { $0.value.files.count > $1.value.files.count }) where !used.contains(m) && v.files.count >= 3 {
            facts.append(LensFact(kind: .removed, title: label(m, v.names), detail: "no longer imported in \(plural(v.files.count, "file")) · \(m)", count: v.files.count))
        }
        return facts
    }

    /// An Expo app with OTA updates: native code that changed while runtimeVersion stayed the same reaches, through an
    /// update, installs built without it. A runtimeVersion policy (appVersion, fingerprint) moves by itself and isn't
    /// checked. ponytail: reads the root app.json only, not app.config.js or apps in subfolders.
    static func nativeWarning(_ changes: [ChangedPath], a: Snapshot, b: Snapshot, in dir: String) async -> LensFact? {
        let native = changes.compactMap(\.new).filter { p in
            ["swift", "m", "mm", "h", "kt", "java", "cpp", "cc", "c"].contains((p as NSString).pathExtension.lowercased()) || (p as NSString).lastPathComponent == "Podfile.lock"
        }
        guard !native.isEmpty else { return nil }
        async let tb = b.read(["package.json", "app.json"], in: dir)
        async let ta = a.read(["app.json"], in: dir)
        let (nb, na) = (await tb, await ta)
        guard nb["package.json"]?.contains("\"expo-updates\"") == true else { return nil }
        func runtime(_ t: String?) -> String? {
            let j = t.flatMap { try? JSONSerialization.jsonObject(with: Data($0.utf8)) as? [String: Any] }
            return (j?["expo"] as? [String: Any])?["runtimeVersion"] as? String
        }
        guard let rb = runtime(nb["app.json"]), rb == runtime(na["app.json"]) else { return nil }
        let names = native.prefix(3).map { ($0 as NSString).lastPathComponent }.joined(separator: ", ") + (native.count > 3 ? "…" : "")
        return LensFact(kind: .warning, title: "Native code changed; runtimeVersion is still \(rb)",
                        detail: "An OTA update with this JavaScript would reach installs built without it. \(plural(native.count, "native file")): \(names)",
                        path: native.first, count: native.count)
    }
}

// MARK: - Branch: the change's base against the branch it goes into

struct BranchStatus: Sendable {
    struct Commit: Sendable, Identifiable { var id: String; var subject: String; var author: String; var date: String; var files: [String] }
    var target: String
    var behind: Int
    var touching: [Commit]      // commits on the target since the change branched off that change the same files
    var conflicts: [String]?    // nil when git can't tell (before 2.38); empty: it merges cleanly
}

enum BranchLens {
    /// nil when `head` already has everything on `upstream`.
    static func status(head: String, upstream: String, target: String, paths: [String], in dir: String) async -> BranchStatus? {
        guard let n = Int((try? await Git.string(["rev-list", "--count", "\(head)..\(upstream)"], in: dir))?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""), n > 0
        else { return nil }
        async let log = Git.string(["--literal-pathspecs", "log", "--format=%x1e%h%x1f%an%x1f%ar%x1f%s", "--name-only", "\(head)..\(upstream)", "--"] + paths.prefix(500), in: dir)
        async let merge = Git.run(["merge-tree", "--write-tree", "--name-only", "--no-messages", upstream, head], in: dir, allowFailure: true)
        let touching = ((try? await log) ?? "").split(separator: "\u{1e}").compactMap { rec -> BranchStatus.Commit? in
            let lines = rec.split(separator: "\n")
            guard let f = lines.first?.split(separator: "\u{1f}", omittingEmptySubsequences: false), f.count == 4 else { return nil }
            return BranchStatus.Commit(id: String(f[0]), subject: String(f[3]), author: String(f[1]), date: String(f[2]), files: lines.dropFirst().map(String.init))
        }
        // merge-tree prints the merged tree, then the conflicted paths; an older git prints its usage instead.
        let out = String(decoding: (try? await merge) ?? Data(), as: UTF8.self).split(separator: "\n").map(String.init)
        let conflicts = out.first.map { $0.count >= 40 && $0.allSatisfy(\.isHexDigit) } == true ? Array(out.dropFirst()) : nil
        return BranchStatus(target: target, behind: n, touching: touching, conflicts: conflicts)
    }
}

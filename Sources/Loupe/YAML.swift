import Foundation

/// Block-style YAML as JSON-like values ([String: Any], [Any], String), one per document.
/// ponytail: no anchors, aliases, tags or multi-line flow collections; enough for config, CI, Kubernetes and Helm
/// files (Helm's `{{ … }}` lines are skipped like comments).
enum YAML {
    static func documents(_ text: String) -> [Any] {
        var docs: [[String]] = [[]]
        for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let l = String(raw)
            if l == "---" || l.hasPrefix("--- ") || l == "..." { docs.append([]); continue }
            docs[docs.count - 1].append(l)
        }
        return docs.compactMap { var p = Parser(lines: $0); return p.document() }
    }

    private struct Parser {
        var lines: [String]
        var i = 0

        static func indent(_ l: String) -> Int { l.prefix { $0 == " " }.count }
        static func skipped(_ l: String) -> Bool {
            let t = l.trimmingCharacters(in: .whitespaces)
            return t.isEmpty || t.hasPrefix("#") || t.hasPrefix("{{")
        }
        static func isItem(_ t: Substring) -> Bool { t == "-" || t.hasPrefix("- ") }

        /// The next line with content: its indent and its text after the indent.
        mutating func peek() -> (indent: Int, text: Substring)? {
            while i < lines.count, Self.skipped(lines[i]) { i += 1 }
            guard i < lines.count else { return nil }
            let n = Self.indent(lines[i])
            return (n, Substring(lines[i]).dropFirst(n))
        }

        mutating func document() -> Any? { peek().flatMap { node($0.indent) } }

        mutating func node(_ indent: Int) -> Any? {
            guard let p = peek(), p.indent >= indent else { return nil }
            if Self.isItem(p.text) { return sequence(p.indent) }
            if Self.entry(p.text) != nil { return mapping(p.indent) }
            i += 1
            return Self.scalar(String(p.text))
        }

        mutating func sequence(_ indent: Int) -> [Any] {
            var out: [Any] = []
            while let p = peek(), p.indent == indent, Self.isItem(p.text) {
                let rest = p.text.dropFirst().drop { $0 == " " }
                if rest.isEmpty || rest.hasPrefix("#") {
                    i += 1
                    out.append(node(indent + 1) ?? "")
                } else {
                    // "- key: v" opens a mapping (and "- - x" a sequence) as deep as the text after the dash: the
                    // item's later keys line up with its first.
                    let n = indent + p.text.count - rest.count
                    lines[i] = String(repeating: " ", count: n) + rest
                    out.append(node(n) ?? "")
                }
            }
            return out
        }

        mutating func mapping(_ indent: Int) -> [String: Any] {
            var out: [String: Any] = [:]
            while let p = peek(), p.indent == indent, !Self.isItem(p.text), let (k, v) = Self.entry(p.text) {
                i += 1
                if v.isEmpty || v.hasPrefix("#") {
                    // The value is on the lines below: deeper, or a sequence at the key's own indent ("key:\n- a").
                    if let n = peek(), n.indent > indent || (n.indent == indent && Self.isItem(n.text)) { out[k] = node(n.indent) ?? "" }
                    else { out[k] = "" }
                } else if v.hasPrefix("|") || v.hasPrefix(">") {
                    out[k] = block(after: indent, folded: v.hasPrefix(">"))
                } else {
                    out[k] = Self.scalar(v)
                }
            }
            return out
        }

        /// A `|` or `>` scalar: the lines deeper than its key, dedented.
        mutating func block(after indent: Int, folded: Bool) -> String {
            var body: [String] = []
            while i < lines.count {
                let l = lines[i]
                if !l.trimmingCharacters(in: .whitespaces).isEmpty && Self.indent(l) <= indent { break }
                body.append(l); i += 1
            }
            while body.last?.trimmingCharacters(in: .whitespaces).isEmpty == true { body.removeLast() }
            let cut = body.filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }.map(Self.indent).min() ?? 0
            return body.map { String($0.dropFirst(min(cut, Self.indent($0)))) }.joined(separator: folded ? " " : "\n")
        }

        /// "key: value" → (key, value), the colon outside quotes and followed by a space or the end; nil otherwise.
        static func entry(_ t: Substring) -> (String, String)? {
            guard let f = t.first, f != "[", f != "{" else { return nil }
            var quote: Character? = f == "\"" || f == "'" ? f : nil
            var j = quote == nil ? t.startIndex : t.index(after: t.startIndex)
            while j < t.endIndex {
                let c = t[j], next = t.index(after: j)
                if let q = quote { if c == q { quote = nil } }
                else if c == ":", next == t.endIndex || t[next] == " " {
                    return (unquote(t[..<j].trimmingCharacters(in: .whitespaces)), t[next...].trimmingCharacters(in: .whitespaces))
                } else if c == " ", next < t.endIndex, t[next] == "#" { return nil }
                j = next
            }
            return nil
        }

        static func unquote(_ s: String) -> String {
            guard s.count >= 2, let f = s.first, f == "\"" || f == "'", s.last == f else { return s }
            return String(s.dropFirst().dropLast())
        }

        static func scalar(_ raw: String) -> Any {
            var v = raw.trimmingCharacters(in: .whitespaces)
            if let f = v.first, f == "\"" || f == "'" {
                let body = v.dropFirst()
                return body.firstIndex(of: f).map { String(body[..<$0]) } ?? v
            }
            if let r = v.range(of: " #") { v = v[..<r.lowerBound].trimmingCharacters(in: .whitespaces) }
            if v.hasPrefix("["), v.hasSuffix("]") { return items(v.dropFirst().dropLast()).map(scalar) }
            if v.hasPrefix("{"), v.hasSuffix("}") {
                var d: [String: Any] = [:]
                for e in items(v.dropFirst().dropLast()) { if let (k, x) = entry(Substring(e)) { d[k] = scalar(x) } }
                return d
            }
            return v
        }

        /// A flow collection's entries, split on the commas outside quotes and brackets.
        static func items(_ s: Substring) -> [String] {
            var out: [String] = [], cur = "", depth = 0
            var quote: Character?
            for c in s {
                if let q = quote { if c == q { quote = nil } }
                else if c == "\"" || c == "'" { quote = c }
                else if c == "[" || c == "{" { depth += 1 }
                else if c == "]" || c == "}" { depth -= 1 }
                else if c == ",", depth == 0 { out.append(cur); cur = ""; continue }
                cur.append(c)
            }
            if !cur.trimmingCharacters(in: .whitespaces).isEmpty { out.append(cur) }
            return out.map { $0.trimmingCharacters(in: .whitespaces) }
        }
    }
}

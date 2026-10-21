import Foundation

struct DiffLine: Hashable {
    enum Kind: UInt8 { case context, added, removed, hunk, meta }
    var kind: Kind
    var text: Substring          // without the leading +/-/space
    var oldLine: Int?
    var newLine: Int?
    var hunkIndex: Int
}

struct Hunk: Hashable {
    var header: String           // "@@ -a,b +c,d @@ ..."
    var lines: Range<Int>        // indices into FileDiff.lines (excluding the header row)
}

struct FileDiff: Identifiable, Hashable {
    var uid = UUID()                 // cheap identity: equality/hash never walks `lines`
    var oldPath: String?
    var newPath: String?
    var headerLines: [String] = []   // "diff --git", "index", "---", "+++" ... needed to rebuild patches
    var lines: [DiffLine] = []       // flat, hunk header rows included — this is what the table renders
    var hunks: [Hunk] = []
    var isBinary = false
    var additions = 0
    var deletions = 0

    var path: String { newPath ?? oldPath ?? "?" }
    var id: String { path }
    static func == (a: FileDiff, b: FileDiff) -> Bool { a.uid == b.uid }
    func hash(into h: inout Hasher) { h.combine(uid) }

    /// A standalone patch containing only hunk `i`, suitable for `git apply`.
    func patch(forHunk i: Int) -> String { patch(selecting: Set(hunks[i].lines), reverse: false) ?? "" }

    /// A patch with only the `selected` changed lines (indices into `lines`), the way `git add -p` edits lines.
    /// An unselected change becomes context on the side the patch is applied to and disappears from the other:
    /// forward (stage) applies to the old side, `reverse` (discard, unstage) to the new side, which must match disk.
    /// nil when no selected line is a change.
    func patch(selecting selected: Set<Int>, reverse: Bool) -> String? {
        var out = headerLines.joined(separator: "\n") + "\n"
        var any = false
        for h in hunks where h.lines.contains(where: { selected.contains($0) && (lines[$0].kind == .added || lines[$0].kind == .removed) }) {
            var body = "", oldCount = 0, newCount = 0, keptPrevious = true
            for i in h.lines {
                let l = lines[i]
                let mark: Character?
                switch l.kind {
                case .context: mark = " "
                case .added: mark = selected.contains(i) ? "+" : reverse ? " " : nil
                case .removed: mark = selected.contains(i) ? "-" : reverse ? nil : " "
                case .meta: if keptPrevious { body += l.text + "\n" }; continue   // "\ No newline" belongs to the line before
                case .hunk: continue
                }
                keptPrevious = mark != nil
                guard let mark else { continue }
                body += String(mark) + l.text + "\n"
                if mark != "+" { oldCount += 1 }
                if mark != "-" { newCount += 1 }
            }
            let (o, n) = DiffParser.parseHunkHeader(Substring(h.header))
            // Keep the function context git prints after the second "@@".
            let tail = h.header.dropFirst(2).range(of: "@@").map { String(h.header[$0.upperBound...]) } ?? ""
            out += "@@ -\(o),\(oldCount) +\(n),\(newCount) @@\(tail)\n" + body
            any = true
        }
        return any ? out : nil
    }
}

enum DiffParser {
    /// Parses `git diff` / Forgejo `.diff` output. Single pass, substrings into one backing String,
    /// so a 50 MB PR diff parses without per-line allocations.
    static func parse(_ data: Data) -> [FileDiff] {
        parse(String(decoding: data, as: UTF8.self))
    }

    static func parse(_ text: String) -> [FileDiff] {
        var files: [FileDiff] = []
        var cur: FileDiff?
        var oldNo = 0, newNo = 0
        var inHunk = false
        var hunkStart = 0

        func closeHunk() {
            guard inHunk, var c = cur else { return }
            c.hunks[c.hunks.count - 1].lines = hunkStart..<c.lines.count
            cur = c
            inHunk = false
        }
        func closeFile() {
            closeHunk()
            if let c = cur { files.append(c) }
            cur = nil
        }

        text.enumerateSubstringLines { line in
            if line.hasPrefix("diff --git ") || line.hasPrefix("diff --cc ") {
                closeFile()
                var f = FileDiff()
                f.headerLines = [String(line)]
                // Fallback paths from "diff --git a/x b/x" — overwritten by ---/+++ when present.
                if let r = line.range(of: " b/", options: .backwards) {
                    f.newPath = String(line[r.upperBound...])
                    f.oldPath = f.newPath
                }
                cur = f
                return
            }
            if cur == nil { cur = FileDiff() }   // diff without "diff --git" (e.g. plain unified)
            if inHunk, let first = line.first, first == " " || first == "+" || first == "-" || first == "\\" {
                let body = line.dropFirst()
                switch first {
                case "+":
                    cur!.lines.append(DiffLine(kind: .added, text: body, oldLine: nil, newLine: newNo, hunkIndex: cur!.hunks.count - 1))
                    newNo += 1; cur!.additions += 1
                case "-":
                    cur!.lines.append(DiffLine(kind: .removed, text: body, oldLine: oldNo, newLine: nil, hunkIndex: cur!.hunks.count - 1))
                    oldNo += 1; cur!.deletions += 1
                case " ":
                    cur!.lines.append(DiffLine(kind: .context, text: body, oldLine: oldNo, newLine: newNo, hunkIndex: cur!.hunks.count - 1))
                    oldNo += 1; newNo += 1
                default:
                    cur!.lines.append(DiffLine(kind: .meta, text: line, oldLine: nil, newLine: nil, hunkIndex: cur!.hunks.count - 1))
                }
                return
            }
            if line.hasPrefix("@@") {
                closeHunk()
                let (o, n) = parseHunkHeader(line)
                oldNo = o; newNo = n
                cur!.hunks.append(Hunk(header: String(line), lines: 0..<0))
                cur!.lines.append(DiffLine(kind: .hunk, text: line, oldLine: nil, newLine: nil, hunkIndex: cur!.hunks.count - 1))
                hunkStart = cur!.lines.count
                inHunk = true
                return
            }
            closeHunk()
            cur!.headerLines.append(String(line))
            if line.hasPrefix("--- ") { cur!.oldPath = stripPrefix(line.dropFirst(4)) }
            else if line.hasPrefix("+++ ") { cur!.newPath = stripPrefix(line.dropFirst(4)) }
            else if line.hasPrefix("rename from ") { cur!.oldPath = String(line.dropFirst(12)) }
            else if line.hasPrefix("rename to ") { cur!.newPath = String(line.dropFirst(10)) }
            else if line.hasPrefix("Binary files ") || line.hasPrefix("GIT binary patch") { cur!.isBinary = true }
        }
        closeFile()
        return files.filter { !$0.headerLines.isEmpty || !$0.lines.isEmpty }
    }

    private static func stripPrefix(_ s: Substring) -> String? {
        var s = s
        if let tab = s.firstIndex(of: "\t") { s = s[..<tab] }   // "--- a/file\t2024-..." from --no-index
        if s == "/dev/null" { return nil }
        if s.hasPrefix("a/") || s.hasPrefix("b/") { return String(s.dropFirst(2)) }
        return String(s)
    }

    /// "@@ -12,7 +12,9 @@" → (12, 12)
    static func parseHunkHeader(_ line: Substring) -> (Int, Int) {
        var old = 0, new = 0
        for tok in line.split(separator: " ").prefix(3) {
            if tok.hasPrefix("-") { old = Int(tok.dropFirst().split(separator: ",").first ?? "") ?? 0 }
            if tok.hasPrefix("+") { new = Int(tok.dropFirst().split(separator: ",").first ?? "") ?? 0 }
        }
        return (old, new)
    }
}

extension String {
    /// Line iteration that yields Substrings (no copies) and handles a trailing newline.
    func enumerateSubstringLines(_ body: (Substring) -> Void) {
        var start = startIndex
        let utf8 = self.utf8
        var i = utf8.startIndex
        while i != utf8.endIndex {
            if utf8[i] == UInt8(ascii: "\n") {
                body(self[start..<i])
                start = utf8.index(after: i)
            }
            i = utf8.index(after: i)
        }
        if start != endIndex { body(self[start...]) }
    }
}

import Foundation

/// A three-way merge by lines, as diff3 does it: base↔ours and base↔theirs are diffed, the lines all three agree on
/// split the files into chunks, and each chunk is either unchanged, changed on one side (taken as is), changed alike on
/// both, or a conflict. Conflicts are trimmed of the lines both sides added alike at their start and end, like git's
/// "zealous" merge, so they match the conflicts git writes into the file.
struct Merge3: Sendable {
    enum Kind: Sendable { case same, ours, theirs, both, conflict }
    struct Chunk: Equatable, Sendable {
        var kind: Kind
        var base: Range<Int>, ours: Range<Int>, theirs: Range<Int>   // line ranges in each version
    }

    let base: [Substring], ours: [Substring], theirs: [Substring]
    let chunks: [Chunk]
    /// Neither side ends in a newline: the merged file shouldn't either.
    let noFinalNewline: Bool

    var conflictCount: Int { chunks.reduce(0) { $0 + ($1.kind == .conflict ? 1 : 0) } }

    init(base: String, ours: String, theirs: String) {
        self.base = Self.lines(base); self.ours = Self.lines(ours); self.theirs = Self.lines(theirs)
        noFinalNewline = !ours.isEmpty && !theirs.isEmpty && !ours.hasSuffix("\n") && !theirs.hasSuffix("\n")
        let mo = Self.matches(self.base, self.ours), mt = Self.matches(self.base, self.theirs)
        var out: [Chunk] = []
        var i = 0, o = 0, t = 0, stable = 0   // stable: lines all three agree on, not yet emitted
        func flushStable() {
            guard stable > 0 else { return }
            out.append(Chunk(kind: .same, base: i - stable..<i, ours: o - stable..<o, theirs: t - stable..<t))
            stable = 0
        }
        while i < self.base.count || o < self.ours.count || t < self.theirs.count {
            if i < self.base.count, mo[i] == o, mt[i] == t { i += 1; o += 1; t += 1; stable += 1; continue }
            flushStable()
            // The next base line both sides kept; the matching is monotonic, so it lies at or after o and t.
            var j = i
            while j < self.base.count, mo[j] == nil || mt[j] == nil { j += 1 }
            let (oj, tj) = j < self.base.count ? (mo[j]!, mt[j]!) : (self.ours.count, self.theirs.count)
            out += Self.classify(self.base, self.ours, self.theirs, i..<j, o..<oj, t..<tj)
            i = j; o = oj; t = tj
        }
        flushStable()
        chunks = out
    }

    private static func classify(_ base: [Substring], _ ours: [Substring], _ theirs: [Substring], _ b: Range<Int>, _ o: Range<Int>, _ t: Range<Int>) -> [Chunk] {
        let bl = base[b], ol = ours[o], tl = theirs[t]
        if ol.elementsEqual(bl) { return [Chunk(kind: tl.elementsEqual(bl) ? .same : .theirs, base: b, ours: o, theirs: t)] }
        if tl.elementsEqual(bl) { return [Chunk(kind: .ours, base: b, ours: o, theirs: t)] }
        if ol.elementsEqual(tl) { return [Chunk(kind: .both, base: b, ours: o, theirs: t)] }
        var pre = 0
        while pre < o.count, pre < t.count, ours[o.lowerBound + pre] == theirs[t.lowerBound + pre] { pre += 1 }
        var suf = 0
        while suf < o.count - pre, suf < t.count - pre, ours[o.upperBound - 1 - suf] == theirs[t.upperBound - 1 - suf] { suf += 1 }
        var r: [Chunk] = []
        if pre > 0 { r.append(Chunk(kind: .both, base: b.lowerBound..<b.lowerBound, ours: o.lowerBound..<o.lowerBound + pre, theirs: t.lowerBound..<t.lowerBound + pre)) }
        r.append(Chunk(kind: .conflict, base: b, ours: o.lowerBound + pre..<o.upperBound - suf, theirs: t.lowerBound + pre..<t.upperBound - suf))
        if suf > 0 { r.append(Chunk(kind: .both, base: b.upperBound..<b.upperBound, ours: o.upperBound - suf..<o.upperBound, theirs: t.upperBound - suf..<t.upperBound)) }
        return r
    }

    /// The merged lines of a chunk that isn't a conflict.
    func merged(_ c: Chunk) -> ArraySlice<Substring> {
        switch c.kind {
        case .same, .ours, .both, .conflict: ours[c.ours]
        case .theirs: theirs[c.theirs]
        }
    }

    /// A file's lines; a final newline doesn't start another line.
    static func lines(_ s: String) -> [Substring] {
        var l = split(s)
        if l.last == "" { l.removeLast() }
        return l
    }

    /// Splits at every "\n", keeping a CRLF line's "\r". Splitting the String itself wouldn't: "\r\n" is one Character,
    /// so a CRLF file would be a single line.
    static func split(_ s: String) -> [Substring] {
        s.unicodeScalars.split(separator: "\n", omittingEmptySubsequences: false).map(Substring.init)
    }

    /// For each line of `a`, the line of `b` it's kept as, or nil when the change removed it.
    static func matches(_ a: [Substring], _ b: [Substring]) -> [Int?] {
        var m = [Int?](repeating: nil, count: a.count)
        var p = 0
        while p < a.count, p < b.count, a[p] == b[p] { m[p] = p; p += 1 }
        var s = 0
        while s < a.count - p, s < b.count - p, a[a.count - 1 - s] == b[b.count - 1 - s] { m[a.count - 1 - s] = b.count - 1 - s; s += 1 }
        // Lines as ints: the diff compares them many times.
        var ids: [Substring: Int] = [:]
        func id(_ l: Substring) -> Int { if let i = ids[l] { return i }; ids[l] = ids.count; return ids.count - 1 }
        let x = a[p..<a.count - s].map(id), y = b[p..<b.count - s].map(id)
        guard !x.isEmpty, !y.isEmpty else { return m }
        // ponytail: stdlib Myers, O((N+M)·D); a whole-file rewrite of a huge file is slow. Patience/histogram diff if that bites.
        var removed = [Bool](repeating: false, count: x.count), inserted = [Bool](repeating: false, count: y.count)
        for c in y.difference(from: x) {
            switch c {
            case .remove(let off, _, _): removed[off] = true
            case .insert(let off, _, _): inserted[off] = true
            }
        }
        var i = 0, j = 0
        while i < x.count, j < y.count {
            if removed[i] { i += 1 } else if inserted[j] { j += 1 } else { m[p + i] = p + j; i += 1; j += 1 }
        }
        return m
    }
}

// MARK: - The index stages of a conflicted path

/// What `git ls-files -u` says about one path: which of base (1), ours (2) and theirs (3) exist, their modes and blobs.
struct ConflictStages: Equatable {
    var entries: [Int: (mode: String, sha: String)] = [:]

    static func == (a: Self, b: Self) -> Bool { a.entries.mapValues { "\($0.mode) \($0.sha)" } == b.entries.mapValues { "\($0.mode) \($0.sha)" } }

    /// Parses `git ls-files -u -z -- path`: `<mode> <sha> <stage>\t<path>\0` per stage.
    init(_ out: String) {
        for rec in out.split(separator: "\0") {
            let (meta, _) = rec.splitOnce("\t")
            let f = meta.split(separator: " ")
            if f.count == 3, let st = Int(f[2]) { entries[st] = (String(f[0]), String(f[1])) }
        }
    }

    enum Kind: Equatable { case text, bothAdded, deletedByUs, deletedByThem, addedByUs, addedByThem, submodule, none }

    var kind: Kind {
        let has = { self.entries[$0] != nil }
        if entries.values.contains(where: { $0.mode == "160000" }) { return .submodule }
        switch (has(1), has(2), has(3)) {
        case (true, true, true): return .text
        case (false, true, true): return .bothAdded
        case (true, false, true): return .deletedByUs
        case (true, true, false): return .deletedByThem
        case (false, true, false): return .addedByUs
        case (false, false, true): return .addedByThem
        default: return .none
        }
    }
}

extension Data {
    /// Git's own test: a NUL in the first 8000 bytes.
    var looksBinary: Bool { prefix(8000).contains(0) }
}

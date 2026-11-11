import SwiftUI
import AppKit

// MARK: - What Ours and Theirs mean right now

/// Git's "ours" and "theirs" swap meaning between a merge and a rebase; the editor names them for what they are.
struct MergeLabels: Equatable {
    var ours: String, oursDetail: String
    var theirs: String, theirsDetail: String

    init(operation: GitOperation?, branch: String?, subject: String) {
        let head = branch ?? "HEAD"
        switch operation {
        case .merge: (ours, oursDetail, theirs, theirsDetail) = ("Your Branch", head, "Incoming", subject)
        case .rebase: (ours, oursDetail, theirs, theirsDetail) = ("Upstream", "what you're rebasing onto", "Your Commit", subject)
        case .cherryPick: (ours, oursDetail, theirs, theirsDetail) = ("Your Branch", head, "Cherry-picked", subject)
        case .revert: (ours, oursDetail, theirs, theirsDetail) = ("Your Branch", head, "Reverted", "the code before " + subject)
        case nil: (ours, oursDetail, theirs, theirsDetail) = ("Ours", head, "Theirs", "")
        }
    }
}

// MARK: - Session: the three versions and the result being edited

@MainActor @Observable
final class MergeSession: NSObject, NSTextStorageDelegate, NSTextViewDelegate {
    enum Role { case ours, theirs, base, result }
    enum Take { case ours, theirs, oursThenTheirs, theirsThenOurs, base, markers }
    enum Side { case ours, theirs }
    struct Pick: Hashable { var side: Side; var line: Int }
    struct Sides { var ours: [Substring]; var base: [Substring]?; var theirs: [Substring] }
    /// A stretch of the result that came from a change: an auto-merged one, or a conflict. Its range follows edits.
    struct Region {
        var range: NSRange
        var kind: Merge3.Kind
        var chunk: Int?                 // in merge.chunks; nil for a conflict that couldn't be matched to one
        var sides: Sides?               // a conflict's three versions
        var picks: [Pick] = []
    }
    struct Tint { var range: NSRange; var color: NSColor }

    let model: WorktreeModel
    let path: String
    var url: URL { URL(fileURLWithPath: model.path).appendingPathComponent(path) }

    private(set) var loaded = false
    private(set) var stages = ConflictStages("")
    private(set) var binary = false
    private(set) var sizes: [Int: Int] = [:]          // stage → bytes, for binary files
    private(set) var editedOutside = false            // the file's markers don't match the stages: it's tracked by its markers
    private(set) var mergeTool: (name: String, gui: Bool)?
    private(set) var current = 0                      // the conflict in focus, counting conflicts only
    private(set) var resolved: [Bool] = []            // per conflict: no markers left in it
    private(set) var markersLeft = true
    var showBase = false
    var suggesting = false
    var proposal: String?

    @ObservationIgnored let ours = NSTextStorage()
    @ObservationIgnored let theirs = NSTextStorage()
    @ObservationIgnored let base = NSTextStorage()
    @ObservationIgnored let result = NSTextStorage()
    @ObservationIgnored private(set) var merge: Merge3?
    @ObservationIgnored private var regions: [Region] = []
    @ObservationIgnored private var conflictRegions: [Int] = []   // indices into regions
    @ObservationIgnored private var starts: [Role: [Int]] = [:]   // utf16 offset of each line, plus the end
    @ObservationIgnored private var paneTints: [Role: [Tint]] = [:]
    @ObservationIgnored private var views: [Role: Weak] = [:]
    @ObservationIgnored private var suppress = false              // loading text: not an edit
    @ObservationIgnored private var replacing: Int?               // the region an action is replacing
    @ObservationIgnored private var known: String?                // what's on disk as far as we know
    @ObservationIgnored private var writeTask: Task<Void, Never>?
    @ObservationIgnored private var noFinalNewline = false
    @ObservationIgnored private var marker = (ours: "HEAD", theirs: "theirs")
    private final class Weak { weak var view: NSTextView?; init(_ v: NSTextView) { view = v } }

    init(model: WorktreeModel, path: String) {
        self.model = model
        self.path = path
        super.init()
        result.delegate = self
    }

    var conflictCount: Int { conflictRegions.count }
    var unresolvedCount: Int { resolved.filter { !$0 }.count }
    var isText: Bool { !binary && (stages.kind == .text || stages.kind == .bothAdded) }
    private var currentRegion: Int? { conflictRegions.indices.contains(current) ? conflictRegions[current] : nil }

    // MARK: Loading

    func load() async {
        writeTask?.cancel(); writeTask = nil
        let dir = model.path
        stages = ConflictStages((try? await Git.string(["ls-files", "-u", "-z", "--", path], in: dir)) ?? "")
        var data: [Int: Data] = [:]
        for n in stages.entries.keys where stages.kind != .submodule {
            data[n] = try? await Git.run(["show", ":\(n):\(path)"], in: dir)
        }
        sizes = data.mapValues(\.count)
        binary = data.values.contains { $0.looksBinary || String(data: $0, encoding: .utf8) == nil }
        mergeTool = await Self.configuredMergeTool(in: dir)
        let disk = (try? Data(contentsOf: url)).flatMap { String(data: $0, encoding: .utf8) }
        known = disk
        if isText {
            let text = { (n: Int) in data[n].map { String(decoding: $0, as: UTF8.self) } ?? "" }
            await build(base: text(1), ours: text(2), theirs: text(3), disk: disk)
        } else if let n = [2, 3].first(where: { data[$0] != nil }), !binary {   // delete/modify: show what's left
            let lines = Merge3.lines(String(decoding: data[n]!, as: UTF8.self))
            set(ours, await Self.highlighted(lines, path: path))
            starts[.ours] = Self.lineStarts(lines)
        }
        loaded = true
    }

    private func build(base b: String, ours o: String, theirs t: String, disk: String?) async {
        let path = path, mk = marker
        let built = await Task.detached(priority: .userInitiated) { () -> Built in
            let m = Merge3(base: b, ours: o, theirs: t)
            let blocks = disk.map { ConflictText.blocks(in: $0) } ?? []
            // Git's file and ours agree on the conflicts: build the result from the chunks, which marks what merged on its own.
            // Otherwise it was edited elsewhere (or merged differently): keep it, and track only its markers.
            let outside = disk != nil && blocks.count != m.conflictCount
            var regions: [Region] = [], resultLines: [Substring] = []
            if outside, let disk {
                resultLines = Merge3.lines(disk)
                var used = Set<Int>()
                for blk in blocks {
                    let j = m.chunks.indices.first { j in
                        let c = m.chunks[j]
                        return c.kind == .conflict && !used.contains(j) && m.ours[c.ours].elementsEqual(blk.conflict.current) && m.theirs[c.theirs].elementsEqual(blk.conflict.incoming)
                    }
                    if let j { used.insert(j) }
                    let base = blk.conflict.base ?? j.map { Array(m.base[m.chunks[$0].base]) }
                    regions.append(Region(range: blk.range, kind: .conflict, chunk: j, sides: Sides(ours: blk.conflict.current, base: base, theirs: blk.conflict.incoming)))
                }
            } else {
                var loc = 0
                for (j, c) in m.chunks.enumerated() {
                    var sides: Sides?
                    let lines: [Substring]
                    if c.kind == .conflict {
                        let s = Sides(ours: Array(m.ours[c.ours]), base: Array(m.base[c.base]), theirs: Array(m.theirs[c.theirs]))
                        sides = s
                        lines = Self.markerBlock(s, mk)
                    } else { lines = Array(m.merged(c)) }
                    let len = lines.reduce(0) { $0 + $1.utf16.count + 1 }
                    if c.kind != .same { regions.append(Region(range: NSRange(location: loc, length: len), kind: c.kind, chunk: j, sides: sides)) }
                    resultLines += lines
                    loc += len
                }
            }
            return Built(merge: m, regions: regions, outside: outside,
                         ours: Self.highlight(m.ours, path: path), theirs: Self.highlight(m.theirs, path: path),
                         base: Self.highlight(m.base, path: path), result: Self.highlight(resultLines, path: path))
        }.value
        let m = built.merge
        merge = m
        editedOutside = built.outside
        noFinalNewline = !built.outside && m.noFinalNewline
        regions = built.regions
        conflictRegions = regions.indices.filter { regions[$0].kind == .conflict }
        set(ours, built.ours.value); set(theirs, built.theirs.value); set(base, built.base.value); set(result, built.result.value)
        starts = [.ours: Self.lineStarts(m.ours), .theirs: Self.lineStarts(m.theirs), .base: Self.lineStarts(m.base)]
        paneTints = [.ours: [], .theirs: [], .base: []]
        for c in m.chunks {
            switch c.kind {
            case .same: break
            case .ours: paneTints[.ours]!.append(Tint(range: lines(.ours, c.ours), color: Palette.ours))
            case .theirs: paneTints[.theirs]!.append(Tint(range: lines(.theirs, c.theirs), color: Palette.theirs))
            case .both:
                paneTints[.ours]!.append(Tint(range: lines(.ours, c.ours), color: Palette.both))
                paneTints[.theirs]!.append(Tint(range: lines(.theirs, c.theirs), color: Palette.both))
            case .conflict:
                paneTints[.ours]!.append(Tint(range: lines(.ours, c.ours), color: Palette.conflict))
                paneTints[.theirs]!.append(Tint(range: lines(.theirs, c.theirs), color: Palette.conflict))
            }
            if c.kind != .same { paneTints[.base]!.append(Tint(range: lines(.base, c.base), color: c.kind == .conflict ? Palette.conflict : Palette.changed)) }
        }
        current = 0
        refreshDerived()
        redraw()
    }

    /// Sendable carrier for text attributed off the main thread; never mutated after.
    struct Frozen: @unchecked Sendable { let value: NSAttributedString }
    private struct Built: @unchecked Sendable {
        var merge: Merge3, regions: [Region], outside: Bool
        var ours: Frozen, theirs: Frozen, base: Frozen, result: Frozen
    }

    private func set(_ s: NSTextStorage, _ a: NSAttributedString) {
        suppress = true
        s.setAttributedString(a)
        suppress = false
    }

    /// Re-reads everything when the file or its stages changed behind our back (a terminal, the merge tool).
    func reloadIfChangedOutside() async {
        guard loaded else { return }
        let disk = (try? Data(contentsOf: url)).flatMap { String(data: $0, encoding: .utf8) }
        let st = ConflictStages((try? await Git.string(["ls-files", "-u", "-z", "--", path], in: model.path)) ?? "")
        guard disk != known || st != stages else { return }
        writeTask?.cancel()
        await load()
    }

    nonisolated static func configuredMergeTool(in dir: String) async -> (name: String, gui: Bool)? {
        func get(_ k: String) async -> String? {
            let v = (try? await Git.string(["config", "--get", k], in: dir))?.trimmingCharacters(in: .whitespacesAndNewlines)
            return v?.isEmpty == false ? v : nil
        }
        if let g = await get("merge.guitool") { return (g, true) }
        return await get("merge.tool").map { ($0, false) }
    }

    // MARK: Editing

    func register(_ v: NSTextView, _ role: Role) { views[role] = Weak(v) }

    nonisolated func textStorage(_ ts: NSTextStorage, didProcessEditing mask: NSTextStorageEditActions, range: NSRange, changeInLength delta: Int) {
        guard mask.contains(.editedCharacters) else { return }
        MainActor.assumeIsolated { edited(range, delta) }
    }

    /// Keeps each region on its text: edits before it move it, edits touching it grow or shrink it.
    private func edited(_ range: NSRange, _ delta: Int) {
        guard !suppress else { return }
        for i in regions.indices { regions[i].range = Self.moved(regions[i].range, edit: range, delta: delta, owns: i == replacing) }
        redraw()
        writeTask?.cancel()
        writeTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(350))
            guard !Task.isCancelled else { return }
            self?.commit()
        }
    }

    /// Where a region is after an edit (`edit` is the edited range after it, `delta` the change in length). `owns`: the
    /// edit replaced this region, so it takes the new text even when it was empty.
    nonisolated static func moved(_ r: NSRange, edit: NSRange, delta: Int, owns: Bool) -> NSRange {
        let lo = edit.location, hi = edit.location + edit.length - delta   // the edited stretch before the edit
        let end = r.location + r.length
        if !owns, hi <= r.location { return NSRange(location: r.location + delta, length: r.length) }
        if !owns, lo >= end { return r }
        let start = min(r.location, lo)
        return NSRange(location: start, length: max(0, max(end, hi) + delta - start))
    }

    /// Writes the result and works out what's still unresolved.
    private func commit() {
        writeTask?.cancel()
        var s = result.string
        if noFinalNewline, s.hasSuffix("\n") { s.removeLast() }
        if s != known {
            do { try Data(s.utf8).write(to: url, options: .atomic); known = s }
            catch { model.error = error.localizedDescription }
        }
        refreshDerived()
    }

    func flush() { if writeTask != nil { commit() } }

    private func refreshDerived() {
        let ns = result.string as NSString
        let r = conflictRegions.map { i in
            let range = regions[i].range
            return range.location + range.length <= ns.length && ConflictText(ns.substring(with: range)).conflictCount == 0
        }
        if r != resolved { resolved = r }
        let left = ConflictText(result.string).conflictCount > 0
        if left != markersLeft { markersLeft = left }
    }

    /// Replaces conflict `k` (default: the current one) with one of its versions.
    func apply(_ take: Take, to k: Int? = nil) {
        let k = k ?? current
        guard conflictRegions.indices.contains(k), let s = regions[conflictRegions[k]].sides else { return }
        let lines: [Substring]
        switch take {
        case .ours: lines = s.ours
        case .theirs: lines = s.theirs
        case .oursThenTheirs: lines = s.ours + s.theirs
        case .theirsThenOurs: lines = s.theirs + s.ours
        case .base: guard let b = s.base else { return }; lines = b
        case .markers: lines = Self.markerBlock(s, marker)
        }
        regions[conflictRegions[k]].picks = []
        replace(k, with: lines)
    }

    func canUseBase(_ k: Int? = nil) -> Bool {
        let k = k ?? current
        return conflictRegions.indices.contains(k) && regions[conflictRegions[k]].sides?.base != nil
    }

    private func replace(_ k: Int, with lines: [Substring]) {
        let i = conflictRegions[k]
        let range = regions[i].range
        let text = lines.map { $0 + "\n" }.joined()
        let attr = Self.highlight(lines, path: path).value
        replacing = i
        defer { replacing = nil }
        if let tv = views[.result]?.view {
            guard tv.shouldChangeText(in: range, replacementString: text) else { return }   // registers the undo
            result.replaceCharacters(in: range, with: attr)
            tv.didChangeText()
        } else {
            result.replaceCharacters(in: range, with: attr)
        }
        commit()
        redraw()
    }

    /// A click on a line of a conflict in Ours or Theirs: adds it to that conflict's result, or takes it out again.
    func click(_ role: Role, at index: Int) {
        guard role == .ours || role == .theirs, let m = merge, let st = starts[role] else { return }
        let line = Self.line(at: index, in: st)
        let side: Side = role == .ours ? .ours : .theirs
        guard let k = conflictRegions.firstIndex(where: { i in
            regions[i].chunk.map { j in (side == .ours ? m.chunks[j].ours : m.chunks[j].theirs).contains(line) } ?? false
        }) else { return }
        let i = conflictRegions[k]
        let p = Pick(side: side, line: line)
        var picks = regions[i].picks
        if let at = picks.firstIndex(of: p) { picks.remove(at: at) } else { picks.append(p) }
        regions[i].picks = picks
        current = k
        if picks.isEmpty { apply(.markers, to: k); return }
        // The side picked from first comes first; each side's lines keep their order.
        let order: [Side] = picks[0].side == .ours ? [.ours, .theirs] : [.theirs, .ours]
        let lines = order.flatMap { s in picks.filter { $0.side == s }.map(\.line).sorted().map { s == .ours ? m.ours[$0] : m.theirs[$0] } }
        replace(k, with: lines)
        regions[i].picks = picks
        redraw()
    }

    // MARK: Navigation

    func go(_ delta: Int) {
        guard conflictCount > 0 else { return }
        focus((current + delta + conflictCount) % conflictCount)
    }

    /// The next conflict that still has markers, after the current one.
    func nextUnresolved() {
        guard let k = (1...max(1, conflictCount)).map({ (current + $0) % max(1, conflictCount) }).first(where: { resolved.indices.contains($0) && !resolved[$0] }) else { return }
        focus(k)
    }

    func focus(_ k: Int) {
        guard conflictRegions.indices.contains(k) else { return }
        current = k
        let r = regions[conflictRegions[k]]
        if let tv = views[.result]?.view {
            Self.reveal(r.range, in: tv)
            tv.setSelectedRange(NSRange(location: r.range.location, length: 0))
        }
        if let j = r.chunk, let m = merge {
            let c = m.chunks[j]
            for (role, lr) in [(Role.ours, c.ours), (.theirs, c.theirs), (.base, c.base)] {
                if let tv = views[role]?.view { Self.reveal(lines(role, lr), in: tv) }
            }
        }
        redraw()
    }

    /// Scrolls so the range sits a little below the top, the way editors jump to a match.
    private static func reveal(_ range: NSRange, in tv: NSTextView) {
        guard let lm = tv.layoutManager, let tc = tv.textContainer, let scroll = tv.enclosingScrollView else { return }
        let len = (tv.string as NSString).length
        let r = NSRange(location: min(range.location, len), length: 0)
        lm.ensureLayout(forCharacterRange: NSRange(location: 0, length: min(len, r.location + 1)))
        let rect = lm.boundingRect(forGlyphRange: lm.glyphRange(forCharacterRange: r, actualCharacterRange: nil), in: tc)
        let y = max(0, rect.minY + tv.textContainerOrigin.y - scroll.contentView.bounds.height / 4)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: min(y, max(0, tv.frame.height - scroll.contentView.bounds.height))))
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    func textViewDidChangeSelection(_ n: Notification) {
        guard let tv = n.object as? NSTextView else { return }
        let loc = tv.selectedRange().location
        if let k = conflictRegions.firstIndex(where: { NSLocationInRange(loc, regions[$0].range) }), k != current { current = k; redraw() }
    }

    // MARK: Tints

    func tints(_ role: Role) -> [Tint] {
        if role == .result {
            return regions.indices.map { i in
                let r = regions[i]
                let k = conflictRegions.firstIndex(of: i)
                let color: NSColor = switch r.kind {
                case .ours: Palette.ours
                case .theirs: Palette.theirs
                case .both, .same: Palette.both
                case .conflict: k.map { resolved.indices.contains($0) && resolved[$0] } == true
                    ? (k == current ? Palette.resolvedCurrent : Palette.resolved)
                    : (k == current ? Palette.conflictCurrent : Palette.conflict)
                }
                return Tint(range: r.range, color: color)
            }
        }
        var t = paneTints[role] ?? []
        if let i = currentRegion, let j = regions[i].chunk, let m = merge {
            let c = m.chunks[j]
            t.append(Tint(range: lines(role, role == .ours ? c.ours : role == .theirs ? c.theirs : c.base), color: Palette.conflictCurrent))
        }
        for i in conflictRegions {
            for p in regions[i].picks where (p.side == .ours) == (role == .ours) && role != .base {
                t.append(Tint(range: lines(role, p.line..<p.line + 1), color: p.side == .ours ? Palette.oursPicked : Palette.theirsPicked))
            }
        }
        return t
    }

    private func redraw() { for w in views.values { w.view?.needsDisplay = true } }

    private func lines(_ role: Role, _ r: Range<Int>) -> NSRange {
        guard let st = starts[role], r.lowerBound < st.count, r.upperBound < st.count else { return NSRange(location: 0, length: 0) }
        return NSRange(location: st[r.lowerBound], length: st[r.upperBound] - st[r.lowerBound])
    }

    // MARK: AI

    func suggest() async {
        guard let i = currentRegion, let s = regions[i].sides else { return }
        let ns = result.string as NSString
        let r = regions[i].range
        let before = ns.substring(to: min(r.location, ns.length)).split(separator: "\n", omittingEmptySubsequences: false).suffix(20).joined(separator: "\n")
        let after = ns.substring(from: min(r.location + r.length, ns.length)).split(separator: "\n", omittingEmptySubsequences: false).prefix(20).joined(separator: "\n")
        suggesting = true
        defer { suggesting = false }
        do { proposal = try await MergeAI.suggest(path: path, sides: s, before: before, after: after, in: model.path) }
        catch { model.error = error.localizedDescription }
    }

    func acceptProposal() {
        guard let p = proposal else { return }
        proposal = nil
        if let i = currentRegion { regions[i].picks = [] }
        replace(current, with: Merge3.lines(p))
    }

    // MARK: Helpers

    nonisolated static func markerBlock(_ s: Sides, _ labels: (ours: String, theirs: String)) -> [Substring] {
        var l: [Substring] = [Substring("<<<<<<< " + labels.ours)] + s.ours
        if let b = s.base { l += ["||||||| base"] + b }
        return l + ["======="] + s.theirs + [Substring(">>>>>>> " + labels.theirs)]
    }

    nonisolated static func lineStarts(_ lines: [Substring]) -> [Int] {
        var st = [0]; st.reserveCapacity(lines.count + 1)
        for l in lines { st.append(st.last! + l.utf16.count + 1) }
        return st
    }

    nonisolated static func line(at index: Int, in st: [Int]) -> Int {
        var lo = 0, hi = st.count - 1   // the last line whose start ≤ index
        while lo < hi { let mid = (lo + hi + 1) / 2; if st[mid] <= index { lo = mid } else { hi = mid - 1 } }
        return min(lo, max(0, st.count - 2))
    }

    nonisolated static func highlighted(_ lines: [Substring], path: String) async -> NSAttributedString {
        await Task.detached(priority: .userInitiated) { highlight(lines, path: path) }.value.value
    }

    /// The lines, each ending in a newline, in the editor's font and the code theme's colours.
    nonisolated static func highlight(_ lines: some Collection<Substring>, path: String) -> Frozen {
        let font = EditorSettings.shared.font, height = EditorSettings.shared.lineHeight
        let para = NSMutableParagraphStyle()
        para.lineSpacing = max(0, height - (font.ascender - font.descender + font.leading))
        let s = NSMutableAttributedString(string: lines.map { $0 + "\n" }.joined(), attributes: [.font: font, .foregroundColor: Palette.text, .paragraphStyle: para])
        guard let lang = Language.forPath(path) else { return Frozen(value: s) }
        var state = Syntax.State(), loc = 0
        s.beginEditing()
        for l in lines {
            for span in Syntax.tokenize(String(l), lang, &state) {
                s.addAttribute(.foregroundColor, value: Syntax.color(span.kind), range: NSRange(location: loc + Int(span.start), length: Int(span.length)))
            }
            loc += l.utf16.count + 1
        }
        s.endEditing()
        return Frozen(value: s)
    }
}

enum Palette {
    static let text = NSColor(name: nil) { EditorSettings.shared.palette(dark: $0.isDark).foreground }
    static let background = NSColor(name: nil) { EditorSettings.shared.palette(dark: $0.isDark).background }
    static let ours = NSColor.systemBlue.withAlphaComponent(0.13)
    static let oursPicked = NSColor.systemBlue.withAlphaComponent(0.32)
    static let theirs = NSColor.systemPurple.withAlphaComponent(0.13)
    static let theirsPicked = NSColor.systemPurple.withAlphaComponent(0.32)
    static let both = NSColor.systemTeal.withAlphaComponent(0.12)
    static let changed = NSColor.systemGray.withAlphaComponent(0.15)
    static let conflict = NSColor.systemOrange.withAlphaComponent(0.16)
    static let conflictCurrent = NSColor.systemOrange.withAlphaComponent(0.3)
    static let resolved = NSColor.systemGreen.withAlphaComponent(0.12)
    static let resolvedCurrent = NSColor.systemGreen.withAlphaComponent(0.25)
}

extension ConflictText {
    /// Each `<<<<<<<` … `>>>>>>>` block in the file: its UTF-16 range (through the closing line's newline) and its sides.
    static func blocks(in text: String) -> [(range: NSRange, conflict: Conflict)] {
        func marker(_ l: Substring, _ c: Character) -> Bool {
            l.count >= 7 && l.prefix(7).allSatisfy { $0 == c } && (l.count == 7 || l[l.index(l.startIndex, offsetBy: 7)] == " ")
        }
        var out: [(NSRange, Conflict)] = []
        var loc = 0, start: Int?, block: [Substring] = []
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let len = line.utf16.count + 1
            if start == nil, marker(line, "<") { start = loc; block = [] }
            if let s = start {
                block.append(line)
                if marker(line, ">") {
                    if case .conflict(let c)? = ConflictText(block.joined(separator: "\n")).segments.first {
                        out.append((NSRange(location: s, length: min(loc + len, (text as NSString).length) - s), c))
                    }
                    start = nil
                }
            }
            loc += len
        }
        return out
    }
}

// MARK: - Code panes

/// Fills whole lines behind the text: which side a change came from, the conflicts, the lines picked.
final class TintLayoutManager: NSLayoutManager {
    var tints: () -> [MergeSession.Tint] = { [] }

    override func drawBackground(forGlyphRange glyphs: NSRange, at origin: NSPoint) {
        let chars = characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
        let width = textContainers.first?.textView?.bounds.width ?? 10_000
        // ponytail: every tint checked per draw; fine for thousands of changes, an interval index past that.
        for t in tints() {
            if t.range.length == 0 {   // a conflict resolved to nothing: a bar where it was
                guard NSLocationInRange(t.range.location, NSRange(location: chars.location, length: chars.length + 1)) else { continue }
                let y: CGFloat
                if t.range.location < (textStorage?.length ?? 0) {
                    y = lineFragmentRect(forGlyphAt: glyphIndexForCharacter(at: t.range.location), effectiveRange: nil).minY
                } else { y = extraLineFragmentRect.height > 0 ? extraLineFragmentRect.minY : usedRect(for: textContainers[0]).maxY }
                t.color.withAlphaComponent(min(1, t.color.alphaComponent * 3)).setFill()
                NSRect(x: 0, y: y + origin.y - 1, width: width, height: 2).fill()
                continue
            }
            let r = NSIntersectionRange(t.range, chars)
            guard r.length > 0 else { continue }
            t.color.setFill()
            enumerateLineFragments(forGlyphRange: glyphRange(forCharacterRange: r, actualCharacterRange: nil)) { rect, _, _, _, _ in
                NSRect(x: 0, y: rect.minY + origin.y, width: width, height: rect.height).fill(using: .sourceOver)
            }
        }
        super.drawBackground(forGlyphRange: glyphs, at: origin)
    }
}

/// A plain click (no drag, no selection) reports where it landed.
final class PickTextView: NSTextView {
    var onClick: ((Int) -> Void)?
    override func mouseDown(with e: NSEvent) {
        super.mouseDown(with: e)   // tracks the drag until mouse-up
        if e.clickCount == 1, selectedRange().length == 0, let onClick { onClick(selectedRange().location) }
    }
}

struct MergeCodePane: NSViewRepresentable {
    var session: MergeSession
    var role: MergeSession.Role

    func makeNSView(context: Context) -> NSScrollView {
        let storage = switch role { case .ours: session.ours; case .theirs: session.theirs; case .base: session.base; case .result: session.result }
        let lm = TintLayoutManager()
        lm.allowsNonContiguousLayout = true
        lm.tints = { [weak session, role] in session?.tints(role) ?? [] }
        storage.addLayoutManager(lm)
        let tc = NSTextContainer(size: NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude))
        tc.widthTracksTextView = false
        lm.addTextContainer(tc)
        let tv = PickTextView(frame: .zero, textContainer: tc)
        tv.isEditable = role == .result
        tv.isSelectable = true
        tv.allowsUndo = true
        tv.isRichText = false
        tv.usesFindBar = true
        tv.isIncrementalSearchingEnabled = true
        for off in [\NSTextView.isAutomaticQuoteSubstitutionEnabled, \.isAutomaticDashSubstitutionEnabled, \.isAutomaticTextReplacementEnabled,
                    \.isAutomaticSpellingCorrectionEnabled, \.isContinuousSpellCheckingEnabled, \.isAutomaticLinkDetectionEnabled] { tv[keyPath: off] = false }
        tv.drawsBackground = true
        tv.backgroundColor = Palette.background
        tv.insertionPointColor = Palette.text
        tv.textContainerInset = NSSize(width: 4, height: 6)
        tv.minSize = .zero
        tv.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        tv.isVerticallyResizable = true
        tv.isHorizontallyResizable = true
        tv.autoresizingMask = [.width]
        let font = EditorSettings.shared.font
        tv.typingAttributes = [.font: font, .foregroundColor: Palette.text]
        if role == .result { tv.delegate = session }
        if role == .ours || role == .theirs { tv.onClick = { [weak session, role] i in session?.click(role, at: i) } }
        let scroll = NSScrollView()
        scroll.documentView = tv
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = true
        scroll.backgroundColor = Palette.background
        session.register(tv, role)
        return scroll
    }

    func updateNSView(_ v: NSScrollView, context: Context) {}
}

// MARK: - The editor

/// Replaces the diff for a conflicted file: Ours and Theirs (and Base) side by side, the merged result below.
struct ConflictPane: View {
    var model: WorktreeModel
    var path: String
    @State private var session: MergeSession?

    var body: some View {
        Group {
            if let session, session.loaded { MergeEditor(model: model, session: session) }
            else { ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity) }
        }
        .task {
            let s = MergeSession(model: model, path: path)
            session = s
            await s.load()
            s.focus(0)
        }
        .task(id: model.statusVersion) { await session?.reloadIfChangedOutside() }   // a terminal, the merge tool
        .onDisappear { session?.flush() }
    }
}

private struct MergeEditor: View {
    var model: WorktreeModel
    @Bindable var session: MergeSession
    @State private var confirmReset = false
    @AppStorage("aiHarness") private var aiHarness = CommitAI.defaultHarness   // re-render when Settings changes it

    private var path: String { session.path }
    private var labels: MergeLabels { MergeLabels(operation: model.operation, branch: model.branch, subject: model.operationSubject) }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if session.isText { editor }
            else { SpecialConflict(model: model, session: session, labels: labels) }
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
            Text(path).fontWeight(.medium).lineLimit(1).truncationMode(.head).textSelection(.enabled)
            if session.stages.kind == .bothAdded { Text("added on both sides").foregroundStyle(.secondary) }
            if session.isText && session.conflictCount > 0 {
                HStack(spacing: 2) {
                    Button { session.go(-1) } label: { Image(systemName: "chevron.up") }.shortcut(.previousConflict).help("Previous conflict")
                    Button { session.go(1) } label: { Image(systemName: "chevron.down") }.shortcut(.nextConflict).help("Next conflict")
                }
                .buttonStyle(.borderless)
                Text("Conflict \(min(session.current + 1, session.conflictCount)) of \(session.conflictCount)").monospacedDigit()
                if session.unresolvedCount > 0 {
                    Text("· \(session.unresolvedCount) left").foregroundStyle(.secondary).monospacedDigit()
                } else {
                    Label("all resolved", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                }
            }
            Spacer()
            Group {
                Button("Use \(labels.ours)") { Task { await model.resolve(path, take: .current) } }
                    .help("Take the whole file from \(labels.ours.lowercased()) (git checkout --ours) and mark it resolved")
                Button("Use \(labels.theirs)") { Task { await model.resolve(path, take: .incoming) } }
                    .help("Take the whole file from \(labels.theirs.lowercased()) (git checkout --theirs) and mark it resolved")
                Button("Reset…") { confirmReset = true }
                    .help("Throw away the edits to this file and bring its conflicts back (git checkout -m)")
                    .confirmationDialog("Bring back the conflicts in \((path as NSString).lastPathComponent)?", isPresented: $confirmReset) {
                        Button("Reset File", role: .destructive) {
                            Task { await model.perform("Resetting") { try await Git.run(["checkout", "-m", "--", path], in: model.path) }; await session.load(); session.focus(0) }
                        }
                    } message: { Text("Your edits to the result are lost.") }
                if session.isText {
                    let tool = session.mergeTool
                    Button("Merge Tool") {
                        session.flush()
                        Task {
                            await model.perform("Merging in \(tool?.name ?? "")") {
                                try await Git.run(["mergetool", "--no-prompt"] + (tool?.gui == true ? ["--gui"] : []) + ["--", path], in: model.path)
                            }
                            await session.reloadIfChangedOutside()
                        }
                    }
                    .disabled(tool == nil || model.busy != nil)
                    .help(tool.map { "Open the file in \($0.name) (git mergetool)" } ?? "Set merge.tool in your git config to use an external merge tool")
                    Button("Mark Resolved") { session.flush(); Task { await model.resolve(path, take: nil) } }
                        .buttonStyle(.borderedProminent)
                        .disabled(session.markersLeft)
                        .help(session.markersLeft ? "Resolve every conflict first: the result still has conflict markers" : "Stage the result (git add)")
                }
            }
            .controlSize(.small)
        }
        .padding(.horizontal, 12).padding(.vertical, 7)
        .background(.bar, ignoresSafeAreaEdges: [])
    }

    private var editor: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                pane(.ours, labels.ours, labels.oursDetail, .blue)
                if session.showBase { Divider(); pane(.base, "Base", "the common ancestor", .gray) }
                Divider()
                pane(.theirs, labels.theirs, labels.theirsDetail, .purple)
            }
            .frame(maxHeight: .infinity)
            Divider()
            chunkBar
            Divider()
            MergeCodePane(session: session, role: .result).frame(maxHeight: .infinity)
            if session.editedOutside {
                Text("This file was changed outside Loupe, so only its remaining conflict markers are tracked.")
                    .font(.caption).foregroundStyle(.secondary).padding(.horizontal, 12).padding(.vertical, 4)
                    .frame(maxWidth: .infinity, alignment: .leading).background(.bar)
            }
        }
    }

    private func pane(_ role: MergeSession.Role, _ title: String, _ detail: String, _ tint: Color) -> some View {
        VStack(spacing: 0) {
            HStack(spacing: 5) {
                Circle().fill(tint).frame(width: 7, height: 7)
                Text(title).font(.caption.weight(.semibold))
                Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                Spacer()
                if role == .ours {
                    Toggle("Base", isOn: $session.showBase).toggleStyle(.button).controlSize(.mini)
                        .help("Show the common ancestor between the two sides")
                }
            }
            .padding(.horizontal, 8).padding(.vertical, 4)
            .background(.bar)
            MergeCodePane(session: session, role: role).id(role == .base ? "base" : "\(role)")
        }
        .frame(maxWidth: .infinity)
    }

    private var chunkBar: some View {
        HStack(spacing: 6) {
            Text("Result").font(.caption.weight(.semibold))
            if session.conflictCount > 0 {
                Text("· Conflict \(session.current + 1)").font(.caption).foregroundStyle(.secondary)
                Group {
                    Button("Use \(labels.ours)") { session.apply(.ours) }.shortcut(.useOurs)
                    Button("Use \(labels.theirs)") { session.apply(.theirs) }.shortcut(.useTheirs)
                    Button("\(labels.ours), Then \(labels.theirs)") { session.apply(.oursThenTheirs) }
                    Button("\(labels.theirs), Then \(labels.ours)") { session.apply(.theirsThenOurs) }
                    Button("Use Base") { session.apply(.base) }.disabled(!session.canUseBase())
                        .help("The lines as they were before either side changed them")
                    Button("Restore") { session.apply(.markers) }
                        .help("Put the conflict markers back")
                    if let h = CommitAI.selected {
                        if session.suggesting { ProgressView().controlSize(.small).frame(width: 20) }
                        else {
                            Button { Task { await session.suggest() } } label: { Label("Suggest", systemImage: "sparkles") }
                                .help("Ask \(h.name) for a merge of this conflict; you review it before it goes in")
                                .popover(isPresented: Binding(get: { session.proposal != nil }, set: { if !$0 { session.proposal = nil } }), arrowEdge: .bottom) {
                                    ProposalView(text: session.proposal ?? "", accept: { session.acceptProposal() }, discard: { session.proposal = nil })
                                }
                        }
                    }
                }
                .controlSize(.small)
                Spacer()
                Text("Click lines above to pick them").font(.caption).foregroundStyle(.tertiary).lineLimit(1)
                if session.unresolvedCount > 0 {
                    Button("Next Unresolved") { session.nextUnresolved() }.controlSize(.small)
                }
            } else {
                Text("· merged without conflicts; edit it freely").font(.caption).foregroundStyle(.secondary)
                Spacer()
            }
        }
        .padding(.horizontal, 10).padding(.vertical, 5)
        .background(.bar)
    }
}

private struct ProposalView: View {
    var text: String
    var accept: () -> Void
    var discard: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Suggested merge").font(.headline)
            ScrollView([.vertical, .horizontal]) {
                Text(text.isEmpty ? "(nothing)" : text).font(.editor(EditorSettings.shared.fontSize)).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(6)
            }
            .frame(width: 520, height: 260)
            .background(Color(nsColor: Palette.background), in: RoundedRectangle(cornerRadius: 6))
            Text("Check it before you use it: it replaces this conflict in the result, and ⌘Z takes it back.").font(.caption).foregroundStyle(.secondary)
            HStack { Spacer(); Button("Discard", action: discard); Button("Use Suggestion", action: accept).buttonStyle(.borderedProminent).keyboardShortcut(.defaultAction) }
        }
        .padding(14)
    }
}

/// Conflicts that aren't about lines: a side deleted the file, it's binary, it's a submodule, or only one side has it.
private struct SpecialConflict: View {
    var model: WorktreeModel
    var session: MergeSession
    var labels: MergeLabels
    private var path: String { session.path }

    var body: some View {
        let kind = session.stages.kind
        VStack(spacing: 0) {
            VStack(spacing: 10) {
                Image(systemName: icon(kind)).font(.system(size: 28)).foregroundStyle(.orange)
                Text(title(kind)).font(.headline)
                Text(detail(kind)).foregroundStyle(.secondary).multilineTextAlignment(.center).frame(maxWidth: 460)
                HStack { buttons(kind) }.controlSize(.regular).padding(.top, 4)
            }
            .padding(24)
            .frame(maxWidth: .infinity)
            if kind != .submodule, !session.binary, session.ours.length > 0 {
                Divider()
                MergeCodePane(session: session, role: .ours)
            } else { Spacer() }
        }
    }

    private func icon(_ k: ConflictStages.Kind) -> String {
        switch k {
        case .submodule: "shippingbox"
        case .deletedByUs, .deletedByThem: "trash"
        default: session.binary ? "doc.richtext" : "doc.questionmark"
        }
    }

    private func title(_ k: ConflictStages.Kind) -> String {
        switch k {
        case .submodule: "Submodule conflict"
        case .deletedByUs: "Deleted on \(labels.ours), changed on \(labels.theirs)"
        case .deletedByThem: "Changed on \(labels.ours), deleted on \(labels.theirs)"
        case .addedByUs: "Only on \(labels.ours)"
        case .addedByThem: "Only on \(labels.theirs)"
        default: session.binary ? "Binary file conflict" : "Conflict"
        }
    }

    private func detail(_ k: ConflictStages.Kind) -> String {
        switch k {
        case .submodule: "Both sides moved the submodule to a different commit. Pick the commit to keep."
        case .deletedByUs, .deletedByThem: "Keep the changed file, or delete it as the other side did."
        case .addedByUs, .addedByThem: "The other side doesn't have this path, usually because it renamed or deleted the file. Keep it or delete it."
        default:
            session.binary ? "Binary files can't be merged line by line. Keep one side: \(labels.ours) \(size(2)), \(labels.theirs) \(size(3))." : "Keep one side."
        }
    }

    private func size(_ stage: Int) -> String {
        session.sizes[stage].map { ByteCountFormatter.string(fromByteCount: Int64($0), countStyle: .file) } ?? "none"
    }

    @ViewBuilder private func buttons(_ k: ConflictStages.Kind) -> some View {
        switch k {
        case .submodule:
            ForEach([2, 3], id: \.self) { st in
                if let e = session.stages.entries[st] {
                    Button("Use \(st == 2 ? labels.ours : labels.theirs) · \(e.sha.prefix(8))") {
                        Task { await model.perform("Resolving") { try await Git.run(["update-index", "--cacheinfo", "160000,\(e.sha),\(path)"], in: model.path) } }
                    }
                    .help("Record the submodule at \(e.sha)")
                }
            }
        case .deletedByUs, .deletedByThem, .addedByUs, .addedByThem:
            Button("Keep File") { Task { await model.resolve(path, take: k == .deletedByThem || k == .addedByUs ? .current : .incoming) } }
                .buttonStyle(.borderedProminent)
            Button("Delete File", role: .destructive) {
                Task { await model.perform("Resolving") { try await Git.run(["rm", "-q", "--", path], in: model.path) } }
            }
        default:
            Button("Use \(labels.ours)") { Task { await model.resolve(path, take: .current) } }
            Button("Use \(labels.theirs)") { Task { await model.resolve(path, take: .incoming) } }
        }
    }
}

// MARK: - AI

/// One conflict merged by the coding-agent CLI set up for commit messages; read-only, the three versions in the prompt.
enum MergeAI {
    @MainActor static func suggest(path: String, sides s: MergeSession.Sides, before: String, after: String, in dir: String) async throws -> String {
        guard let h = CommitAI.selected, let exe = AIHarnesses.shared.paths[h.id] else {
            throw CommitAI.Failure(errorDescription: "No AI tool is set up. Pick one in Settings › General.")
        }
        let j = { (l: [Substring]) in l.joined(separator: "\n") }
        let prompt = """
        You resolve one git merge conflict in \(path). Below are the lines before it, the base version (the common ancestor), \
        ours and theirs, and the lines after it. Combine the intent of both changes. Reply with only the lines that replace the \
        conflict: no code fences, no explanation, no conflict markers, and keep the file's indentation. Don't run tools or look at \
        files; everything you need is here.

        Before:
        \(before)
        Base:
        \(s.base.map(j) ?? "(not available)")
        Ours:
        \(j(s.ours))
        Theirs:
        \(j(s.theirs))
        After:
        \(after)
        """
        let out = FileManager.default.temporaryDirectory.appendingPathComponent("loupe-merge-\(UUID().uuidString).txt").path
        defer { try? FileManager.default.removeItem(atPath: out) }
        let inv = h.invocation(prompt, CommitAI.model(for: h), out)
        var text = try await CommitAI.run(exe, inv.args, stdin: inv.stdin ? prompt : nil, in: dir)
        if h.readsOutputFile, let f = try? String(contentsOfFile: out, encoding: .utf8), !f.isEmpty { text = f }
        return clean(text)
    }

    /// Drops ANSI escapes and code fences, and blank lines around the code; keeps its indentation.
    static func clean(_ s: String) -> String {
        var lines = s.replacingOccurrences(of: "\u{1B}\\[[0-9;?]*[A-Za-z]", with: "", options: .regularExpression)
            .split(separator: "\n", omittingEmptySubsequences: false)
        while let f = lines.first, f.trimmingCharacters(in: .whitespaces).isEmpty || f.hasPrefix("```") { lines.removeFirst() }
        while let l = lines.last, l.trimmingCharacters(in: .whitespaces).isEmpty || l.hasPrefix("```") { lines.removeLast() }
        return lines.joined(separator: "\n")
    }
}

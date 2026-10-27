import AppKit
import SwiftUI

/// Anything rendered as an inline comment under a diff line.
struct InlineComment: Hashable {
    var author: String
    var body: String
    var newLine: Int?
    var oldLine: Int?
    var draftID: UUID?           // non-nil → pending, deletable
}

enum HunkAction { case stage, unstage, discard }

/// Virtualized diff renderer. NSTableView only materializes visible rows and each row draws itself
/// with one attributed-string draw, so 100k-line diffs scroll at display refresh rate.
enum CodeAction { case definition, references }

/// A request to bring a new-side line into view; a new `id` repeats it.
struct DiffReveal: Equatable { var line: Int; var id = UUID() }

/// What a diff shows: both sides with the changes marked, or one side alone (the file before, or after).
enum DiffSide: String { case diff, old, new }

struct DiffTableView: NSViewRepresentable {
    var file: FileDiff
    @AppStorage("diffSide") private var side = DiffSide.diff
    @Environment(\.diffBlame) private var blame
    var comments: [InlineComment] = []
    var hunkActions: [HunkAction] = []                       // empty → no hunk buttons (review mode)
    var onHunkAction: ((HunkAction, _ lines: Set<Int>) -> Void)?   // lines: indices into file.lines (a hunk or a selection)
    var onAddComment: ((_ newLine: Int?, _ oldLine: Int?, _ body: String) -> Void)?   // nil → commenting disabled
    var onDeleteDraft: ((UUID) -> Void)?
    var canComment: ((DiffLine) -> Bool)? = nil                   // nil → every line (when commenting is on)
    var onNavigate: ((_ word: String, _ action: CodeAction) -> Void)? = nil   // nil → no ⌘-click / Go to Definition
    var reveal: DiffReveal? = nil                                 // scroll to a line and select it
    var changeRuler = false                                       // marks for the changes beside the vertical scroller

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let table = DiffTable()
        table.headerView = nil
        table.usesAlternatingRowBackgroundColors = false
        table.intercellSpacing = .zero
        table.selectionHighlightStyle = .none   // selection is character-level, drawn by the rows themselves
        table.allowsMultipleSelection = false
        table.gridStyleMask = []
        table.style = .plain
        table.backgroundColor = Theme.background
        table.columnAutoresizingStyle = .noColumnAutoresizing
        let col = NSTableColumn(identifier: .init("diff"))
        table.addTableColumn(col)
        table.dataSource = context.coordinator
        table.delegate = context.coordinator
        table.coordinator = context.coordinator
        table.target = context.coordinator
        table.menu = NSMenu()
        table.menu?.delegate = context.coordinator
        context.coordinator.table = table

        let scroll = NSScrollView()
        scroll.documentView = table
        if changeRuler { scroll.verticalScroller = ChangeScroller() }
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = true
        scroll.backgroundColor = Theme.background
        scroll.documentCursor = .iBeam
        // Keep the single column at least as wide as the viewport so row backgrounds span it.
        scroll.contentView.postsFrameChangedNotifications = true
        NotificationCenter.default.addObserver(context.coordinator, selector: #selector(Coordinator.viewportResized(_:)),
                                               name: NSView.frameDidChangeNotification, object: scroll.contentView)
        NotificationCenter.default.addObserver(context.coordinator, selector: #selector(Coordinator.styleChanged(_:)),
                                               name: .editorStyleChanged, object: nil)
        context.coordinator.installFind()
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        let c = context.coordinator
        c.parent = self
        let fileChanged = c.fileUID != file.uid
        // The change ruler comes and goes with Full mode, on a view that stays.
        let swapRuler = changeRuler != (scroll.verticalScroller is ChangeScroller)
        if swapRuler { scroll.verticalScroller = changeRuler ? ChangeScroller() : NSScroller() }
        // A file with only one side (new, deleted, or only additions or removals) always shows as a diff: the header
        // hides Before / After for it.
        let side = file.additions > 0 && file.deletions > 0 ? self.side : .diff
        let rebuilt = fileChanged || c.side != side || c.blame != blame || c.comments != comments || c.hunkActions != hunkActions
        if rebuilt {
            let samePath = c.path == file.path
            var keepScroll = samePath ? scroll.contentView.bounds.origin : .zero
            // Diff / Before / After on the same file: keep the code at the top of the view in place. The same scroll
            // offset would land elsewhere, since the lines between come and go.
            // Also when the same file reloads (Before / After load it whole): anchored by line number, not row.
            let anchor = samePath && (fileChanged || c.side != side) ? c.topLine() : nil
            c.side = side; c.blame = blame
            c.rebuild(file: file, comments: comments, hunkActions: hunkActions)
            c.fitColumn()
            c.table.reloadData()
            if !samePath { c.clearSelection() }
            if let anchor, let r = c.rows.indices.first(where: { r in
                guard let i = c.line(row: r) else { return false }
                let l = c.file.lines[i]
                if let n = anchor.new, let ln = l.newLine { return ln >= n }
                if let o = anchor.old, let lo = l.oldLine { return lo >= o }
                return false
            }) {
                keepScroll.y = max(0, c.table.rect(ofRow: r).minY - anchor.offset)
            }
            scroll.contentView.scroll(to: keepScroll)
            scroll.reflectScrolledClipView(scroll.contentView)
            c.refreshFind()
        }
        if rebuilt || swapRuler, let ruler = scroll.verticalScroller as? ChangeScroller {
            ruler.table = c.table
            ruler.onJump = { [weak c] row in c?.show(row: row) }
            ruler.marks = c.changeMarks()
        }
        if let r = reveal, r.id != c.revealed {
            c.revealed = r.id
            // After this layout pass: a file that just loaded has no row frames yet.
            DispatchQueue.main.async { c.reveal(line: r.line) }
        }
    }

    // MARK: Coordinator

    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSMenuDelegate {
        enum Row { case line(Int), hunk(Int, Int), comment(InlineComment) }   // hunk(lineIndex, hunkIndex)

        var parent: DiffTableView!
        weak var table: DiffTable!
        var rows: [Row] = []
        var file = FileDiff()
        var fileUID = UUID()
        var path = ""
        var comments: [InlineComment] = []
        var hunkActions: [HunkAction] = []
        var side = DiffSide.diff
        var blame: DiffBlame?
        var blameRows: [Int: (label: String?, odd: Bool)] = [:]   // per row showing a new-side line
        var blameWidth: CGFloat { blame == nil ? 0 : 210 }
        var lineCacheSide = DiffSide.diff
        var highlights: [Int: NSRange] = [:]     // line index → changed span (intra-line diff)
        var syntax: [[SyntaxSpan]] = []          // per line; empty until the background pass finishes
        private var syntaxTask: Task<Void, Never>?
        var contentWidth: CGFloat = 0
        var numberWidth: CGFloat = 0
        var revealed: UUID?
        var hover: (row: Int, cols: Range<Int>)?   // the identifier under the mouse while ⌘ is held
        private var commentHeights: [InlineComment: CGFloat] = [:]
        /// Lines as drawn, by line index. Colors stay dynamic, so an appearance change needs no rebuild.
        private let lineCache: NSCache<NSNumber, NSAttributedString> = {
            let c = NSCache<NSNumber, NSAttributedString>()
            c.countLimit = 1500; c.totalCostLimit = 1_000_000   // cost: UTF-16 length
            return c
        }()
        // Line numbers and +/− markers, measured per font. The small font's digits are fixed-width, so a number's width follows its digit count.
        private(set) var numAttrs: [NSAttributedString.Key: Any] = [:]
        private(set) var blameAttrs: [NSAttributedString.Key: Any] = [:]
        private(set) var numHeight: CGFloat = 0
        private var numWidths: [Int: CGFloat] = [:]
        private(set) var addMarker = NSAttributedString(), delMarker = NSAttributedString()

        func digitsWidth(_ s: NSString) -> CGFloat {
            if let w = numWidths[s.length] { return w }
            let w = s.size(withAttributes: numAttrs).width
            numWidths[s.length] = w
            return w
        }

        private func measureStyle() {
            numAttrs = [.font: Theme.smallFont, .foregroundColor: Theme.lineNumber]
            blameAttrs = [.font: NSFont.systemFont(ofSize: Theme.smallFont.pointSize), .foregroundColor: Theme.lineNumber]
            numHeight = ("0" as NSString).size(withAttributes: numAttrs).height
            numWidths = [:]
            addMarker = NSAttributedString(string: "+", attributes: [.font: Theme.font, .foregroundColor: Theme.addMark])
            delMarker = NSAttributedString(string: "−", attributes: [.font: Theme.font, .foregroundColor: Theme.delMark])
        }

        /// Line `i` as the row draws it: tabs expanded, syntax colors, the intra-line highlight.
        func attributed(line i: Int) -> NSAttributedString {
            if let s = lineCache.object(forKey: i as NSNumber) { return s }
            let l = file.lines[i]
            let text = Syntax.displayText(l.text)
            let color: NSColor = l.kind == .meta ? Theme.metaForeground : Theme.foreground
            let s = NSMutableAttributedString(string: text, attributes: [.font: Theme.font, .foregroundColor: color])
            if i < syntax.count {
                for sp in syntax[i] where Int(sp.start + sp.length) <= s.length {
                    s.addAttribute(.foregroundColor, value: Syntax.color(sp.kind), range: NSRange(location: Int(sp.start), length: Int(sp.length)))
                }
            }
            if side == .diff, let r = highlights[i], NSMaxRange(r) <= s.length, !l.text.contains("\t") {
                s.addAttribute(.backgroundColor, value: l.kind == .added ? Theme.addHL : Theme.delHL, range: r)
            }
            lineCache.setObject(s, forKey: i as NSNumber, cost: s.length)
            return s
        }

        /// ⌘ held over an identifier: underline it and show the link cursor, as an IDE does.
        func updateHover(at p: NSPoint?, command: Bool) {
            var next: (row: Int, cols: Range<Int>)?
            if command, parent.onNavigate != nil, let p, table.bounds.contains(p), let w = identifier(at: p) { next = (w.row, w.cols) }
            let changed = next?.row != hover?.row || next?.cols != hover?.cols
            let oldRow = hover?.row
            hover = next
            let cursor = next == nil ? NSCursor.iBeam : NSCursor.pointingHand
            if NSCursor.current != cursor { cursor.set() }
            guard changed else { return }
            for r in [oldRow, next?.row].compactMap({ $0 }) { table.view(atColumn: 0, row: r, makeIfNecessary: false)?.needsDisplay = true }
        }

        /// Scrolls the new-side `line` to a third of the way down and selects it.
        func reveal(line: Int) {
            guard table != nil, let r = rows.firstIndex(where: { row in
                if case .line(let i) = row { return file.lines[i].newLine == line && file.lines[i].kind != .removed }
                return false
            }) else { return }
            show(row: r)
        }

        /// Scrolls `r` to a third of the way down and selects it.
        func show(row r: Int) {
            let rect = table.rect(ofRow: r)
            let y = max(0, rect.minY - table.visibleRect.height / 3)
            table.scroll(NSPoint(x: 0, y: y))
            anchor = TextPos(row: r, col: 0); head = TextPos(row: r, col: length(row: r))
            redrawVisible()
        }

        func rebuild(file: FileDiff, comments: [InlineComment], hunkActions: [HunkAction]) {
            if fileUID != file.uid { highlights = DiffTableView.intraLineHighlights(file); lineCache.removeAllObjects(); searchTexts = nil }
            // Before / After draw plain code: no intra-line highlight in the cached lines.
            if lineCacheSide != side { lineCache.removeAllObjects(); lineCacheSide = side }
            self.file = file; fileUID = file.uid; path = file.path
            self.comments = comments; self.hunkActions = hunkActions
            measureStyle()
            let heights = commentHeights
            commentHeights = Dictionary(comments.map { ($0, heights[$0] ?? CommentRowView.height(for: $0)) }, uniquingKeysWith: { a, _ in a })
            if syntaxFileUID != file.uid { startSyntax(file) }

            var byNew: [Int: [InlineComment]] = [:], byOld: [Int: [InlineComment]] = [:]
            for c in comments {
                if let n = c.newLine, n > 0 { byNew[n, default: []].append(c) } else if let o = c.oldLine { byOld[o, default: []].append(c) }
            }
            func commentsFor(_ i: Int) -> [InlineComment] {
                let l = file.lines[i]
                switch l.kind {
                case .added: return byNew.removeValue(forKey: l.newLine!) ?? []
                case .removed: return byOld.removeValue(forKey: l.oldLine!) ?? []
                case .context: return (byNew.removeValue(forKey: l.newLine!) ?? []) + (byOld.removeValue(forKey: l.oldLine!) ?? [])
                default: return []
                }
            }

            var out: [Row] = []
            out.reserveCapacity(file.lines.count + comments.count)
            var maxLen = 0, maxNo = 0
            for (i, l) in file.lines.enumerated() {
                maxLen = max(maxLen, l.text.utf16.count)
                maxNo = max(maxNo, l.oldLine ?? 0, l.newLine ?? 0)
                if l.kind == .hunk { if side == .diff { out.append(.hunk(i, l.hunkIndex)) }; continue }   // Before / After: just the file
                if side == .old && l.kind == .added || side == .new && l.kind == .removed { continue }
                out.append(.line(i))
                out += commentsFor(i).map(Row.comment)
            }
            // Comments whose line isn't in the diff anymore (outdated) go to the end rather than vanish.
            // (One side alone leaves out the other side's lines and their comments, which aren't outdated.)
            if side == .diff { for c in (byNew.values.flatMap { $0 } + byOld.values.flatMap { $0 }) { out.append(.comment(c)) } }
            rows = out

            let cw = Theme.charWidth
            numberWidth = CGFloat(max(3, String(maxNo).count)) * cw + 12
            let textWidth = CGFloat(min(maxLen, Theme.maxDrawnChars)) * cw + 24
            contentWidth = textX + textWidth
            blameRows = blame.map(blameLabels) ?? [:]
        }

        /// Blame beside each new-side line: the commit, its author and age on the first line of each run of lines from
        /// one commit, and a shade that alternates per run.
        private func blameLabels(_ b: DiffBlame) -> [Int: (label: String?, odd: Bool)] {
            let ago = RelativeDateTimeFormatter(); ago.unitsStyle = .abbreviated
            var out: [Int: (label: String?, odd: Bool)] = [:], last: String?, odd = false
            for (r, row) in rows.enumerated() {
                guard case .line(let i) = row, let n = file.lines[i].newLine, file.lines[i].kind != .removed, n - 1 < b.shas.count else { continue }
                let sha = b.shas[n - 1]
                guard sha != last else { out[r] = (nil, odd); continue }
                if last != nil { odd.toggle() }
                last = sha
                if sha.allSatisfy({ $0 == "0" }) { out[r] = ("Not committed", odd); continue }
                let c = b.commits[sha]
                out[r] = ("\(sha.prefix(7))  \(c?.author ?? "")  · \(c.map { ago.localizedString(for: $0.date, relativeTo: Date()) } ?? "")", odd)
            }
            return out
        }

        func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

        private var syntaxFileUID: UUID?

        /// Colours arrive a beat after the text: tokenizing a 150k-line file on the main thread would stall the switch.
        private func startSyntax(_ file: FileDiff) {
            syntaxTask?.cancel()
            syntax = []
            syntaxFileUID = file.uid
            guard let lang = Language.forPath(file.path) else { return }
            let uid = file.uid
            // On the main actor: the Coordinator isn't isolated, so a bare Task would land the result on a background
            // thread, racing `draw` for `syntax` and touching the table off-main (memory corruption, crashes far away).
            syntaxTask = Task { @MainActor [weak self] in
                let spans = await Task.detached(priority: .userInitiated) { Syntax.highlight(file, language: lang) }.value
                guard let self, !Task.isCancelled, self.fileUID == uid, self.table != nil else { return }
                self.syntax = spans
                self.lineCache.removeAllObjects()
                self.redrawVisible()
            }
        }

        func fitColumn() {
            guard let table, let clip = table.enclosingScrollView?.contentView else { return }
            let w = max(contentWidth, clip.bounds.width)
            if table.tableColumns[0].width != w { table.tableColumns[0].width = w }
        }

        @objc func viewportResized(_ n: Notification) { fitColumn() }

        /// A new font or theme: widths and row heights follow the font, every row redraws.
        @objc func styleChanged(_ n: Notification) {
            guard let table else { return }
            lineCache.removeAllObjects()
            rebuild(file: file, comments: comments, hunkActions: hunkActions)
            fitColumn()
            table.reloadData()
            table.enclosingScrollView?.verticalScroller?.needsDisplay = true
        }

        /// Runs of changed rows, for the change ruler. Comments inside a run don't break it.
        func changeMarks() -> [ChangeScroller.Mark] {
            guard side == .diff else { return [] }   // Before / After are plain code
            var out: [ChangeScroller.Mark] = []
            for (r, row) in rows.enumerated() {
                guard case .line(let first) = row else { continue }
                let kind = file.lines[first].kind
                let added = kind == .added ? 1 : 0, removed = kind == .removed ? 1 : 0
                guard added + removed > 0 else { continue }
                if let last = out.last, rows[(last.rows.upperBound + 1)..<r].allSatisfy({ if case .comment = $0 { true } else { false } }) {
                    out[out.count - 1].rows = last.rows.lowerBound...r
                    out[out.count - 1].added += added; out[out.count - 1].removed += removed
                } else {
                    let l = file.lines[first]
                    out.append(.init(rows: r...r, added: added, removed: removed, line: l.newLine ?? l.oldLine ?? 0))
                }
            }
            return out
        }

        func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
            switch rows[row] {
            case .hunk: return Theme.lineHeight + 8
            case .comment(let c): return commentHeights[c] ?? CommentRowView.height(for: c)
            default: return Theme.lineHeight
            }
        }

        func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }

        func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
            switch rows[row] {
            case .comment(let c):
                let v = tableView.makeView(withIdentifier: CommentRowView.id, owner: nil) as? CommentRowView ?? CommentRowView()
                v.configure(c) { [weak self] id in self?.parent.onDeleteDraft?(id) }
                return v
            case .hunk(let li, let hi):
                let v = tableView.makeView(withIdentifier: HunkRowView.id, owner: nil) as? HunkRowView ?? HunkRowView()
                v.configure(header: file.lines[li].text, actions: hunkActions) { [weak self] a in
                    guard let self else { return }
                    self.parent.onHunkAction?(a, Set(self.file.hunks[hi].lines))
                }
                return v
            default:
                let v = tableView.makeView(withIdentifier: LineRowView.id, owner: nil) as? LineRowView ?? LineRowView()
                v.coordinator = self
                v.row = rows[row]
                v.rowIndex = row
                v.needsDisplay = true
                return v
            }
        }

        // MARK: Interaction

        /// The first diff line in view, and how far below the top of the view its row starts.
        func topLine() -> (old: Int?, new: Int?, offset: CGFloat)? {
            guard let table else { return nil }
            let visible = table.visibleRect, top = table.rows(in: visible).location
            guard top >= 0, let r = (top..<rows.count).first(where: { line(row: $0) != nil }), let i = line(row: r) else { return nil }
            let l = file.lines[i]
            return (l.kind == .added ? nil : l.oldLine, l.kind == .removed ? nil : l.newLine, table.rect(ofRow: r).minY - visible.minY)
        }

        /// Which diff line a row shows, if it shows one.
        func line(row: Int) -> Int? {
            guard row >= 0, row < rows.count, case .line(let i) = rows[row] else { return nil }
            return i
        }

        // MARK: Text selection
        // Monospaced text makes point → (row, column) exact, so the table gets real character selection
        // without turning 100k rows into an NSTextView.

        struct TextPos: Comparable {
            var row: Int, col: Int
            static func < (a: TextPos, b: TextPos) -> Bool { (a.row, a.col) < (b.row, b.col) }
        }
        var anchor: TextPos?
        var head: TextPos?

        var selection: (TextPos, TextPos)? {
            guard let a = anchor, let h = head, a != h else { return nil }
            return (min(a, h), max(a, h))
        }

        // MARK: Find

        struct Match { var row: Int; var range: NSRange }
        var findBar: FindBar?
        var matches: [Match] = []
        var matchIndex = 0
        var matchesByRow: [Int: [Int]] = [:]
        private var searchTexts: [NSString]?   // each line as drawn; built on the first search, dropped with the file
        private var findMonitor: Any?

        deinit { if let findMonitor { NSEvent.removeMonitor(findMonitor) } }

        /// ⌘F anywhere in this table's window opens the find bar, even while the file tree has the focus; ⌘G and ⇧⌘G
        /// step through the matches while it's open.
        func installFind() {
            findMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
                let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask), key = e.charactersIgnoringModifiers?.lowercased()
                guard key == "f" && mods == .command || key == "g" && (mods == .command || mods == [.command, .shift]) else { return e }
                let windowNumber = e.windowNumber
                // Local monitors run on the main thread.
                let handled = MainActor.assumeIsolated { () -> Bool in
                    guard let self, let table = self.table, table.window?.windowNumber == windowNumber, !table.isHiddenOrHasHiddenAncestor else { return false }
                    if key == "f" { self.openFind(); return true }
                    guard self.findBar?.superview != nil, table.enclosingScrollView?.isFindBarVisible == true else { return false }
                    self.step(mods.contains(.shift) ? -1 : 1)
                    return true
                }
                return handled ? nil : e
            }
        }

        func openFind() {
            guard let scroll = table.enclosingScrollView else { return }
            if findBar == nil {
                let bar = FindBar()
                bar.onChange = { [weak self] q in self?.search(q) }
                bar.onStep = { [weak self] d in self?.step(d) }
                bar.onClose = { [weak self] in self?.closeFind() }
                findBar = bar
                scroll.findBarPosition = .aboveContent
                scroll.findBarView = bar
            }
            scroll.isFindBarVisible = true
            findBar?.focus()
            if let q = findBar?.query, !q.isEmpty { search(q) }
        }

        func closeFind() {
            table.enclosingScrollView?.isFindBarVisible = false
            matches = []; matchesByRow = [:]; searchTexts = nil
            redrawVisible()
            table.window?.makeFirstResponder(table)
        }

        /// After the file or layout changed under an open find bar.
        func refreshFind() {
            guard table.enclosingScrollView?.isFindBarVisible == true, let q = findBar?.query, !q.isEmpty else { return }
            search(q, scroll: false)
        }

        func search(_ q: String, scroll: Bool = true) {
            var found: [Match] = []
            if !q.isEmpty {
                if searchTexts == nil { searchTexts = file.lines.map { Syntax.displayText($0.text) as NSString } }
                let texts = searchTexts!
                for (r, row) in rows.enumerated() {
                    guard case .line(let i) = row else { continue }
                    let t = texts[i]
                    var range = NSRange(location: 0, length: t.length)
                    while range.length > 0 {
                        let f = t.range(of: q, options: [.caseInsensitive, .diacriticInsensitive], range: range)
                        if f.location == NSNotFound { break }
                        found.append(Match(row: r, range: f))
                        range = NSRange(location: NSMaxRange(f), length: t.length - NSMaxRange(f))
                    }
                }
            }
            matches = found
            matchesByRow = Dictionary(grouping: found.indices) { found[$0].row }
            // Start at the first match from the top of the view down, as an editor does.
            let top = table.rows(in: table.visibleRect).location
            matchIndex = found.firstIndex { $0.row >= top } ?? 0
            findBar?.setCount(found.isEmpty ? (q.isEmpty ? nil : 0) : matchIndex + 1, of: found.count)
            redrawVisible()
            if scroll, !found.isEmpty { scrollToMatch() }
        }

        func step(_ d: Int) {
            guard !matches.isEmpty else { return }
            let old = matches[matchIndex].row
            matchIndex = (matchIndex + d + matches.count) % matches.count
            findBar?.setCount(matchIndex + 1, of: matches.count)
            let new = matches[matchIndex].row
            redrawVisible { $0 == old || $0 == new }
            scrollToMatch()
        }

        /// Brings the current match into view: centred vertically when it's off screen, and sideways when it's past the edge.
        private func scrollToMatch() {
            let m = matches[matchIndex]
            let rowRect = table.rect(ofRow: m.row), visible = table.visibleRect
            let x = textX + CGFloat(m.range.location) * Theme.charWidth, w = CGFloat(m.range.length) * Theme.charWidth
            let y = rowRect.minY >= visible.minY && rowRect.maxY <= visible.maxY ? visible.minY : max(0, rowRect.midY - visible.height / 2)
            let left = x >= visible.minX && x + w <= visible.maxX ? visible.minX : max(0, x - visible.width / 3)
            if y != visible.minY || left != visible.minX { table.scroll(NSPoint(x: left, y: y)) }
        }

        /// Where a row's text starts: after both line numbers.
        var textX: CGFloat { blameWidth + numberWidth * (side == .diff ? 2 : 1) + 16 }

        func text(row: Int) -> [UInt16] {
            line(row: row).map { Array(Syntax.displayText(file.lines[$0].text).utf16) } ?? []
        }

        /// The drawn length of a row, without copying its text.
        func length(row: Int) -> Int { line(row: row).map { attributed(line: $0).length } ?? 0 }

        private func hitRow(_ p: NSPoint) -> Int {
            let row = table.row(at: p)
            return row >= 0 ? row : p.y < 0 ? 0 : max(0, rows.count - 1)
        }

        func pos(_ p: NSPoint) -> TextPos {
            let row = hitRow(p)
            let len = length(row: row)
            let col = Int(((p.x - textX) / Theme.charWidth).rounded())
            return TextPos(row: row, col: max(0, min(len, col)))
        }

        /// The identifier around a text position, as a column range (empty when there's none).
        func wordRange(at: TextPos) -> Range<Int> { wordRange(in: text(row: at.row), col: at.col) }

        private func wordRange(in u: [UInt16], col: Int) -> Range<Int> {
            // Left of the text (over the line numbers) the column is negative: no word there.
            guard col >= 0 else { return 0..<0 }
            func isWord(_ c: UInt16) -> Bool { (c >= 48 && c <= 57) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122) || c == 95 || c > 127 }
            var a = min(col, u.count), b = a
            if a < u.count, isWord(u[a]) || (a > 0 && isWord(u[a - 1])) {
                if a == u.count || !isWord(u[a]) { a -= 1; b = a }
                while a > 0, isWord(u[a - 1]) { a -= 1 }
                while b < u.count, isWord(u[b]) { b += 1 }
            }
            return a..<b
        }

        /// The identifier under a point, if it is one (not a number or punctuation), with where it is.
        func identifier(at p: NSPoint) -> (row: Int, cols: Range<Int>, word: String)? {
            let row = hitRow(p)
            let u = text(row: row)
            let r = wordRange(in: u, col: Int(((p.x - textX) / Theme.charWidth).rounded(.down)))
            guard !r.isEmpty, !(u[r.lowerBound] >= 48 && u[r.lowerBound] <= 57) else { return nil }
            return (row, r, String(utf16CodeUnits: Array(u[r]), count: r.count))
        }

        func word(at p: NSPoint) -> String? { identifier(at: p)?.word }

        func mouseDown(_ p: NSPoint, clicks: Int, command: Bool = false) {
            if command, clicks == 1, let nav = parent.onNavigate, let w = identifier(at: p) {   // ⌘-click: go to definition
                anchor = TextPos(row: w.row, col: w.cols.lowerBound); head = TextPos(row: w.row, col: w.cols.upperBound)
                redrawVisible()
                nav(w.word, .definition)
                return
            }
            let at = pos(p)
            switch clicks {
            case 2:   // word
                var r = wordRange(at: at)
                if r.isEmpty, r.lowerBound < length(row: at.row) { r = r.lowerBound..<(r.lowerBound + 1) }
                anchor = TextPos(row: at.row, col: r.lowerBound); head = TextPos(row: at.row, col: r.upperBound)
            case 3...:  // line
                anchor = TextPos(row: at.row, col: 0); head = TextPos(row: at.row, col: length(row: at.row))
            default:
                anchor = at; head = at
            }
            redrawVisible()
        }

        func mouseDragged(_ p: NSPoint) {
            guard let a = anchor else { return }
            let old = head, new = pos(p)
            head = new
            // Only rows between the old and new head, and the anchor's, change.
            let lo = min(old?.row ?? a.row, new.row), hi = max(old?.row ?? a.row, new.row)
            redrawVisible { $0 == a.row || (lo...hi).contains($0) }
        }

        func selectAll() {
            guard !rows.isEmpty else { return }
            anchor = TextPos(row: 0, col: 0)
            head = TextPos(row: rows.count - 1, col: length(row: rows.count - 1))
            redrawVisible()
        }

        func clearSelection() { anchor = nil; head = nil; redrawVisible() }

        /// Selected column range on a row, if any.
        func selectedColumns(row: Int) -> Range<Int>? {
            guard let (s, e) = selection, row >= s.row, row <= e.row, line(row: row) != nil else { return nil }
            let len = length(row: row)
            let a = row == s.row ? s.col : 0
            let b = row == e.row ? e.col : len + 1   // +1: a full-line selection shows its newline
            return a < b ? a..<b : nil
        }

        /// Marks every prepared row (also those just outside the viewport, which would keep stale pixels), or those `rows` picks.
        func redrawVisible(_ rows: ((Int) -> Bool)? = nil) {
            table?.enumerateAvailableRowViews { v, r in
                if rows?(r) ?? true { (v.view(atColumn: 0) as? NSView)?.needsDisplay = true }
            }
        }

        func beginComment(lineIndex i: Int, row: Int) {
            guard let onAdd = parent.onAddComment else { return }
            let l = file.lines[i]
            guard l.kind != .hunk, l.kind != .meta else { return }
            let newLine = l.kind == .removed ? nil : l.newLine
            let oldLine = l.kind == .removed ? l.oldLine : nil
            let pop = NSPopover()
            pop.behavior = .transient
            let label = newLine.map { "Line \($0)" } ?? "Line \(oldLine ?? 0) (old)"
            pop.contentViewController = NSHostingController(rootView: CommentComposer(title: "\(file.path) · \(label)") { body in
                pop.close()
                if let body, !body.isEmpty { onAdd(newLine, oldLine, body) }
            })
            let rect = table.rect(ofRow: row)
            pop.show(relativeTo: NSRect(x: table.visibleRect.minX + 60, y: rect.minY, width: 40, height: rect.height), of: table, preferredEdge: .maxY)
        }

        func menuNeedsUpdate(_ menu: NSMenu) {
            menu.removeAllItems()
            let row = table.clickedRow
            let p = table.window.map { table.convert($0.mouseLocationOutsideOfEventStream, from: nil) }
            if let nav = parent.onNavigate, let p, row >= 0, let w = word(at: NSPoint(x: p.x, y: table.rect(ofRow: row).midY)) {
                menu.addItem(ClosureMenuItem("Go to Definition of “\(w)”") { nav(w, .definition) })
                menu.addItem(ClosureMenuItem("Find References to “\(w)”") { nav(w, .references) })
                menu.addItem(.separator())
            }
            if let i = line(row: row) {
                if parent.onAddComment != nil, parent.canComment?(file.lines[i]) ?? true {
                    menu.addItem(ClosureMenuItem("Add Comment…") { [weak self] in self?.beginComment(lineIndex: i, row: row) })
                }
                // Lines: the selection if the click is inside it, else the clicked line if it's a change.
                let changed = { (j: Int) in self.file.lines[j].kind == .added || self.file.lines[j].kind == .removed }
                let selected = selectedLines()
                let lines: Set<Int> = selected.contains(i) ? selected.filter(changed) : changed(i) ? [i] : []
                for a in hunkActions where !lines.isEmpty {
                    let what = lines.count == 1 ? "Line" : "\(lines.count) Lines"
                    let title = a == .stage ? "Stage \(what)" : a == .unstage ? "Unstage \(what)" : "Discard \(what)…"
                    menu.addItem(ClosureMenuItem(title) { [weak self] in self?.parent.onHunkAction?(a, lines) })
                }
                let hunk = file.hunks.indices.contains(file.lines[i].hunkIndex) ? Set(file.hunks[file.lines[i].hunkIndex].lines) : []
                for a in hunkActions where !hunk.isEmpty {
                    let title = a == .stage ? "Stage Hunk" : a == .unstage ? "Unstage Hunk" : "Discard Hunk…"
                    menu.addItem(ClosureMenuItem(title) { [weak self] in self?.parent.onHunkAction?(a, hunk) })
                }
                if menu.items.count > 0 { menu.addItem(.separator()) }
            }
            menu.addItem(ClosureMenuItem("Copy") { [weak self] in self?.table.copy(nil) })
        }

        /// Diff lines on the selected rows.
        func selectedLines() -> Set<Int> {
            guard let (s, e) = selection else { return [] }
            return Set((s.row...e.row).compactMap { line(row: $0) })
        }

        func copySelection() {
            guard let text = selectedText() else { return }
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
        }

        func selectedText() -> String? {
            guard let (s, e) = selection else { return nil }
            var out: [String] = []
            for r in s.row...e.row where line(row: r) != nil {
                let u = text(row: r)
                let a = min(r == s.row ? s.col : 0, u.count), b = min(r == e.row ? e.col : u.count, u.count)
                // ponytail: copies the displayed text, so tabs come out as four spaces
                out.append(a < b ? String(utf16CodeUnits: Array(u[a..<b]), count: b - a) : "")
            }
            return out.joined(separator: "\n")
        }

        /// n / p: jump to the next or previous change: a hunk header, or in a whole-file view the first line of a run
        /// of changes. It lands a few lines below the top, so the context above it shows.
        func jumpHunk(forward: Bool) {
            func isChange(_ r: Int) -> Bool {
                switch rows[r] {
                case .hunk: return true
                case .line(let i): return file.lines[i].kind == .added || file.lines[i].kind == .removed
                case .comment: return false
                }
            }
            func starts(_ r: Int) -> Bool {
                guard isChange(r) else { return false }
                if case .hunk = rows[r] { return true }
                return r == 0 || !isChange(r - 1)   // a change right under its hunk header isn't a second stop
            }
            let margin = 3
            let start = table.rows(in: table.visibleRect).location + margin
            let range = forward ? Array(min(rows.count, start + 1)..<rows.count) : Array((0..<max(0, start)).reversed())
            for r in range where starts(r) {
                if case .hunk = rows[r] { table.scroll(NSPoint(x: table.visibleRect.minX, y: table.rect(ofRow: r).minY)); return }
                table.scroll(NSPoint(x: table.visibleRect.minX, y: max(0, table.rect(ofRow: max(0, r - margin)).minY)))
                return
            }
        }
    }

    /// For each removed/added run pair, mark the span between the common prefix and suffix.
    nonisolated static func intraLineHighlights(_ f: FileDiff) -> [Int: NSRange] {
        var out: [Int: NSRange] = [:]
        var i = 0
        let lines = f.lines
        while i < lines.count {
            guard lines[i].kind == .removed else { i += 1; continue }
            var dels: [Int] = [], adds: [Int] = []
            while i < lines.count, lines[i].kind == .removed { dels.append(i); i += 1 }
            while i < lines.count, lines[i].kind == .added { adds.append(i); i += 1 }
            guard dels.count == adds.count, dels.count <= 50 else { continue }   // only highlight 1:1 edits
            for (d, a) in zip(dels, adds) {
                let x = Array(lines[d].text.utf16), y = Array(lines[a].text.utf16)
                guard x.count < 1000, y.count < 1000 else { continue }
                var p = 0
                while p < x.count, p < y.count, x[p] == y[p] { p += 1 }
                var s = 0
                while s < x.count - p, s < y.count - p, x[x.count - 1 - s] == y[y.count - 1 - s] { s += 1 }
                // Whole-line change → no highlight; it would only add noise.
                if p == 0 && s == 0 { continue }
                if x.count - p - s > 0 { out[d] = NSRange(location: p, length: x.count - p - s) }
                if y.count - p - s > 0 { out[a] = NSRange(location: p, length: y.count - p - s) }
            }
        }
        return out
    }
}

// MARK: - Table subclass (keyboard + copy)

/// A vertical scroller with a mark beside the track for every run of changes, at its place in the file.
/// Clicking a mark scrolls to it; the knob and the rest of the track work as usual.
final class ChangeScroller: NSScroller {
    struct Mark { var rows: ClosedRange<Int>; var added: Int; var removed: Int; var line: Int }
    var marks: [Mark] = [] { didSet { placedRects = nil; hovered = nil; toolTip = nil; needsDisplay = true } }
    weak var table: NSTableView?
    var onJump: ((Int) -> Void)?

    // Not drawn as an overlay: the scroll view keeps the track visible, so the marks are always there.
    override class var isCompatibleWithOverlayScrollers: Bool { false }

    // Placed once per slot and table height: the slot draws every scroll frame.
    private var placedRects: (slot: NSRect, height: CGFloat, rects: [NSRect])?
    private var hovered: Int?

    private func placed() -> [NSRect] {
        guard let table, table.bounds.height > 0 else { return [] }
        let slot = rect(for: .knobSlot), h = table.bounds.height
        if let p = placedRects, p.slot == slot, p.height == h { return p.rects }
        let rects = marks.map { m in
            let top = table.rect(ofRow: m.rows.lowerBound).minY / h, bottom = table.rect(ofRow: m.rows.upperBound).maxY / h
            let height = max(3, (bottom - top) * slot.height)
            let y = isFlipped ? slot.minY + top * slot.height : slot.maxY - top * slot.height - height
            return NSRect(x: slot.minX + 3, y: y, width: slot.width - 6, height: height)
        }
        placedRects = (slot, h, rects)
        return rects
    }

    override func drawKnobSlot(in slotRect: NSRect, highlight flag: Bool) {
        Theme.background.setFill(); slotRect.fill()
        NSColor.separatorColor.setFill(); NSRect(x: slotRect.minX, y: slotRect.minY, width: 1, height: slotRect.height).fill()
        for (m, r) in zip(marks, placed()) {
            (m.removed == 0 ? NSColor.systemGreen : m.added == 0 ? NSColor.systemRed : NSColor.systemOrange).setFill()
            NSBezierPath(roundedRect: r, xRadius: 1, yRadius: 1).fill()
        }
    }

    override func drawKnob() {
        let r = rect(for: .knob).insetBy(dx: 1.5, dy: 1)
        let path = NSBezierPath(roundedRect: r, xRadius: 3, yRadius: 3)
        NSColor.labelColor.withAlphaComponent(0.12).setFill(); path.fill()
        NSColor.labelColor.withAlphaComponent(0.25).setStroke(); path.stroke()
    }

    private func mark(at p: NSPoint) -> Int? {
        guard !rect(for: .knob).contains(p) else { return nil }   // the knob still drags
        return placed().firstIndex { $0.insetBy(dx: -3, dy: -3).contains(p) }
    }

    override func mouseDown(with event: NSEvent) {
        if let i = mark(at: convert(event.locationInWindow, from: nil)) { onJump?(marks[i].rows.lowerBound) } else { super.mouseDown(with: event) }
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.filter { $0.owner === self }.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self))
    }

    override func mouseMoved(with event: NSEvent) {
        let i = mark(at: convert(event.locationInWindow, from: nil))
        let cursor = i == nil ? NSCursor.arrow : NSCursor.pointingHand
        if NSCursor.current != cursor { cursor.set() }
        guard i != hovered else { return }
        hovered = i
        toolTip = i.map { marks[$0] }.map { m in "Line \(m.line): " + [m.added > 0 ? "+\(m.added)" : nil, m.removed > 0 ? "−\(m.removed)" : nil].compactMap { $0 }.joined(separator: " ") }
    }

    override func mouseExited(with event: NSEvent) { NSCursor.arrow.set() }
}

final class DiffTable: NSTableView {
    weak var coordinator: DiffTableView.Coordinator?

    override func keyDown(with event: NSEvent) {
        switch event.charactersIgnoringModifiers {
        case "n": coordinator?.jumpHunk(forward: true)
        case "p": coordinator?.jumpHunk(forward: false)
        case "v": NotificationCenter.default.post(name: .markViewed, object: nil)   // only PR review listens
        case "c" where event.modifierFlags.contains(.command): copy(nil)
        default: super.keyDown(with: event)
        }
    }

    @objc func copy(_ sender: Any?) { coordinator?.copySelection() }
    override func selectAll(_ sender: Any?) { coordinator?.selectAll() }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        coordinator?.mouseDown(convert(event.locationInWindow, from: nil), clicks: event.clickCount, command: event.modifierFlags.contains(.command))
    }

    override func mouseDragged(with event: NSEvent) {
        autoscroll(with: event)
        coordinator?.mouseDragged(convert(event.locationInWindow, from: nil))
    }

    // ⌘-hover: links on identifiers.
    private var tracking: NSTrackingArea?
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let t = NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self)
        addTrackingArea(t)
        tracking = t
    }
    override func mouseMoved(with event: NSEvent) {
        super.mouseMoved(with: event)
        coordinator?.updateHover(at: convert(event.locationInWindow, from: nil), command: event.modifierFlags.contains(.command))
    }
    override func flagsChanged(with event: NSEvent) {
        super.flagsChanged(with: event)
        guard let w = window else { return }
        let p = convert(w.mouseLocationOutsideOfEventStream, from: nil)
        coordinator?.updateHover(at: visibleRect.contains(p) ? p : nil, command: event.modifierFlags.contains(.command))
    }
    override func mouseExited(with event: NSEvent) {
        super.mouseExited(with: event)
        coordinator?.updateHover(at: nil, command: false)
    }

    override func becomeFirstResponder() -> Bool { coordinator?.redrawVisible(); return super.becomeFirstResponder() }
    override func resignFirstResponder() -> Bool { coordinator?.redrawVisible(); return super.resignFirstResponder() }
}

// MARK: - Theme

/// The editor's metrics and colors, from `EditorSettings`. Colors resolve per appearance at draw time.
enum Theme {
    static var font: NSFont { EditorSettings.shared.font }
    static var smallFont: NSFont { EditorSettings.shared.smallFont }
    static var lineHeight: CGFloat { EditorSettings.shared.lineHeight }
    static var charWidth: CGFloat { EditorSettings.shared.charWidth }
    static var textY: CGFloat { EditorSettings.shared.textY }   // where a line's text starts inside its row
    static let maxDrawnChars = 1500   // ponytail: lines longer than this are truncated on screen (minified files); copy still gets full text

    private static func themed(_ f: @escaping (CodePalette) -> NSColor) -> NSColor {
        NSColor(name: nil) { f(EditorSettings.shared.palette(dark: $0.isDark)) }
    }
    static let background = themed { $0.background }
    static let foreground = themed { $0.foreground }
    static let metaForeground = themed { $0.metaForeground }
    static let lineNumber = themed { $0.lineNumber }
    static let addMark = themed { $0.addMark }
    static let delMark = themed { $0.delMark }
    static let addBG = themed { $0.addBG }
    static let delBG = themed { $0.delBG }
    static let addHL = themed { $0.addHL }
    static let delHL = themed { $0.delHL }
    static let gutterBG = themed { $0.gutterBG }
    static let hunkBG = themed { $0.hunkBG }
}

extension NSAppearance {
    var isDark: Bool { bestMatch(from: [.darkAqua, .aqua]) == .darkAqua }
}

// MARK: - Row views

final class LineRowView: NSView {
    static let id = NSUserInterfaceItemIdentifier("line")
    weak var coordinator: DiffTableView.Coordinator?
    var row: DiffTableView.Coordinator.Row = .line(0)
    var rowIndex = 0

    override func hitTest(_ point: NSPoint) -> NSView? { nil }   // the table owns mouse handling (text selection)

    override init(frame: NSRect) { super.init(frame: frame); identifier = Self.id }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        guard let c = coordinator else { return }
        let nw = c.numberWidth
        switch row {
        case .line(let i):
            let l = c.file.lines[i]
            // Before and After show their own side's numbers only.
            let nums: [Int?] = c.side == .old ? [l.oldLine] : c.side == .new ? [l.newLine] : [l.oldLine, l.newLine]
            drawLine(l, index: i, numbers: nums, nw: nw, width: bounds.width, c: c)
        default: break
        }
        if let h = c.hover, h.row == rowIndex, !h.cols.isEmpty {   // ⌘-hover link
            let x = c.textX + CGFloat(h.cols.lowerBound) * Theme.charWidth
            NSColor.linkColor.setFill()
            NSRect(x: x, y: bounds.height - 2.5, width: CGFloat(h.cols.count) * Theme.charWidth, height: 1.2).fill()
        }
    }

    private func drawLine(_ l: DiffLine, index: Int, numbers nums: [Int?], nw: CGFloat, width: CGFloat, c: DiffTableView.Coordinator) {
        let h = bounds.height, x = c.blameWidth, diff = c.side == .diff
        if c.blame != nil {
            let b = c.blameRows[rowIndex]
            NSColor.labelColor.withAlphaComponent(b?.odd == true ? 0.07 : 0.035).setFill(); NSRect(x: 0, y: 0, width: x, height: h).fill()
            if let label = b?.label {
                NSGraphicsContext.saveGraphicsState()
                NSRect(x: 0, y: 0, width: x - 6, height: h).clip()
                (label as NSString).draw(at: NSPoint(x: 8, y: (h - c.numHeight) / 2), withAttributes: c.blameAttrs)
                NSGraphicsContext.restoreGraphicsState()
            }
        }
        // Red and green only in the diff; Before and After are plain code.
        let bg: NSColor? = !diff ? nil : l.kind == .added ? Theme.addBG : l.kind == .removed ? Theme.delBG : nil
        if let bg { bg.setFill(); NSRect(x: x, y: 0, width: width - x, height: h).fill() }
        let gutterW = nw * CGFloat(nums.count)
        Theme.gutterBG.setFill(); NSRect(x: x, y: 0, width: gutterW, height: h).fill()

        for (k, n) in nums.enumerated() {
            guard let n else { continue }
            let s = String(n) as NSString
            s.draw(at: NSPoint(x: x + nw * CGFloat(k + 1) - c.digitsWidth(s) - 6, y: (h - c.numHeight) / 2), withAttributes: c.numAttrs)
        }
        let tx = x + gutterW + 4
        if let cols = c.selectedColumns(row: rowIndex) {
            let focused = window?.firstResponder === c.table && window?.isKeyWindow == true
            (focused ? NSColor.selectedTextBackgroundColor : NSColor.unemphasizedSelectedTextBackgroundColor).setFill()
            NSRect(x: tx + 12 + CGFloat(cols.lowerBound) * Theme.charWidth, y: 0,
                   width: CGFloat(cols.count) * Theme.charWidth, height: h).fill()
        }
        if let ms = c.matchesByRow[rowIndex] {
            for k in ms {
                let m = c.matches[k].range
                (k == c.matchIndex ? NSColor.systemOrange.withAlphaComponent(0.6) : NSColor.systemYellow.withAlphaComponent(0.3)).setFill()
                NSBezierPath(roundedRect: NSRect(x: tx + 12 + CGFloat(m.location) * Theme.charWidth - 1, y: 1,
                                                 width: CGFloat(m.length) * Theme.charWidth + 2, height: h - 2), xRadius: 2, yRadius: 2).fill()
            }
        }
        if diff, let marker = l.kind == .added ? c.addMarker : l.kind == .removed ? c.delMarker : nil { marker.draw(at: NSPoint(x: tx, y: Theme.textY)) }

        NSGraphicsContext.saveGraphicsState()
        NSRect(x: x, y: 0, width: width, height: h).clip()
        c.attributed(line: index).draw(at: NSPoint(x: tx + 12, y: Theme.textY))
        NSGraphicsContext.restoreGraphicsState()
    }
}

/// The find bar above a diff: a search field, how many matches, previous and next, Done.
final class FindBar: NSView, NSSearchFieldDelegate {
    var onChange: ((String) -> Void)?
    var onStep: ((Int) -> Void)?
    var onClose: (() -> Void)?
    private let field = NSSearchField()
    private let count = NSTextField(labelWithString: "")
    var query: String { field.stringValue }

    override init(frame: NSRect) {
        super.init(frame: NSRect(x: 0, y: 0, width: 400, height: 32))
        field.placeholderString = "Find in file"
        field.delegate = self
        field.sendsWholeSearchString = false
        count.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        count.textColor = .secondaryLabelColor
        let nav = NSSegmentedControl(images: [NSImage(systemSymbolName: "chevron.up", accessibilityDescription: "Previous")!,
                                              NSImage(systemSymbolName: "chevron.down", accessibilityDescription: "Next")!],
                                     trackingMode: .momentary, target: self, action: #selector(navigate(_:)))
        nav.controlSize = .small
        nav.setToolTip("Previous (⇧⌘G)", forSegment: 0); nav.setToolTip("Next (⌘G)", forSegment: 1)
        let done = NSButton(title: "Done", target: self, action: #selector(close))
        done.controlSize = .small
        field.controlSize = .small
        let stack = NSStackView(views: [field, count, nav, NSView(), done])
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            field.widthAnchor.constraint(equalToConstant: 260),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }
    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill(); bounds.fill()
        NSColor.separatorColor.setFill(); NSRect(x: 0, y: isFlipped ? bounds.maxY - 1 : 0, width: bounds.width, height: 1).fill()
    }

    func focus() {
        DispatchQueue.main.async { [self] in window?.makeFirstResponder(field); field.currentEditor()?.selectAll(nil) }
    }

    func setCount(_ i: Int?, of n: Int) { count.stringValue = i == nil ? "" : n == 0 ? "No results" : "\(i!) of \(n)" }

    func controlTextDidChange(_ n: Notification) { onChange?(field.stringValue) }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy sel: Selector) -> Bool {
        switch sel {
        case #selector(NSResponder.insertNewline(_:)): onStep?(NSApp.currentEvent?.modifierFlags.contains(.shift) == true ? -1 : 1); return true
        case #selector(NSResponder.cancelOperation(_:)): onClose?(); return true
        default: return false
        }
    }

    @objc private func navigate(_ s: NSSegmentedControl) { onStep?(s.selectedSegment == 0 ? -1 : 1) }
    @objc private func close() { onClose?() }
}

final class HunkRowView: NSView {
    static let id = NSUserInterfaceItemIdentifier("hunk")
    private let label = NSTextField(labelWithString: "")
    private let buttons = NSStackView()
    private var handler: ((HunkAction) -> Void)?
    private var actions: [HunkAction]?

    override init(frame: NSRect) {
        super.init(frame: frame)
        identifier = Self.id
        wantsLayer = true
        label.font = Theme.smallFont
        label.textColor = .secondaryLabelColor
        label.lineBreakMode = .byTruncatingTail
        buttons.orientation = .horizontal
        buttons.spacing = 6
        for v in [label, buttons] { v.translatesAutoresizingMaskIntoConstraints = false; addSubview(v) }
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            buttons.leadingAnchor.constraint(equalTo: label.trailingAnchor, constant: 12),
            buttons.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }
    required init?(coder: NSCoder) { fatalError() }

    override func updateLayer() { label.font = Theme.smallFont; layer?.backgroundColor = Theme.hunkBG.cgColor }

    func configure(header: Substring, actions: [HunkAction], handler: @escaping (HunkAction) -> Void) {
        label.stringValue = String(header)
        self.handler = handler
        guard actions != self.actions else { return }   // reused row: keep its buttons and their layout
        self.actions = actions
        buttons.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for a in actions {
            let title = a == .stage ? "Stage Hunk" : a == .unstage ? "Unstage Hunk" : "Discard"
            let b = NSButton(title: title, target: self, action: #selector(tap(_:)))
            b.controlSize = .small
            b.bezelStyle = .push
            b.tag = a == .stage ? 0 : a == .unstage ? 1 : 2
            if a == .discard { b.contentTintColor = .systemRed }
            buttons.addArrangedSubview(b)
        }
        needsDisplay = true
    }

    @objc private func tap(_ b: NSButton) { handler?(b.tag == 0 ? .stage : b.tag == 1 ? .unstage : .discard) }
}

final class CommentRowView: NSView {
    static let id = NSUserInterfaceItemIdentifier("comment")
    static let boxWidth: CGFloat = 620
    private static let bodyFont = NSFont.systemFont(ofSize: 12.5)
    private let author = NSTextField(labelWithString: "")
    private let body = NSTextField(wrappingLabelWithString: "")
    private let delete = NSButton()
    private let box = NSView()
    private var draftID: UUID?
    private var onDelete: ((UUID) -> Void)?

    static func height(for c: InlineComment) -> CGFloat {
        let r = (c.body as NSString).boundingRect(with: NSSize(width: boxWidth - 24, height: .greatestFiniteMagnitude),
                                                 options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: [.font: bodyFont])
        return ceil(r.height) + 46
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        identifier = Self.id
        box.wantsLayer = true
        box.layer?.cornerRadius = 8
        box.layer?.borderWidth = 1
        author.font = .systemFont(ofSize: 11, weight: .semibold)
        author.textColor = .secondaryLabelColor
        body.font = Self.bodyFont
        body.isSelectable = true
        delete.image = NSImage(systemSymbolName: "trash", accessibilityDescription: "Delete draft")
        delete.isBordered = false
        delete.target = self
        delete.action = #selector(del)
        for v in [box, author, body, delete] { v.translatesAutoresizingMaskIntoConstraints = false; addSubview(v) }
        NSLayoutConstraint.activate([
            box.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 56),
            box.widthAnchor.constraint(equalToConstant: Self.boxWidth),
            box.topAnchor.constraint(equalTo: topAnchor, constant: 5),
            box.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -5),
            author.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 12),
            author.topAnchor.constraint(equalTo: box.topAnchor, constant: 7),
            delete.trailingAnchor.constraint(equalTo: box.trailingAnchor, constant: -8),
            delete.centerYAnchor.constraint(equalTo: author.centerYAnchor),
            body.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 12),
            body.trailingAnchor.constraint(equalTo: box.trailingAnchor, constant: -12),
            body.topAnchor.constraint(equalTo: author.bottomAnchor, constant: 4),
        ])
    }
    required init?(coder: NSCoder) { fatalError() }

    func configure(_ c: InlineComment, onDelete: @escaping (UUID) -> Void) {
        author.stringValue = c.draftID != nil ? "\(c.author) · Pending" : c.author
        body.stringValue = c.body
        draftID = c.draftID
        delete.isHidden = c.draftID == nil
        self.onDelete = onDelete
        needsLayout = true
        updateColors()
    }

    override func viewDidChangeEffectiveAppearance() { updateColors() }

    private func updateColors() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            let accent = draftID != nil ? NSColor.systemOrange : NSColor.controlAccentColor
            box.layer?.backgroundColor = accent.withAlphaComponent(0.07).cgColor
            box.layer?.borderColor = accent.withAlphaComponent(0.35).cgColor
        }
    }

    @objc private func del() { if let id = draftID { onDelete?(id) } }
}

final class ClosureMenuItem: NSMenuItem {
    private let handler: () -> Void
    init(_ title: String, handler: @escaping () -> Void) {
        self.handler = handler
        super.init(title: title, action: #selector(fire), keyEquivalent: "")
        target = self
    }
    required init(coder: NSCoder) { fatalError() }
    @objc private func fire() { handler() }
}

struct CommentComposer: View {
    var title: String
    var done: (String?) -> Void
    @State private var text = ""
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.caption).foregroundStyle(.secondary)
            TextEditor(text: $text)
                .font(.body)
                .frame(width: 380, height: 110)
                .focused($focused)
                .scrollContentBackground(.hidden)
                .padding(4)
                .background(.background.secondary, in: RoundedRectangle(cornerRadius: 6))
            HStack {
                Spacer()
                Button("Cancel") { done(nil) }.keyboardShortcut(.cancelAction)
                Button("Add Comment") { done(text.trimmingCharacters(in: .whitespacesAndNewlines)) }
                    .keyboardShortcut(.return, modifiers: .command)
                    .buttonStyle(.borderedProminent)
                    .disabled(text.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(12)
        // The popover window only becomes key after it's on screen; focusing in onAppear is too early.
        .onAppear { DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { focused = true } }
    }
}

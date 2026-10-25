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

enum DiffLayout: String, CaseIterable { case unified = "Unified", split = "Split" }

enum HunkAction { case stage, unstage, discard }

/// Virtualized diff renderer. NSTableView only materializes visible rows and each row draws itself
/// with one attributed-string draw, so 100k-line diffs scroll at display refresh rate.
enum CodeAction { case definition, references }

/// A request to bring a new-side line into view; a new `id` repeats it.
struct DiffReveal: Equatable { var line: Int; var id = UUID() }

struct DiffTableView: NSViewRepresentable {
    var file: FileDiff
    var layout: DiffLayout = .unified
    var comments: [InlineComment] = []
    var hunkActions: [HunkAction] = []                       // empty → no hunk buttons (review mode)
    var onHunkAction: ((HunkAction, _ lines: Set<Int>) -> Void)?   // lines: indices into file.lines (a hunk or a selection)
    var onAddComment: ((_ newLine: Int?, _ oldLine: Int?, _ body: String) -> Void)?   // nil → commenting disabled
    var onDeleteDraft: ((UUID) -> Void)?
    var canComment: ((DiffLine) -> Bool)? = nil                   // nil → every line (when commenting is on)
    var onNavigate: ((_ word: String, _ action: CodeAction) -> Void)? = nil   // nil → no ⌘-click / Go to Definition
    var reveal: DiffReveal? = nil                                 // scroll to a line and select it

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
        table.backgroundColor = .textBackgroundColor
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
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = true
        scroll.backgroundColor = .textBackgroundColor
        scroll.documentCursor = .iBeam
        // Keep the single column at least as wide as the viewport so row backgrounds span it.
        scroll.contentView.postsFrameChangedNotifications = true
        NotificationCenter.default.addObserver(context.coordinator, selector: #selector(Coordinator.viewportResized(_:)),
                                               name: NSView.frameDidChangeNotification, object: scroll.contentView)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        let c = context.coordinator
        c.parent = self
        let fileChanged = c.fileUID != file.uid
        if fileChanged || c.layout != layout || c.comments != comments || c.hunkActions != hunkActions {
            let samePath = c.path == file.path
            let keepScroll = samePath ? scroll.contentView.bounds.origin : .zero
            c.rebuild(file: file, layout: layout, comments: comments, hunkActions: hunkActions)
            c.fitColumn()
            c.table.reloadData()
            if !samePath { c.clearSelection() }
            scroll.contentView.scroll(to: keepScroll)
            scroll.reflectScrolledClipView(scroll.contentView)
        }
        if let r = reveal, r.id != c.revealed {
            c.revealed = r.id
            // After this layout pass: a file that just loaded has no row frames yet.
            DispatchQueue.main.async { c.reveal(line: r.line) }
        }
    }

    // MARK: Coordinator

    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSMenuDelegate {
        enum Row { case line(Int), split(Int?, Int?), hunk(Int, Int), comment(InlineComment) }   // hunk(lineIndex, hunkIndex)

        var parent: DiffTableView!
        weak var table: DiffTable!
        var rows: [Row] = []
        var file = FileDiff()
        var fileUID = UUID()
        var path = ""
        var layout = DiffLayout.unified
        var comments: [InlineComment] = []
        var hunkActions: [HunkAction] = []
        var highlights: [Int: NSRange] = [:]     // line index → changed span (intra-line diff)
        var syntax: [[SyntaxSpan]] = []          // per line; empty until the background pass finishes
        private var syntaxTask: Task<Void, Never>?
        var contentWidth: CGFloat = 0
        var numberWidth: CGFloat = 0
        var revealed: UUID?
        var hover: (row: Int, side: Int, cols: Range<Int>)?   // the identifier under the mouse while ⌘ is held

        /// ⌘ held over an identifier: underline it and show the link cursor, as an IDE does.
        func updateHover(at p: NSPoint?, command: Bool) {
            var next: (row: Int, side: Int, cols: Range<Int>)?
            if command, parent.onNavigate != nil, let p, table.bounds.contains(p), word(at: p) != nil {
                let side = layout == .split && p.x < (table.tableColumns[0].width / 2).rounded() ? 0 : 1
                var at = pos(p); at.col = Int(((p.x - textX(side: side)) / Theme.charWidth).rounded(.down))
                next = (at.row, side, wordRange(at: at, side: side))
            }
            let changed = next?.row != hover?.row || next?.cols != hover?.cols || next?.side != hover?.side
            let oldRow = hover?.row
            hover = next
            (next == nil ? NSCursor.iBeam : NSCursor.pointingHand).set()
            guard changed else { return }
            for r in [oldRow, next?.row].compactMap({ $0 }) { table.view(atColumn: 0, row: r, makeIfNecessary: false)?.needsDisplay = true }
        }

        /// Scrolls the new-side `line` to a third of the way down and selects it.
        func reveal(line: Int) {
            guard let table, let r = rows.firstIndex(where: { row in
                if case .line(let i) = row { return file.lines[i].newLine == line && file.lines[i].kind != .removed }
                if case .split(_, let b?) = row { return file.lines[b].newLine == line }
                return false
            }) else { return }
            let rect = table.rect(ofRow: r)
            let y = max(0, rect.minY - table.visibleRect.height / 3)
            table.scroll(NSPoint(x: 0, y: y))
            selSide = 1
            anchor = TextPos(row: r, col: 0); head = TextPos(row: r, col: text(row: r, side: 1).count)
            redrawVisible()
        }

        func rebuild(file: FileDiff, layout: DiffLayout, comments: [InlineComment], hunkActions: [HunkAction]) {
            self.file = file; fileUID = file.uid; path = file.path
            self.layout = layout; self.comments = comments; self.hunkActions = hunkActions
            highlights = DiffTableView.intraLineHighlights(file)
            if syntaxFileUID != file.uid { startSyntax(file) }

            var byNew: [Int: [InlineComment]] = [:], byOld: [Int: [InlineComment]] = [:]
            for c in comments {
                if let n = c.newLine, n > 0 { byNew[n, default: []].append(c) } else if let o = c.oldLine { byOld[o, default: []].append(c) }
            }
            func commentsFor(_ i: Int?) -> [InlineComment] {
                guard let i else { return [] }
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
            var i = 0
            let lines = file.lines
            while i < lines.count {
                let l = lines[i]
                maxLen = max(maxLen, l.text.utf16.count)
                maxNo = max(maxNo, l.oldLine ?? 0, l.newLine ?? 0)
                if l.kind == .hunk { out.append(.hunk(i, l.hunkIndex)); i += 1; continue }
                if layout == .unified || l.kind == .context || l.kind == .meta {
                    out.append(layout == .unified ? .line(i) : .split(i, i))
                    out += commentsFor(i).map(Row.comment)
                    i += 1; continue
                }
                // Split: pair a run of removals with the following run of additions.
                var dels: [Int] = [], adds: [Int] = []
                while i < lines.count, lines[i].kind == .removed { dels.append(i); maxLen = max(maxLen, lines[i].text.utf16.count); i += 1 }
                while i < lines.count, lines[i].kind == .added { adds.append(i); maxLen = max(maxLen, lines[i].text.utf16.count); i += 1 }
                for k in 0..<max(dels.count, adds.count) {
                    let d = k < dels.count ? dels[k] : nil, a = k < adds.count ? adds[k] : nil
                    out.append(.split(d, a))
                    out += (commentsFor(d) + commentsFor(a)).map(Row.comment)
                }
            }
            // Comments whose line isn't in the diff anymore (outdated) go to the end rather than vanish.
            for c in (byNew.values.flatMap { $0 } + byOld.values.flatMap { $0 }) { out.append(.comment(c)) }
            rows = out

            let cw = Theme.charWidth
            numberWidth = CGFloat(max(3, String(maxNo).count)) * cw + 12
            let textWidth = CGFloat(min(maxLen, Theme.maxDrawnChars)) * cw + 24
            contentWidth = layout == .unified ? numberWidth * 2 + 16 + textWidth : (numberWidth + 16 + textWidth) * 2
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
                guard let self, !Task.isCancelled, self.fileUID == uid, let table = self.table else { return }
                self.syntax = spans
                let visible = table.rows(in: table.visibleRect)
                for r in visible.location..<NSMaxRange(visible) { table.view(atColumn: 0, row: r, makeIfNecessary: false)?.needsDisplay = true }
            }
        }

        func fitColumn() {
            guard let table, let clip = table.enclosingScrollView?.contentView else { return }
            let w = max(contentWidth, clip.bounds.width)
            if table.tableColumns[0].width != w { table.tableColumns[0].width = w }
        }

        @objc func viewportResized(_ n: Notification) { fitColumn() }

        func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
            switch rows[row] {
            case .hunk: return Theme.lineHeight + 8
            case .comment(let c): return CommentRowView.height(for: c)
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

        /// Which diff line a click hit; in split view the half under the mouse decides.
        func lineIndex(row: Int, x: CGFloat?) -> Int? {
            guard row >= 0, row < rows.count else { return nil }
            switch rows[row] {
            case .line(let i): return i
            case .split(let l, let r):
                let half = (table.tableColumns[0].width / 2).rounded()
                if let x, x < half { return l ?? r }
                return r ?? l
            default: return nil
            }
        }

        // MARK: Text selection
        // Monospaced text makes point → (row, column) exact, so the table gets real character selection
        // without turning 100k rows into an NSTextView.

        struct TextPos: Comparable {
            var row: Int, col: Int
            static func < (a: TextPos, b: TextPos) -> Bool { (a.row, a.col) < (b.row, b.col) }
        }
        var selSide = 1                     // 0 = left (old) half in split view, 1 = right / unified
        var anchor: TextPos?
        var head: TextPos?

        var selection: (TextPos, TextPos)? {
            guard let a = anchor, let h = head, a != h else { return nil }
            return (min(a, h), max(a, h))
        }

        func textX(side: Int) -> CGFloat {
            layout == .unified ? numberWidth * 2 + 16 : (side == 0 ? 0 : (table.tableColumns[0].width / 2).rounded()) + numberWidth + 16
        }

        func line(row: Int, side: Int) -> Int? {
            guard row >= 0, row < rows.count else { return nil }
            switch rows[row] {
            case .line(let i): return i
            case .split(let l, let r): return side == 0 ? l : r
            default: return nil
            }
        }

        func text(row: Int, side: Int) -> [UInt16] {
            line(row: row, side: side).map { Array(Syntax.displayText(file.lines[$0].text).utf16) } ?? []
        }

        func pos(_ p: NSPoint) -> TextPos {
            var row = table.row(at: p)
            if row < 0 { row = p.y < 0 ? 0 : max(0, rows.count - 1) }
            let len = text(row: row, side: selSide).count
            let col = Int(((p.x - textX(side: selSide)) / Theme.charWidth).rounded())
            return TextPos(row: row, col: max(0, min(len, col)))
        }

        /// The identifier around a text position, as a column range (empty when there's none).
        func wordRange(at: TextPos, side: Int) -> Range<Int> {
            let u = text(row: at.row, side: side)
            func isWord(_ c: UInt16) -> Bool { (c >= 48 && c <= 57) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122) || c == 95 || c > 127 }
            var a = min(at.col, u.count), b = a
            if a < u.count, isWord(u[a]) || (a > 0 && isWord(u[a - 1])) {
                if a == u.count || !isWord(u[a]) { a -= 1; b = a }
                while a > 0, isWord(u[a - 1]) { a -= 1 }
                while b < u.count, isWord(u[b]) { b += 1 }
            }
            return a..<b
        }

        /// The identifier under a point, if it is one (not a number or punctuation).
        func word(at p: NSPoint) -> String? {
            let side = layout == .split && p.x < (table.tableColumns[0].width / 2).rounded() ? 0 : 1
            var at = pos(p)
            at.col = Int(((p.x - textX(side: side)) / Theme.charWidth).rounded(.down))
            let r = wordRange(at: at, side: side)
            guard !r.isEmpty else { return nil }
            let u = Array(text(row: at.row, side: side)[r])
            guard let first = u.first, !(first >= 48 && first <= 57) else { return nil }
            return String(utf16CodeUnits: u, count: u.count)
        }

        func mouseDown(_ p: NSPoint, clicks: Int, command: Bool = false) {
            selSide = layout == .split && p.x < (table.tableColumns[0].width / 2).rounded() ? 0 : 1
            if command, clicks == 1, let nav = parent.onNavigate, let w = word(at: p) {   // ⌘-click: go to definition
                var at = pos(p); at.col = Int(((p.x - textX(side: selSide)) / Theme.charWidth).rounded(.down))
                let r = wordRange(at: at, side: selSide)
                anchor = TextPos(row: at.row, col: r.lowerBound); head = TextPos(row: at.row, col: r.upperBound)
                redrawVisible()
                nav(w, .definition)
                return
            }
            let at = pos(p)
            switch clicks {
            case 2:   // word
                var r = wordRange(at: at, side: selSide)
                if r.isEmpty, r.lowerBound < text(row: at.row, side: selSide).count { r = r.lowerBound..<(r.lowerBound + 1) }
                anchor = TextPos(row: at.row, col: r.lowerBound); head = TextPos(row: at.row, col: r.upperBound)
            case 3...:  // line
                anchor = TextPos(row: at.row, col: 0); head = TextPos(row: at.row, col: text(row: at.row, side: selSide).count)
            default:
                anchor = at; head = at
            }
            redrawVisible()
        }

        func mouseDragged(_ p: NSPoint) {
            guard anchor != nil else { return }
            head = pos(p)
            redrawVisible()
        }

        func selectAll() {
            guard !rows.isEmpty else { return }
            anchor = TextPos(row: 0, col: 0)
            head = TextPos(row: rows.count - 1, col: text(row: rows.count - 1, side: selSide).count)
            redrawVisible()
        }

        func clearSelection() { anchor = nil; head = nil; redrawVisible() }

        /// Selected column range on a row/side, if any.
        func selectedColumns(row: Int, side: Int) -> Range<Int>? {
            guard side == selSide, let (s, e) = selection, row >= s.row, row <= e.row, line(row: row, side: side) != nil else { return nil }
            let len = text(row: row, side: side).count
            let a = row == s.row ? s.col : 0
            let b = row == e.row ? e.col : len + 1   // +1: a full-line selection shows its newline
            return a < b ? a..<b : nil
        }

        func redrawVisible() {
            guard let table else { return }
            let vis = table.rows(in: table.visibleRect)
            for r in vis.location..<NSMaxRange(vis) { table.view(atColumn: 0, row: r, makeIfNecessary: false)?.needsDisplay = true }
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
            if let i = lineIndex(row: row, x: p?.x) {
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

        /// Diff lines on the selected rows; in split view both halves of a row count.
        func selectedLines() -> Set<Int> {
            guard let (s, e) = selection else { return [] }
            var out = Set<Int>()
            for r in s.row...e.row { for side in [0, 1] { if let l = line(row: r, side: side) { out.insert(l) } } }
            return out
        }

        func copySelection() {
            guard let text = selectedText() else { return }
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
        }

        func selectedText() -> String? {
            guard let (s, e) = selection else { return nil }
            var out: [String] = []
            for r in s.row...e.row where line(row: r, side: selSide) != nil {
                let u = text(row: r, side: selSide)
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
                case .split(let a, let b): return [a, b].contains { $0.map { file.lines[$0].kind == .added || file.lines[$0].kind == .removed } ?? false }
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

enum Theme {
    static let font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
    static let smallFont = NSFont.monospacedDigitSystemFont(ofSize: 10.5, weight: .regular)
    static let lineHeight: CGFloat = 18
    static let maxDrawnChars = 1500   // ponytail: lines longer than this are truncated on screen (minified files); copy still gets full text
    static let charWidth: CGFloat = ("M" as NSString).size(withAttributes: [.font: font]).width

    static let addBG = NSColor(name: nil) { $0.isDark ? NSColor(srgbRed: 0.18, green: 0.35, blue: 0.22, alpha: 0.55) : NSColor(srgbRed: 0.85, green: 0.97, blue: 0.87, alpha: 1) }
    static let delBG = NSColor(name: nil) { $0.isDark ? NSColor(srgbRed: 0.42, green: 0.18, blue: 0.19, alpha: 0.55) : NSColor(srgbRed: 1.0, green: 0.90, blue: 0.90, alpha: 1) }
    static let addHL = NSColor(name: nil) { $0.isDark ? NSColor(srgbRed: 0.22, green: 0.55, blue: 0.3, alpha: 0.7) : NSColor(srgbRed: 0.65, green: 0.92, blue: 0.70, alpha: 1) }
    static let delHL = NSColor(name: nil) { $0.isDark ? NSColor(srgbRed: 0.7, green: 0.25, blue: 0.27, alpha: 0.7) : NSColor(srgbRed: 1.0, green: 0.75, blue: 0.76, alpha: 1) }
    static let gutterBG = NSColor(name: nil) { $0.isDark ? NSColor(white: 1, alpha: 0.03) : NSColor(white: 0, alpha: 0.025) }
    static let hunkBG = NSColor(name: nil) { $0.isDark ? NSColor(srgbRed: 0.2, green: 0.25, blue: 0.4, alpha: 0.45) : NSColor(srgbRed: 0.92, green: 0.94, blue: 1.0, alpha: 1) }
    static let emptyBG = NSColor(name: nil) { $0.isDark ? NSColor(white: 1, alpha: 0.025) : NSColor(white: 0, alpha: 0.035) }
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
            drawSide(l, index: i, x: 0, numbers: [l.oldLine, l.newLine], nw: nw, width: bounds.width, c: c)
        case .split(let a, let b):
            let half = (bounds.width / 2).rounded()
            if let a { let l = c.file.lines[a]; drawSide(l, index: a, x: 0, numbers: [l.oldLine], nw: nw, width: half, c: c, splitLeft: true) }
            else { Theme.emptyBG.setFill(); NSRect(x: 0, y: 0, width: half, height: bounds.height).fill() }
            if let b { let l = c.file.lines[b]; drawSide(l, index: b, x: half, numbers: [l.newLine], nw: nw, width: bounds.width - half, c: c, splitLeft: false) }
            else { Theme.emptyBG.setFill(); NSRect(x: half, y: 0, width: bounds.width - half, height: bounds.height).fill() }
            NSColor.separatorColor.setFill(); NSRect(x: half - 0.5, y: 0, width: 1, height: bounds.height).fill()
        default: break
        }
        if let h = c.hover, h.row == rowIndex, !h.cols.isEmpty {   // ⌘-hover link
            let x = c.textX(side: h.side) + CGFloat(h.cols.lowerBound) * Theme.charWidth
            NSColor.linkColor.setFill()
            NSRect(x: x, y: bounds.height - 2.5, width: CGFloat(h.cols.count) * Theme.charWidth, height: 1.2).fill()
        }
    }

    private func drawSide(_ l: DiffLine, index: Int, x: CGFloat, numbers: [Int?], nw: CGFloat, width: CGFloat, c: DiffTableView.Coordinator, splitLeft: Bool? = nil) {
        let h = bounds.height
        // In split view a context line shows its old number left and new number right.
        var nums = numbers
        if let left = splitLeft, l.kind == .context { nums = [left ? l.oldLine : l.newLine] }
        let bg: NSColor? = l.kind == .added ? Theme.addBG : l.kind == .removed ? Theme.delBG : nil
        if let bg { bg.setFill(); NSRect(x: x, y: 0, width: width, height: h).fill() }
        let gutterW = nw * CGFloat(nums.count)
        Theme.gutterBG.setFill(); NSRect(x: x, y: 0, width: gutterW, height: h).fill()

        let numAttrs: [NSAttributedString.Key: Any] = [.font: Theme.smallFont, .foregroundColor: NSColor.tertiaryLabelColor]
        for (k, n) in nums.enumerated() {
            guard let n else { continue }
            let s = String(n) as NSString
            let sz = s.size(withAttributes: numAttrs)
            s.draw(at: NSPoint(x: x + nw * CGFloat(k + 1) - sz.width - 6, y: (h - sz.height) / 2), withAttributes: numAttrs)
        }
        let marker = l.kind == .added ? "+" : l.kind == .removed ? "−" : ""
        let tx = x + gutterW + 4
        if let cols = c.selectedColumns(row: rowIndex, side: splitLeft == true ? 0 : 1) {
            let focused = window?.firstResponder === c.table && window?.isKeyWindow == true
            (focused ? NSColor.selectedTextBackgroundColor : NSColor.unemphasizedSelectedTextBackgroundColor).setFill()
            NSRect(x: tx + 12 + CGFloat(cols.lowerBound) * Theme.charWidth, y: 0,
                   width: CGFloat(cols.count) * Theme.charWidth, height: h).fill()
        }
        (marker as NSString).draw(at: NSPoint(x: tx, y: 1), withAttributes: [.font: Theme.font, .foregroundColor: l.kind == .added ? NSColor.systemGreen : NSColor.systemRed])

        let text = Syntax.displayText(l.text)
        let color: NSColor = l.kind == .meta ? .secondaryLabelColor : .labelColor
        let s = NSMutableAttributedString(string: text, attributes: [.font: Theme.font, .foregroundColor: color])
        if index < c.syntax.count {
            for sp in c.syntax[index] where Int(sp.start + sp.length) <= s.length {
                s.addAttribute(.foregroundColor, value: Syntax.color(sp.kind), range: NSRange(location: Int(sp.start), length: Int(sp.length)))
            }
        }
        if let r = c.highlights[index], NSMaxRange(r) <= s.length, !l.text.contains("\t") {
            s.addAttribute(.backgroundColor, value: l.kind == .added ? Theme.addHL : Theme.delHL, range: r)
        }
        NSGraphicsContext.saveGraphicsState()
        NSRect(x: x, y: 0, width: width, height: h).clip()
        s.draw(at: NSPoint(x: tx + 12, y: 1))
        NSGraphicsContext.restoreGraphicsState()
    }
}

final class HunkRowView: NSView {
    static let id = NSUserInterfaceItemIdentifier("hunk")
    private let label = NSTextField(labelWithString: "")
    private let buttons = NSStackView()
    private var handler: ((HunkAction) -> Void)?

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

    override func updateLayer() { layer?.backgroundColor = Theme.hunkBG.cgColor }

    func configure(header: Substring, actions: [HunkAction], handler: @escaping (HunkAction) -> Void) {
        label.stringValue = String(header)
        self.handler = handler
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

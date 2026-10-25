import SwiftUI

/// The lenses' report for a change from `a` to `b`, computed when the inputs change. Facts with a location open it.
struct ChangeOverview: View {
    var dir: String
    var a: Snapshot
    var b: Snapshot
    var changes: [ChangedPath]
    var key: String                       // recompute when this changes (head sha, file list…)
    var onOpen: (String, Int?) -> Void
    @State private var reports: [LensReport]?
    @State private var computedFor: String?
    @State private var width: CGFloat = 0

    var body: some View {
        Group {
            if let reports, computedFor == key {
                ScrollViewReader { proxy in
                    ScrollView {
                        content(reports, proxy)
                            .padding(.horizontal, 24).padding(.vertical, 20)
                            .frame(maxWidth: 1320, alignment: .leading)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
                    }
                }
            } else {
                VStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Reading the change…").font(.callout).foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .task(id: key) {
            guard computedFor != key else { return }
            let r = await ChangeLens.analyze(changes, a: a, b: b, in: dir)
            guard !Task.isCancelled else { return }
            reports = r; computedFor = key
        }
    }

    @ViewBuilder private func content(_ reports: [LensReport], _ proxy: ScrollViewProxy) -> some View {
        let main = reports.filter { $0.kind == .database || $0.kind == .code }
        let side = reports.filter { $0.kind != .database && $0.kind != .code }
        VStack(alignment: .leading, spacing: 28) {
            Summary(reports: reports) { id in withAnimation(.easeOut(duration: 0.25)) { proxy.scrollTo(id, anchor: .top) } }
            if reports.count == 1 {
                Text("Nothing structural beyond the files: no schema, dependency, configuration or code-definition changes.")
                    .font(.callout).foregroundStyle(.secondary)
            }
            if !main.isEmpty && !side.isEmpty && width > 1060 {
                HStack(alignment: .top, spacing: 28) {
                    VStack(alignment: .leading, spacing: 28) { ForEach(main) { section($0) } }.frame(maxWidth: .infinity)
                    VStack(alignment: .leading, spacing: 28) { ForEach(side) { section($0) } }.frame(width: 340)
                }
            } else {
                VStack(alignment: .leading, spacing: 28) { ForEach(main + side) { section($0) } }
            }
        }
    }

    private func section(_ r: LensReport) -> some View { LensSectionView(report: r, onOpen: onOpen).id(r.id) }
}

// MARK: - Shared look

extension LensFact.Kind {
    var color: Color {
        switch self {
        case .added: .green
        case .removed: .red
        case .changed: .orange
        case .renamed: .purple
        case .info: .secondary
        case .warning: .yellow
        }
    }
    var symbol: String {
        switch self {
        case .added: "plus.circle.fill"
        case .removed: "minus.circle.fill"
        case .changed: "pencil.circle.fill"
        case .renamed: "arrow.right.circle.fill"
        case .info: "circle"
        case .warning: "exclamationmark.triangle.fill"
        }
    }
    var label: String {
        switch self {
        case .added: "New"
        case .removed: "Removed"
        case .changed: "Modified"
        case .renamed: "Renamed"
        case .info: "Unchanged"
        case .warning: "Warning"
        }
    }
}

private extension LensReport.Kind {
    var tint: Color {
        switch self {
        case .files: .blue
        case .database: .orange
        case .dependencies: .brown
        case .configuration: .gray
        case .code: .indigo
        }
    }
}

/// The white-on-colour circle symbols macOS uses for added / removed / modified.
private struct StatusSymbol: View {
    var kind: LensFact.Kind
    var size: CGFloat = 12
    var body: some View {
        Image(systemName: kind.symbol)
            .symbolRenderingMode(.palette)
            .foregroundStyle(.white, kind.color)
            .font(.system(size: size))
            .frame(width: size + 2)
            .help(kind.label)
    }
}

/// A rounded group of rows, as in System Settings.
private struct Grouped<Content: View>: View {
    @ViewBuilder var content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: 0) { content }
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.primary.opacity(0.035), in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Color.primary.opacity(0.07)))
    }
}

/// Rows separated by inset hairlines.
private struct Rows<Item: Identifiable, Row: View>: View {
    var items: [Item]
    var inset: CGFloat = 12
    @ViewBuilder var row: (Item) -> Row
    var body: some View {
        ForEach(Array(items.enumerated()), id: \.element.id) { i, item in
            if i > 0 { Divider().padding(.leading, inset) }
            row(item)
        }
    }
}

private struct Subheading: View {
    var text: String
    var body: some View { Text(text).font(.subheadline.weight(.medium)).foregroundStyle(.secondary).padding(.leading, 2) }
}

/// "+2 −6 ~3", each in its colour; zeroes left out.
private struct Deltas: View {
    var added = 0, removed = 0, changed = 0
    var body: some View {
        HStack(spacing: 5) {
            if added > 0 { Text("+\(added)").foregroundStyle(.green) }
            if removed > 0 { Text("−\(removed)").foregroundStyle(.red) }
            if changed > 0 { Text("~\(changed)").foregroundStyle(.orange) }
        }
        .font(.caption.monospacedDigit())
    }
}

/// Hover highlight and click-to-open for anything with a location.
private struct Openable: ViewModifier {
    var path: String?
    var line: Int?
    var onOpen: (String, Int?) -> Void
    @State private var hover = false
    func body(content: Content) -> some View {
        content
            .background(hover && path != nil ? Color.primary.opacity(0.05) : .clear)
            .contentShape(Rectangle())
            .onHover { hover = $0 && path != nil }
            .onTapGesture { if let path { onOpen(path, line) } }
            .help(path.map { "Open \($0)\(line.map { ":\($0)" } ?? "")" } ?? "")
    }
}

private extension View {
    func openable(_ path: String?, _ line: Int?, _ onOpen: @escaping (String, Int?) -> Void) -> some View { modifier(Openable(path: path, line: line, onOpen: onOpen)) }
}

/// A small disclosure button: "▸ Definition".
private struct Disclosure: View {
    var title: String
    @Binding var open: Bool
    var body: some View {
        Button { withAnimation(.easeOut(duration: 0.15)) { open.toggle() } } label: {
            HStack(spacing: 4) {
                Image(systemName: "chevron.right").font(.system(size: 9, weight: .semibold)).rotationEffect(.degrees(open ? 90 : 0))
                Text(title)
            }
            .font(.caption).foregroundStyle(.secondary)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// A source excerpt, syntax highlighted, with lines tinted for what happened to them, cut at `maxLines`.
struct CodeBlock: View {
    var text: String
    var path: String                      // picks the language
    var kind: LensFact.Kind? = nil        // added → green, removed → red
    var maxLines = 12
    @State private var colored: [AttributedString] = []
    @State private var expanded = false

    private var lines: [String] {
        var l = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        while l.last?.trimmingCharacters(in: .whitespaces).isEmpty == true { l.removeLast() }
        while l.first?.trimmingCharacters(in: .whitespaces).isEmpty == true { l.removeFirst() }
        let indent = l.filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }.map { $0.prefix { $0 == " " || $0 == "\t" }.count }.min() ?? 0
        return l.map { String($0.dropFirst(min(indent, $0.prefix { $0 == " " || $0 == "\t" }.count))) }
    }

    var body: some View {
        let all = lines
        let shown = expanded ? all.count : min(all.count, maxLines)
        let tint: Color? = kind == .added ? .green : kind == .removed ? .red : nil
        let mark = kind == .added ? "+" : kind == .removed ? "−" : nil
        VStack(alignment: .leading, spacing: 0) {
            ScrollView(.horizontal, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 1) {
                    ForEach(0..<shown, id: \.self) { i in
                        HStack(spacing: 8) {
                            if let mark { Text(mark).foregroundStyle(.secondary).frame(width: 8) }
                            Text(i < colored.count ? colored[i] : AttributedString(all[i])).fixedSize()
                        }
                    }
                }
                .font(.system(size: 11, design: .monospaced))
                .textSelection(.enabled)
                .padding(.vertical, 7).padding(.horizontal, 9)
            }
            if all.count > maxLines {
                Divider()
                Button(expanded ? "Show Less" : "Show All \(all.count) Lines") { withAnimation(.easeOut(duration: 0.15)) { expanded.toggle() } }
                    .buttonStyle(.link).font(.caption)
                    .padding(.horizontal, 9).padding(.vertical, 4)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background((tint ?? .clear).opacity(0.07))
        .background(Color(nsColor: .textBackgroundColor))
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color.primary.opacity(0.08)))
        .task(id: text) {
            let l = all, p = path
            colored = await Task.detached(priority: .userInitiated) { BlameView.highlight(l, path: p) }.value
        }
    }
}

/// Lays children out left to right, wrapping to new rows.
private struct Flow: Layout {
    var spacing: CGFloat = 5
    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = arrange(proposal.width ?? .infinity, subviews)
        return CGSize(width: rows.map(\.width).max() ?? 0, height: rows.last.map { $0.y + $0.height } ?? 0)
    }
    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for row in arrange(bounds.width, subviews) {
            var x = bounds.minX
            for i in row.items {
                let s = subviews[i].sizeThatFits(.unspecified)
                subviews[i].place(at: CGPoint(x: x, y: bounds.minY + row.y), proposal: .unspecified)
                x += s.width + spacing
            }
        }
    }
    private func arrange(_ width: CGFloat, _ subviews: Subviews) -> [(items: [Int], y: CGFloat, width: CGFloat, height: CGFloat)] {
        var rows: [(items: [Int], y: CGFloat, width: CGFloat, height: CGFloat)] = []
        var cur: [Int] = [], x: CGFloat = 0, y: CGFloat = 0, h: CGFloat = 0
        for (i, v) in subviews.enumerated() {
            let s = v.sizeThatFits(.unspecified)
            if !cur.isEmpty && x + s.width > width {
                rows.append((cur, y, x - spacing, h)); y += h + spacing; cur = []; x = 0; h = 0
            }
            cur.append(i); x += s.width + spacing; h = max(h, s.height)
        }
        if !cur.isEmpty { rows.append((cur, y, x - spacing, h)) }
        return rows
    }
}

/// The first `limit` items as rows in a group, and a button for the rest.
private struct CappedRows<Item: Identifiable, Row: View>: View {
    var items: [Item]
    var limit: Int
    var noun: String
    var inset: CGFloat = 12
    @ViewBuilder var row: (Item) -> Row
    @State private var all = false
    var body: some View {
        Rows(items: all ? items : Array(items.prefix(limit)), inset: inset, row: row)
        if items.count > limit {
            Divider().padding(.leading, inset)
            Button(all ? "Show Fewer" : "Show \(items.count - limit) More \(noun)") { withAnimation(.easeOut(duration: 0.15)) { all.toggle() } }
                .buttonStyle(.link).font(.caption)
                .padding(.horizontal, 12).padding(.vertical, 7)
        }
    }
}

// MARK: - Summary

/// One tile per lens: its headline number and what moved; a tile scrolls to its section.
private struct Summary: View {
    var reports: [LensReport]
    var onSelect: (String) -> Void
    var body: some View {
        HStack(spacing: 10) {
            ForEach(reports) { r in SummaryTile(report: r).onTapGesture { onSelect(r.id) } }
        }
    }
}

private struct SummaryTile: View {
    var report: LensReport
    @State private var hover = false
    var body: some View {
        let m = report.metric
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 7) {
                Image(systemName: report.icon).font(.system(size: 10, weight: .medium)).foregroundStyle(.white)
                    .frame(width: 20, height: 20)
                    .background(report.kind.tint.gradient, in: RoundedRectangle(cornerRadius: 5))
                Text(report.title).font(.callout).foregroundStyle(.secondary).lineLimit(1)
                Spacer(minLength: 0)
                Image(systemName: "chevron.right").font(.system(size: 9, weight: .semibold)).foregroundStyle(.tertiary).opacity(hover ? 1 : 0)
            }
            HStack(alignment: .firstTextBaseline, spacing: 5) {
                Text("\(m.value)").font(.system(size: 24, weight: .semibold, design: .rounded).monospacedDigit())
                Text(m.unit).font(.callout).foregroundStyle(.secondary).lineLimit(1)
            }
            Deltas(added: m.added, removed: m.removed, changed: m.changed).frame(height: 13, alignment: .leading)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.primary.opacity(hover ? 0.06 : 0.035), in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Color.primary.opacity(0.07)))
        .contentShape(Rectangle())
        .onHover { hover = $0 }
        .help(report.summary)
    }
}

// MARK: - Sections

private struct LensSectionView: View {
    var report: LensReport
    var onOpen: (String, Int?) -> Void
    @State private var collapsed = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Button { withAnimation(.easeOut(duration: 0.15)) { collapsed.toggle() } } label: {
                HStack(spacing: 6) {
                    Text(report.title).font(.title3.weight(.semibold))
                    Image(systemName: "chevron.down").font(.system(size: 10, weight: .semibold)).foregroundStyle(.tertiary)
                        .rotationEffect(.degrees(collapsed ? -90 : 0))
                    Spacer()
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if !collapsed {
                switch report.kind {
                case .files: FilesBody(report: report)
                case .database: DatabaseBody(groups: report.database, onOpen: onOpen)
                case .code: CodeBody(report: report, onOpen: onOpen)
                case .dependencies: DependencyBody(report: report, onOpen: onOpen)
                case .configuration: ConfigBody(report: report, onOpen: onOpen)
                }
            }
        }
    }
}

// MARK: - Files

private struct FilesBody: View {
    var report: LensReport
    static let colors: [String: Color] = ["Code": .blue, "Tests": .teal, "SQL": .orange, "Dependencies": .brown, "Config": .gray,
                                          "Build & CI": .indigo, "Docs": .mint, "Assets": .pink]
    var body: some View {
        let facts = report.sections.first?.facts ?? []
        let total = max(1, facts.reduce(0) { $0 + ($1.count ?? 0) })
        Grouped {
            VStack(alignment: .leading, spacing: 6) {
                Text(report.summary).font(.callout).foregroundStyle(.secondary)
                // A storage-style bar: how the change splits by kind of file.
                GeometryReader { g in
                    HStack(spacing: 1.5) {
                        ForEach(facts) { f in
                            Rectangle().fill(Self.colors[f.title, default: .gray])
                                .frame(width: max(2, (g.size.width - CGFloat(facts.count - 1) * 1.5) * CGFloat(f.count ?? 0) / CGFloat(total)))
                        }
                    }
                }
                .frame(height: 6)
                .clipShape(Capsule())
            }
            .padding(12)
            Divider()
            Rows(items: facts, inset: 30) { f in
                HStack(spacing: 8) {
                    Circle().fill(Self.colors[f.title, default: .gray]).frame(width: 8, height: 8).padding(.leading, 2)
                    Text(f.title)
                    Spacer()
                    Text(f.detail?.split(separator: "·").dropFirst().joined(separator: "·").trimmingCharacters(in: .whitespaces) ?? "")
                        .font(.caption).foregroundStyle(.secondary)
                    Text("\(f.count ?? 0)").monospacedDigit().foregroundStyle(.secondary).frame(minWidth: 24, alignment: .trailing)
                }
                .padding(.horizontal, 12).padding(.vertical, 6)
            }
        }
    }
}

// MARK: - Database

private struct DatabaseBody: View {
    var groups: [DBGroup]
    var onOpen: (String, Int?) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            ForEach(groups) { g in
                VStack(alignment: .leading, spacing: 16) {
                    Label(g.root, systemImage: "folder").font(.callout).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
                    if !g.warnings.isEmpty {
                        Grouped { Rows(items: g.warnings) { WarningRow(fact: $0, onOpen: onOpen) } }
                    }
                    if !g.tables.isEmpty {
                        VStack(alignment: .leading, spacing: 8) {
                            Subheading(text: "Tables")
                            LazyVGrid(columns: [GridItem(.adaptive(minimum: 270, maximum: 520), spacing: 12, alignment: .top)], alignment: .leading, spacing: 12) {
                                ForEach(g.tables) { TableBox(table: $0, onOpen: onOpen) }
                            }
                        }
                    }
                    if !g.objects.isEmpty {
                        VStack(alignment: .leading, spacing: 8) {
                            Subheading(text: "Functions, Views and Types")
                            Grouped { Rows(items: g.objects, inset: 34) { ObjectRow(object: $0, onOpen: onOpen) } }
                        }
                    }
                    if !g.data.isEmpty {
                        VStack(alignment: .leading, spacing: 8) {
                            Subheading(text: "Data")
                            Grouped { Rows(items: g.data, inset: 34) { DataRow(data: $0, onOpen: onOpen) } }
                        }
                    }
                }
            }
        }
    }
}

/// A table with its columns, what happened to each, and its keys, indexes and triggers.
private struct TableBox: View {
    var table: DBTable
    var onOpen: (String, Int?) -> Void
    @State private var showAll = false
    @State private var showSQL = false

    private enum Row: Identifiable {
        case column(DBColumn)
        case gap(Int, Int)
        var id: String { switch self { case .column(let c): "c " + c.name; case .gap(let at, _): "g \(at)" } }
    }

    /// In a changed table, runs of three or more untouched columns fold into one row.
    private var rows: [Row] {
        if showAll || table.kind != .changed { return table.columns.map { .column($0) } }
        var out: [Row] = [], run: [DBColumn] = []
        func flush() {
            if run.count >= 3 { out.append(.gap(out.count, run.count)) } else { out += run.map { .column($0) } }
            run = []
        }
        for c in table.columns {
            if c.kind == .info { run.append(c) } else { flush(); out.append(.column(c)) }
        }
        flush()
        return out
    }

    var body: some View {
        let r = rows
        let limit = table.kind == .changed || showAll ? Int.max : 12
        Grouped {
            HStack(spacing: 8) {
                Image(systemName: "tablecells").foregroundStyle(.secondary)
                VStack(alignment: .leading, spacing: 0) {
                    Text(table.name).font(.system(size: 13, weight: .semibold)).lineLimit(1).truncationMode(.middle)
                    if let old = table.oldName { Text("Renamed from \(old)").font(.caption).foregroundStyle(.secondary) }
                }
                Spacer(minLength: 6)
                HStack(spacing: 4) {
                    StatusSymbol(kind: table.kind, size: 11)
                    Text(table.kind == .removed ? "Dropped" : table.kind.label).font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 8)
            .openable(table.path, table.line, onOpen)
            Divider()
            VStack(spacing: 0) {
                ForEach(r.prefix(limit)) { row in
                    switch row {
                    case .column(let c): ColumnRow(column: c, tableKind: table.kind).openable(c.path, c.line, onOpen)
                    case .gap(_, let n):
                        Button { withAnimation(.easeOut(duration: 0.15)) { showAll = true } } label: {
                            Text("\(n) unchanged columns").font(.caption).foregroundStyle(.tertiary)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.leading, 40).padding(.vertical, 3)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
                if r.count > limit {
                    Button("Show \(r.count - limit) More Columns") { withAnimation(.easeOut(duration: 0.15)) { showAll = true } }
                        .buttonStyle(.link).font(.caption)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.leading, 40).padding(.vertical, 3)
                }
            }
            .padding(.vertical, 5)
            if !table.extras.isEmpty {
                Divider().padding(.leading, 12)
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(table.extras) { ExtraRow(fact: $0).openable($0.path, $0.line, onOpen) }
                }
                .padding(.vertical, 5)
            }
            if let sql = table.sql {
                Divider().padding(.leading, 12)
                VStack(alignment: .leading, spacing: 6) {
                    Disclosure(title: table.kind == .removed ? "Definition before this change" : "Definition", open: $showSQL)
                    if showSQL { CodeBlock(text: sql, path: "x.sql", maxLines: 30) }
                }
                .padding(.horizontal, 12).padding(.vertical, 7)
            }
        }
    }
}

private struct ColumnRow: View {
    var column: DBColumn
    var tableKind: LensFact.Kind

    var body: some View {
        let c = column
        // In a new or dropped table every column shares the table's fate; only columns that changed on their own get a mark.
        let own = tableKind == .changed && c.kind != .info
        let quiet = tableKind == .removed || (tableKind == .changed && !own)
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 6) {
                ZStack { if own { StatusSymbol(kind: c.kind, size: 10) } }.frame(width: 12)
                Image(systemName: "key.fill").font(.system(size: 8.5)).foregroundStyle(.secondary).opacity(c.primary ? 1 : 0).frame(width: 10)
                    .help("Primary key")
                Text(c.name).font(.system(size: 12)).foregroundStyle(quiet ? .secondary : .primary).lineLimit(1)
                if let ref = c.references {
                    Image(systemName: "link").font(.system(size: 9)).foregroundStyle(.tertiary).help("References \(ref)")
                }
                Spacer(minLength: 8)
                Group {
                    if let old = c.oldType { Text("\(old) → \(c.type)") } else { Text(c.type) }
                }
                .font(.system(size: 11, design: .monospaced)).foregroundStyle(.secondary).lineLimit(1)
                .help(c.notNull ? "NOT NULL" : "Nullable")
            }
            let notes = own && c.kind == .added
                ? [c.notNull ? "not null" : nil, c.defaultValue.map { "default \($0)" }, c.references.map { "references \($0)" }].compactMap { $0 }
                : c.notes.filter { !$0.hasPrefix("type ") }
            if !notes.isEmpty {
                Text(notes.joined(separator: ", ")).font(.caption).foregroundStyle(.secondary).padding(.leading, 34).lineLimit(2)
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 2.5)
    }
}

/// A key, constraint, index or trigger on a table.
private struct ExtraRow: View {
    var fact: LensFact
    var body: some View {
        let icon = switch fact.tag {
        case "index": "list.bullet"
        case "trigger": "bolt"
        case "key": "key"
        default: "checkmark.shield"
        }
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            StatusSymbol(kind: fact.kind, size: 10).frame(width: 12)
            Image(systemName: icon).font(.system(size: 9)).foregroundStyle(.secondary).frame(width: 10)
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 5) {
                    Text(fact.title).font(.system(size: 12)).lineLimit(1).truncationMode(.middle)
                    Text(fact.tag ?? "").font(.caption).foregroundStyle(.tertiary)
                }
                if let v = fact.after ?? fact.before, fact.tag != "trigger" {
                    Text(fact.before.map { b in fact.after != nil ? "\(b) → \(v)" : v } ?? v)
                        .font(.system(size: 10.5, design: .monospaced)).foregroundStyle(.secondary).lineLimit(2).truncationMode(.middle)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12).padding(.vertical, 2.5)
    }
}

private struct ObjectRow: View {
    var object: DBObject
    var onOpen: (String, Int?) -> Void
    @State private var open = false

    var body: some View {
        let o = object
        let code = o.type == "function" || o.type == "view" || o.type == "trigger"
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                StatusSymbol(kind: o.kind)
                Text(o.name).font(.system(size: 12.5, weight: .medium)).lineLimit(1)
                Text(o.type).font(.caption).foregroundStyle(.tertiary)
                Spacer()
                Text(o.kind == .removed ? "Dropped" : o.kind == .changed && code ? "Redefined" : o.kind.label).font(.caption).foregroundStyle(.secondary)
            }
            .openable(o.path, o.line, onOpen)
            if !code, let a = o.after ?? o.before {
                Text(o.before.map { b in o.after != nil ? "\(b)  →  \(a)" : a } ?? a)
                    .font(.system(size: 11, design: .monospaced)).foregroundStyle(.secondary).lineLimit(2).padding(.leading, 22)
            }
            if code && (o.before != nil || o.after != nil) {
                VStack(alignment: .leading, spacing: 6) {
                    Disclosure(title: "Definition", open: $open)
                    if open {
                        if o.kind == .changed, let b = o.before, let a = o.after {
                            CodeBlock(text: b, path: "x.sql", kind: .removed, maxLines: 10)
                            CodeBlock(text: a, path: "x.sql", kind: .added, maxLines: 10)
                        } else if let t = o.after ?? o.before {
                            CodeBlock(text: t, path: "x.sql", maxLines: 12)
                        }
                    }
                }
                .padding(.leading, 22)
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
    }
}

private struct DataRow: View {
    var data: DBData
    var onOpen: (String, Int?) -> Void
    @State private var open = false

    var body: some View {
        let d = data
        let what = d.kind == .added ? "added" : d.kind == .removed ? "removed" : "changed"
        let pairs = d.kind == .changed ? Array(zip(d.before, d.after)) : []
        let singles = d.kind == .added ? d.after : d.before
        let n = d.kind == .changed ? pairs.count : singles.count
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                StatusSymbol(kind: d.kind)
                (Text(d.verb.capitalized + (d.verb == "INSERT" ? " into " : d.verb == "DELETE" ? " from " : " ")).foregroundColor(.secondary)
                    + Text(d.table).fontWeight(.medium))
                    .font(.system(size: 12.5))
                Spacer()
                Text("\(plural(d.count, "statement")) \(what)").font(.caption).foregroundStyle(.secondary)
                Text((d.path as NSString).lastPathComponent).font(.caption).foregroundStyle(.tertiary).lineLimit(1).truncationMode(.middle)
            }
            .openable(d.path, d.line, onOpen)
            VStack(alignment: .leading, spacing: 6) {
                Disclosure(title: n == 1 ? "Statement" : "\(n) statements", open: $open)
                if open {
                    ForEach(0..<n, id: \.self) { i in
                        if d.kind == .changed {
                            CodeBlock(text: pairs[i].0, path: "x.sql", kind: .removed, maxLines: 10)
                            CodeBlock(text: pairs[i].1, path: "x.sql", kind: .added, maxLines: 10)
                        } else {
                            CodeBlock(text: singles[i], path: "x.sql", kind: d.kind, maxLines: 10)
                        }
                    }
                }
            }
            .padding(.leading, 22)
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
    }
}

private struct WarningRow: View {
    var fact: LensFact
    var onOpen: (String, Int?) -> Void
    @State private var show = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: "exclamationmark.triangle.fill").symbolRenderingMode(.multicolor).font(.system(size: 14))
                VStack(alignment: .leading, spacing: 2) {
                    Text("\(fact.title) not replayed").font(.system(size: 12.5, weight: .medium))
                    Text("Loupe doesn't run procedural SQL, so the schema shown doesn't include \(fact.count ?? 1 == 1 ? "its" : "their") effect.")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                if fact.after != nil { Button(show ? "Hide" : "Show") { withAnimation(.easeOut(duration: 0.15)) { show.toggle() } }.controlSize(.small) }
                if let p = fact.path { Button("Open") { onOpen(p, fact.line) }.controlSize(.small) }
            }
            if show, let s = fact.after { CodeBlock(text: s, path: "x.sql", maxLines: 14).padding(.leading, 24) }
        }
        .padding(.horizontal, 12).padding(.vertical, 9)
    }
}

// MARK: - Code

private struct CodeBody: View {
    var report: LensReport
    var onOpen: (String, Int?) -> Void

    /// Files whose interface changed (definitions added, removed, re-signed) first; body-only edits after.
    private var files: [LensSection] {
        func weight(_ s: LensSection) -> Int { s.facts.filter { $0.kind != .changed || $0.after != nil }.count * 100 + s.facts.count }
        return report.sections.sorted { weight($0) > weight($1) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(report.summary).font(.callout).foregroundStyle(.secondary)
            Grouped { CappedRows(items: files, limit: 10, noun: "Files") { FileDefinitions(section: $0, onOpen: onOpen) } }
        }
    }
}

private struct FileDefinitions: View {
    var section: LensSection
    var onOpen: (String, Int?) -> Void

    var body: some View {
        let path = section.title
        let api = section.facts.filter { $0.kind != .changed || $0.after != nil }
        let bodies = section.facts.filter { $0.kind == .changed && $0.after == nil }
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "doc.text").foregroundStyle(.secondary)
                Text((path as NSString).lastPathComponent).font(.system(size: 12.5, weight: .medium))
                Text((path as NSString).deletingLastPathComponent).font(.caption).foregroundStyle(.tertiary).lineLimit(1).truncationMode(.head)
                Spacer()
                Deltas(added: section.facts.filter { $0.kind == .added }.count, removed: section.facts.filter { $0.kind == .removed }.count,
                       changed: section.facts.filter { $0.kind == .changed }.count)
            }
            .openable(path, nil, onOpen)
            ForEach(api.prefix(12)) { f in
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        StatusSymbol(kind: f.kind, size: 11)
                        Text(f.title).font(.system(size: 12, weight: .medium))
                        if let t = f.tag { Text(t).font(.caption).foregroundStyle(.tertiary) }
                        Spacer()
                        Text(f.kind == .changed ? "Signature changed" : f.kind.label).font(.caption).foregroundStyle(.secondary)
                    }
                    .openable(f.path, f.line, onOpen)
                    Group {
                        if f.kind == .changed, let b = f.before, let a = f.after {
                            CodeBlock(text: b, path: path, kind: .removed, maxLines: 4)
                            CodeBlock(text: a, path: path, kind: .added, maxLines: 4)
                        } else if let s = f.after ?? f.before {
                            CodeBlock(text: s, path: path, maxLines: 4)
                        }
                    }
                    .padding(.leading, 19)
                }
                .padding(.leading, 20)
            }
            if api.count > 12 { Text("\(api.count - 12) more").font(.caption).foregroundStyle(.secondary).padding(.leading, 40) }
            if !bodies.isEmpty {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("Body changed").font(.caption).foregroundStyle(.secondary).fixedSize()
                    Flow { ForEach(bodies) { BodyChip(fact: $0, onOpen: onOpen) } }
                }
                .padding(.leading, 20)
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
    }
}

private struct BodyChip: View {
    var fact: LensFact
    var onOpen: (String, Int?) -> Void
    @State private var hover = false
    var body: some View {
        Text(fact.title)
            .font(.system(size: 11.5))
            .padding(.horizontal, 7).padding(.vertical, 2)
            .background(Color.primary.opacity(hover ? 0.12 : 0.06), in: Capsule())
            .onHover { hover = $0 }
            .onTapGesture { if let p = fact.path { onOpen(p, fact.line) } }
            .help("\(fact.tag ?? "") \(fact.title)")
    }
}

// MARK: - Dependencies and configuration

private struct DependencyBody: View {
    var report: LensReport
    var onOpen: (String, Int?) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            ForEach(report.sections) { s in
                VStack(alignment: .leading, spacing: 6) {
                    Subheading(text: s.title)
                    Grouped {
                        CappedRows(items: s.facts, limit: 10, noun: "Packages", inset: 32) { f in
                            HStack(spacing: 8) {
                                StatusSymbol(kind: f.kind)
                                Text(f.title).lineLimit(1).truncationMode(.middle)
                                Spacer(minLength: 6)
                                Text(f.kind == .changed ? "\(f.before ?? "") → \(f.after ?? "")" : f.after ?? f.before ?? "")
                                    .font(.system(size: 11, design: .monospaced)).foregroundStyle(.secondary).lineLimit(1)
                                if let t = f.tag { Text(t).font(.caption).foregroundStyle(t == "major" || t == "downgrade" ? .orange : .secondary) }
                            }
                            .padding(.horizontal, 10).padding(.vertical, 6)
                            .openable(f.path, f.line, onOpen)
                        }
                    }
                }
            }
        }
    }
}

private struct ConfigBody: View {
    var report: LensReport
    var onOpen: (String, Int?) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            ForEach(report.sections) { s in
                VStack(alignment: .leading, spacing: 6) {
                    Subheading(text: s.title)
                    Grouped {
                        CappedRows(items: s.facts, limit: 10, noun: "Keys", inset: 32) { f in ConfigRow(fact: f).openable(f.path, f.line, onOpen) }
                    }
                }
            }
        }
    }
}

private struct ConfigRow: View {
    var fact: LensFact
    var body: some View {
        let f = fact
        let parts = f.title.split(separator: ".")
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 8) {
                StatusSymbol(kind: f.kind)
                (Text(parts.dropLast().map { $0 + "." }.joined()).foregroundColor(.secondary) + Text(parts.last.map(String.init) ?? f.title))
                    .font(.system(size: 11.5, design: .monospaced)).lineLimit(1).truncationMode(.head)
                Spacer(minLength: 4)
                if let t = f.tag { Text(t).font(.caption).foregroundStyle(.secondary) }
            }
            if f.before != nil || f.after != nil {
                Text(f.kind == .changed ? "\(f.before ?? "") → \(f.after ?? "")" : f.after ?? f.before ?? "")
                    .font(.system(size: 11, design: .monospaced)).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                    .padding(.leading, 22)
            } else if f.tag == nil, let d = f.detail {
                Text(d).font(.caption).foregroundStyle(.secondary).padding(.leading, 22)
            }
        }
        .padding(.horizontal, 10).padding(.vertical, 6)
    }
}

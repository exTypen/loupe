import SwiftUI

/// The lenses' report for a change from `a` to `b`, computed when the inputs change. Facts with a location open it.
struct ChangeOverview: View {
    var dir: String
    var a: Snapshot
    var b: Snapshot
    var changes: [ChangedPath]
    var key: String                       // recompute when this changes (head sha, file list…)
    /// The branch the change goes into, when its tip is further on than `a`: a pull request's base branch.
    var target: (ref: String, name: String)? = nil
    var onOpen: (String, Int?) -> Void
    @State private var reports: [LensReport]?
    @State private var branch: BranchStatus?
    @State private var computedFor: String?

    var body: some View {
        Group {
            if let reports, computedFor == key {
                ScrollViewReader { proxy in
                    ScrollView {
                        content(reports, proxy)
                            .padding(.horizontal, 24).padding(.vertical, 20)
                            .frame(maxWidth: .infinity, alignment: .leading)
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
            async let r = ChangeLens.analyze(changes, a: a, b: b, upstream: target?.ref, in: dir)
            var status: BranchStatus?
            if let target, case .rev(let head) = b {
                let paths = Array(Set(changes.flatMap { [$0.old, $0.new].compactMap { $0 } }))
                status = await BranchLens.status(head: head, upstream: target.ref, target: target.name, paths: paths, in: dir)
            }
            let reports = await r
            guard !Task.isCancelled else { return }
            self.reports = reports; branch = status; computedFor = key
        }
    }

    @ViewBuilder private func content(_ reports: [LensReport], _ proxy: ScrollViewProxy) -> some View {
        // Not lazy: a handful of very tall sections whose heights differ from the estimates kept LazyVStack
        // re-placing them forever while scrolling (main thread pinned at 100%), and laziness saves nothing at this count.
        VStack(alignment: .leading, spacing: 28) {
            VStack(alignment: .leading, spacing: 10) {
                Summary(reports: reports, onOpen: onOpen) { id in withAnimation(.easeOut(duration: 0.25)) { proxy.scrollTo(id, anchor: .top) } }
                if let branch { BranchRow(status: branch, onOpen: onOpen) }
            }
            if !reports.contains(where: { $0.kind != .files && $0.kind != .followUps }) {
                Text("Nothing structural beyond the files: no schema, dependency or configuration changes.")
                    .font(.callout).foregroundStyle(.secondary)
            }
            // The files lens lives in its summary tile; the others get a section each.
            ForEach(reports.filter { $0.kind != .files }) { section($0) }
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
        case .followUps: .purple
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
/// Highlighted lines of small snippets, so one that a lazy stack drops and recreates isn't highlighted again.
/// Bounded: a few hundred snippets, a few MB.
enum HighlightCache {
    private final class Lines { let value: [AttributedString]; init(_ v: [AttributedString]) { value = v } }
    private static let cache: NSCache<NSString, Lines> = {
        let c = NSCache<NSString, Lines>(); c.countLimit = 400; c.totalCostLimit = 4 << 20; return c
    }()
    private static func key(_ lines: [String], _ path: String) -> NSString { (path + "\u{0}" + lines.joined(separator: "\n")) as NSString }

    static func cached(_ lines: [String], path: String) -> [AttributedString]? { cache.object(forKey: key(lines, path))?.value }

    static func highlight(_ lines: [String], path: String) async -> [AttributedString] {
        if let c = cached(lines, path: path) { return c }
        let out = await Task.detached(priority: .userInitiated) { SnippetHighlight.lines(lines, path: path) }.value
        cache.setObject(Lines(out), forKey: key(lines, path), cost: lines.reduce(0) { $0 + $1.utf16.count } * 8)
        return out
    }
}

struct CodeBlock: View {
    var text: String
    var path: String                      // picks the language
    var kind: LensFact.Kind? = nil        // added → green, removed → red
    var maxLines = 12
    private let all: [String]
    @State private var colored: [AttributedString]
    @State private var expanded = false

    init(text: String, path: String, kind: LensFact.Kind? = nil, maxLines: Int = 12) {
        self.text = text; self.path = path; self.kind = kind; self.maxLines = maxLines
        var l = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        while l.last?.trimmingCharacters(in: .whitespaces).isEmpty == true { l.removeLast() }
        while l.first?.trimmingCharacters(in: .whitespaces).isEmpty == true { l.removeFirst() }
        let indent = l.filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }.map { $0.prefix { $0 == " " || $0 == "\t" }.count }.min() ?? 0
        all = l.map { String($0.dropFirst(min(indent, $0.prefix { $0 == " " || $0 == "\t" }.count))) }
        _colored = State(initialValue: HighlightCache.cached(all, path: path) ?? [])
    }

    var body: some View {
        let all = self.all
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
                .font(.editor(11))
                .foregroundStyle(Color(nsColor: Theme.foreground))
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
        // Shaped backgrounds rather than a clip: no mask layer over the scroll view inside.
        .background((tint ?? .clear).opacity(0.07), in: RoundedRectangle(cornerRadius: 6))
        .background(Color(nsColor: Theme.background), in: RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color.primary.opacity(0.08)))
        .task(id: text) {
            guard colored.isEmpty else { return }
            colored = await HighlightCache.highlight(all, path: path)
        }
    }
}

/// Two versions of a short text (a constraint, a function) as one diff: lines matched ignoring their indentation, so
/// re-nesting doesn't paint everything; the new version's lines shown, the old one's where they're gone.
struct LineDiff: View {
    let rows: [(mark: Character, text: String)]
    @State private var colored: [AttributedString]

    init(before: String, after: String) {
        let a = before.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        let b = after.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        let ka = a.map { $0.trimmingCharacters(in: .whitespaces) }, kb = b.map { $0.trimmingCharacters(in: .whitespaces) }
        // Longest common subsequence over the trimmed lines; these texts are a few dozen lines.
        var l = Array(repeating: Array(repeating: 0, count: kb.count + 1), count: ka.count + 1)
        for i in stride(from: ka.count - 1, through: 0, by: -1) {
            for j in stride(from: kb.count - 1, through: 0, by: -1) {
                l[i][j] = ka[i] == kb[j] ? l[i + 1][j + 1] + 1 : max(l[i + 1][j], l[i][j + 1])
            }
        }
        var out: [(Character, String)] = [], i = 0, j = 0
        while i < ka.count || j < kb.count {
            if i < ka.count, j < kb.count, ka[i] == kb[j] { out.append((" ", b[j])); i += 1; j += 1 }
            else if j < kb.count, i == ka.count || l[i][j + 1] >= l[i + 1][j] { out.append(("+", b[j])); j += 1 }
            else { out.append(("-", a[i])); i += 1 }
        }
        rows = out
        _colored = State(initialValue: HighlightCache.cached(out.map(\.1), path: "x.sql") ?? [])
    }

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(rows.enumerated()), id: \.offset) { k, r in
                    HStack(spacing: 6) {
                        Text(r.mark == "+" ? "+" : r.mark == "-" ? "−" : " ").foregroundStyle(r.mark == "+" ? .green : .red).frame(width: 8)
                        Text(k < colored.count ? colored[k] : AttributedString(r.text)).fixedSize()
                    }
                    .padding(.horizontal, 9).padding(.vertical, 0.5)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(r.mark == "+" ? Color(nsColor: Theme.addBG) : r.mark == "-" ? Color(nsColor: Theme.delBG) : .clear)
                }
            }
            .font(.editor(11))
            .foregroundStyle(Color(nsColor: Theme.foreground))
            .textSelection(.enabled)
            .padding(.vertical, 6)
        }
        .background(Color(nsColor: Theme.background), in: RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color.primary.opacity(0.08)))
        .task {
            guard colored.isEmpty else { return }
            colored = await HighlightCache.highlight(rows.map(\.text), path: "x.sql")
        }
    }
}

/// Cards in as many columns as fit, each one under the currently shortest column.
private struct Masonry: Layout {
    var minWidth: CGFloat = 300
    var maxWidth: CGFloat = 640           // a few cards on a wide window stay readable instead of stretching
    var spacing: CGFloat = 12

    /// The frames sizeThatFits measured, reused by the placeSubviews that follows instead of measuring every card again.
    /// Sizing always measures afresh, so a card that grew is never placed from stale heights.
    struct Cache { var width: CGFloat = -1; var frames: [CGRect] = [] }
    func makeCache(subviews: Subviews) -> Cache { Cache() }

    private func frames(_ width: CGFloat, _ subviews: Subviews, _ cache: inout Cache, reuse: Bool) -> [CGRect] {
        if reuse, cache.width == width, cache.frames.count == subviews.count { return cache.frames }
        // As many columns as fit, but no more than there are cards; a full row never reaches maxWidth.
        let n = max(1, min(subviews.count, Int((width + spacing) / (minWidth + spacing))))
        let w = min(maxWidth, (width - CGFloat(n - 1) * spacing) / CGFloat(n))
        var heights = [CGFloat](repeating: 0, count: n)
        let out = subviews.map { v in
            let c = heights.firstIndex(of: heights.min()!)!
            let h = v.sizeThatFits(ProposedViewSize(width: w, height: nil)).height
            defer { heights[c] += h + spacing }
            return CGRect(x: CGFloat(c) * (w + spacing), y: heights[c], width: w, height: h)
        }
        cache = Cache(width: width, frames: out)
        return out
    }
    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout Cache) -> CGSize {
        let width = proposal.width.flatMap { $0.isFinite ? $0 : nil } ?? minWidth
        return CGSize(width: width, height: frames(width, subviews, &cache, reuse: false).map(\.maxY).max() ?? 0)
    }
    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout Cache) {
        for (v, f) in zip(subviews, frames(bounds.width, subviews, &cache, reuse: true)) {
            v.place(at: CGPoint(x: bounds.minX + f.minX, y: bounds.minY + f.minY), proposal: ProposedViewSize(f.size))
        }
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
    var onOpen: (String, Int?) -> Void
    var onSelect: (String) -> Void
    var body: some View {
        // A lens gets a tile only beside another: alone, its tile would repeat its section's heading.
        let others = reports.filter { $0.kind != .files }
        HStack(alignment: .top, spacing: 10) {
            ForEach(reports.filter { $0.kind == .files } + (others.count > 1 ? others : [])) { r in
                SummaryTile(report: r, onOpen: onOpen).onTapGesture { if r.kind != .files { onSelect(r.id) } }
            }
        }
        .fixedSize(horizontal: false, vertical: true)
    }
}

private struct SummaryTile: View {
    var report: LensReport
    var onOpen: (String, Int?) -> Void
    @State private var hover = false
    var body: some View {
        let m = report.metric
        // "1 table ~1" says the same thing twice: then the summary says what happened instead.
        let restates = report.kind != .files && [m.added, m.removed, m.changed].filter { $0 > 0 }.count <= 1
        let generated = report.kinds["Generated"]?.count ?? 0
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 7) {
                Image(systemName: report.icon).font(.system(size: 10, weight: .medium)).foregroundStyle(.white)
                    .frame(width: 20, height: 20)
                    .background(report.kind.tint.gradient, in: RoundedRectangle(cornerRadius: 5))
                Text(report.title).font(.callout).foregroundStyle(.secondary).lineLimit(1)
                Spacer(minLength: 0)
                Image(systemName: "chevron.right").font(.system(size: 9, weight: .semibold)).foregroundStyle(.tertiary).opacity(hover && report.kind != .files ? 1 : 0)
            }
            HStack(alignment: .bottom, spacing: 16) {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(alignment: .firstTextBaseline, spacing: 5) {
                        Text("\(m.value)").font(.system(size: 24, weight: .semibold, design: .rounded).monospacedDigit())
                        Text(m.unit).font(.callout).foregroundStyle(.secondary).lineLimit(1)
                    }
                    HStack(spacing: 6) {
                        if restates { Text(report.summary).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
                        else { Deltas(added: m.added, removed: m.removed, changed: m.changed) }
                        if generated > 0 { Text("\(m.value - generated) to read").font(.caption).foregroundStyle(.secondary) }
                    }
                    .frame(height: 13, alignment: .leading)
                }
                .fixedSize(horizontal: report.kind == .files, vertical: false)
                if report.kind == .files { FileKinds(facts: report.sections.first?.facts ?? [], kinds: report.kinds, onOpen: onOpen) }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Color.primary.opacity(hover && report.kind != .files ? 0.06 : 0.035), in: RoundedRectangle(cornerRadius: 8))
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
                    Text(report.summary).font(.callout).foregroundStyle(.secondary).lineLimit(1).padding(.leading, 4)
                    Spacer()
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if !collapsed {
                switch report.kind {
                case .files: EmptyView()
                case .database: DatabaseBody(groups: report.database, onOpen: onOpen)
                case .dependencies: DependencyBody(report: report, onOpen: onOpen)
                case .configuration: ConfigBody(report: report, onOpen: onOpen)
                case .followUps: FollowUpBody(report: report, onOpen: onOpen)
                }
            }
        }
    }
}

// MARK: - Files

/// How the changed files split by kind: a thin bar, and the counts in one wrapping line.
private struct FileKinds: View {
    var facts: [LensFact]
    var kinds: [String: [ChangedPath]]
    var onOpen: (String, Int?) -> Void
    static let colors: [String: Color] = ["Code": .blue, "Tests": .teal, "SQL": .orange, "Dependencies": .brown, "Config": .gray,
                                          "Build & CI": .indigo, "Docs": .mint, "Assets": .pink, "Generated": .gray.opacity(0.45)]
    var body: some View {
        let total = max(1, facts.reduce(0) { $0 + ($1.count ?? 0) })
        VStack(alignment: .leading, spacing: 6) {
            GeometryReader { g in
                HStack(spacing: 1.5) {
                    ForEach(facts) { f in
                        Rectangle().fill(Self.colors[f.title, default: .gray])
                            .frame(width: max(2, (g.size.width - CGFloat(facts.count - 1) * 1.5) * CGFloat(f.count ?? 0) / CGFloat(total)))
                    }
                }
            }
            .frame(height: 4)
            .clipShape(Capsule())
            Flow(spacing: 12, lineSpacing: 3) {
                ForEach(facts) { f in KindChip(fact: f, color: Self.colors[f.title, default: .gray], files: kinds[f.title] ?? [], onOpen: onOpen) }
            }
        }
    }
}

/// "● 2 SQL": its files in a popover.
private struct KindChip: View {
    var fact: LensFact
    var color: Color
    var files: [ChangedPath]
    var onOpen: (String, Int?) -> Void
    @State private var shown = false
    @State private var hover = false

    var body: some View {
        Button { shown = true } label: {
            HStack(spacing: 4) {
                Circle().fill(color).frame(width: 7, height: 7)
                Text("\(fact.count ?? 0) \(fact.title == "SQL" ? "SQL" : fact.title.lowercased())")
                    .foregroundStyle(hover || shown ? .primary : .secondary)
            }
            .font(.caption)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .pointerStyle(.link)
        .onHover { hover = $0 }
        .popover(isPresented: $shown, arrowEdge: .bottom) {
            KindFiles(kind: fact.title, files: files) { p in shown = false; onOpen(p, nil) }
        }
    }
}

/// Views in rows, wrapping to the next row where one doesn't fit.
private struct Flow: Layout {
    var spacing: CGFloat = 8
    var lineSpacing: CGFloat = 4

    private func frames(_ width: CGFloat, _ subviews: Subviews) -> [CGRect] {
        var out: [CGRect] = [], x: CGFloat = 0, y: CGFloat = 0, row: CGFloat = 0
        for v in subviews {
            let s = v.sizeThatFits(.unspecified)
            if x > 0, x + s.width > width { x = 0; y += row + lineSpacing; row = 0 }
            out.append(CGRect(origin: CGPoint(x: x, y: y), size: s))
            x += s.width + spacing; row = max(row, s.height)
        }
        return out
    }
    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let f = frames(proposal.width ?? .infinity, subviews)
        return CGSize(width: f.map(\.maxX).max() ?? 0, height: f.map(\.maxY).max() ?? 0)
    }
    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for (v, f) in zip(subviews, frames(bounds.width, subviews)) {
            v.place(at: CGPoint(x: bounds.minX + f.minX, y: bounds.minY + f.minY), proposal: ProposedViewSize(f.size))
        }
    }
}

/// The files of one kind, each opening in Files.
private struct KindFiles: View {
    var kind: String
    var files: [ChangedPath]
    var open: (String) -> Void

    var body: some View {
        let list = VStack(alignment: .leading, spacing: 0) {
            Text(kind).font(.headline).padding(.horizontal, 12).padding(.top, 10).padding(.bottom, 4)
            ForEach(files, id: \.self) { c in
                let k: LensFact.Kind = c.old == nil ? .added : c.new == nil ? .removed : c.old != c.new ? .renamed : .changed
                HStack(spacing: 7) {
                    FileIcon(path: c.path, size: 14)
                    Text((c.path as NSString).lastPathComponent).lineLimit(1)
                    Text((c.path as NSString).deletingLastPathComponent).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
                    Spacer(minLength: 6)
                    StatusSymbol(kind: k, size: 10)
                }
                .font(.system(size: 12))
                .padding(.horizontal, 12).padding(.vertical, 4)
                .openable(c.path, nil) { p, _ in open(p) }
            }
        }
        .padding(.bottom, 8)
        Group { if files.count > 16 { ScrollView { list }.frame(height: 460) } else { list } }.frame(width: 400)
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
                        Grouped { CappedRows(items: g.warnings, limit: 6, noun: "Warnings", inset: 36) { WarningRow(fact: $0, onOpen: onOpen) } }
                    }
                    if !g.tables.isEmpty {
                        VStack(alignment: .leading, spacing: 8) {
                            Subheading(text: "Tables")
                            Masonry { ForEach(g.tables) { TableBox(table: $0, onOpen: onOpen) } }
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
        // Only a key, index or trigger changed: the columns say nothing.
        let untouched = table.kind == .changed && !showAll && !table.extras.isEmpty && table.columns.allSatisfy { $0.kind == .info }
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
            if !untouched {
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
            }
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
                // A short definition reads on one line; a long one (a CHECK, say) as the SQL it was, before above after.
                if fact.tag != "trigger", (fact.before ?? "").count + (fact.after ?? "").count > 90 || (fact.before ?? fact.after ?? "").contains("\n") {
                    Group {
                        if let b = fact.before, let a = fact.after { LineDiff(before: b, after: a) }
                        else if let b = fact.before { CodeBlock(text: b, path: "x.sql", kind: .removed, maxLines: 14) }
                        else if let a = fact.after { CodeBlock(text: a, path: "x.sql", kind: .added, maxLines: 14) }
                    }
                    .padding(.top, 3)
                } else if let v = fact.after ?? fact.before, fact.tag != "trigger" {
                    Text(fact.before.map { b in fact.after != nil ? "\(b) → \(v)" : v } ?? v)
                        .font(.system(size: 10.5, design: .monospaced)).foregroundStyle(.secondary).lineLimit(2).truncationMode(.middle)
                }
                ForEach(fact.notes, id: \.self) { Text($0).font(.system(size: 10.5)).foregroundStyle(.secondary).lineLimit(3) }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12).padding(.vertical, 2.5)
    }
}

private struct ObjectRow: View {
    var object: DBObject
    var onOpen: (String, Int?) -> Void
    @State private var open: Bool

    init(object: DBObject, onOpen: @escaping (String, Int?) -> Void) {
        self.object = object; self.onOpen = onOpen
        // A short definition is the change itself: shown, not behind a click.
        _open = State(initialValue: (object.after ?? object.before ?? "").split(separator: "\n").count <= 25)
    }

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
            ForEach(o.notes, id: \.self) { Text($0).font(.caption).foregroundStyle(.secondary).lineLimit(3).padding(.leading, 22) }
            if !code, let a = o.after ?? o.before {
                Text(o.before.map { b in o.after != nil ? "\(b)  →  \(a)" : a } ?? a)
                    .font(.system(size: 11, design: .monospaced)).foregroundStyle(.secondary).lineLimit(2).padding(.leading, 22)
            }
            if code && (o.before != nil || o.after != nil) {
                VStack(alignment: .leading, spacing: 6) {
                    Disclosure(title: "Definition", open: $open)
                    if open {
                        if o.kind == .changed, let b = o.before, let a = o.after {
                            LineDiff(before: b, after: a)
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
                    Text(fact.title).font(.system(size: 12.5, weight: .medium))
                    ForEach(fact.notes, id: \.self) { Text($0).font(.caption).foregroundStyle(.secondary) }
                }
                Spacer()
                if let p = fact.path {
                    // A migration's directory names it better than its up.sql; left out when the text already does.
                    let name = (p as NSString).lastPathComponent
                    let file = ["up.sql", "down.sql"].contains(name.lowercased()) ? ((p as NSString).deletingLastPathComponent as NSString).lastPathComponent : name
                    if !fact.notes.contains(where: { $0.contains(file) }) {
                        Text(file).font(.caption).foregroundStyle(.tertiary).lineLimit(1).truncationMode(.middle).frame(maxWidth: 260, alignment: .trailing)
                    }
                }
                if fact.after != nil { Button(show ? "Hide" : "Show") { withAnimation(.easeOut(duration: 0.15)) { show.toggle() } }.controlSize(.small) }
                if let p = fact.path { Button("Open") { onOpen(p, fact.line) }.controlSize(.small) }
            }
            if show, let s = fact.after { CodeBlock(text: s, path: "x.sql", maxLines: 14).padding(.leading, 24) }
        }
        .padding(.horizontal, 12).padding(.vertical, 9)
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

// MARK: - Follow-ups

private struct FollowUpBody: View {
    var report: LensReport
    var onOpen: (String, Int?) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            ForEach(report.sections) { s in
                VStack(alignment: .leading, spacing: 6) {
                    Subheading(text: s.title)
                    Grouped {
                        CappedRows(items: s.facts, limit: 8, noun: "Lines", inset: 10) { f in
                            HStack(spacing: 8) {
                                Text(marked(f)).font(.system(size: 11.5, design: .monospaced)).lineLimit(1)
                                Spacer(minLength: 8)
                                Text("\(((f.path ?? "") as NSString).lastPathComponent):\(f.line ?? 1)").font(.caption).foregroundStyle(.tertiary).lineLimit(1)
                            }
                            .padding(.horizontal, 10).padding(.vertical, 6)
                            .openable(f.path, f.line, onOpen)
                        }
                    }
                }
            }
        }
    }

    /// The line, with what it matched in colour.
    private func marked(_ f: LensFact) -> AttributedString {
        var a = AttributedString(f.title)
        a.foregroundColor = .secondary
        if let t = f.tag, let r = a.range(of: t) { a[r].foregroundColor = .orange }
        return a
    }
}

// MARK: - Branch

/// How far the change's base is behind the branch it goes into, whether the two merge, and the commits there that
/// change the same files. Conflicting files and those commits fold away: there can be dozens.
private struct BranchRow: View {
    var status: BranchStatus
    var onOpen: (String, Int?) -> Void
    @State private var showConflicts = false
    @State private var showCommits = false

    private struct File: Identifiable { var id: String }

    var body: some View {
        let s = status
        let conflicts = s.conflicts ?? []
        Grouped {
            HStack(spacing: 10) {
                Image(systemName: "arrow.triangle.branch").font(.system(size: 13)).foregroundStyle(.secondary).frame(width: 18)
                VStack(alignment: .leading, spacing: 1) {
                    Text("\(plural(s.behind, "commit")) behind \(s.target)").font(.system(size: 12.5, weight: .medium))
                    Text(s.touching.isEmpty ? "None of them change these files"
                         : "\(s.touching.count == s.behind ? "All" : "\(s.touching.count)") of them change these files")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer(minLength: 8)
                if let c = s.conflicts {
                    Label(c.isEmpty ? "Merges cleanly" : plural(c.count, "conflicting file"),
                          systemImage: c.isEmpty ? "checkmark.circle.fill" : "xmark.octagon.fill")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(c.isEmpty ? .green : .red)
                        .padding(.horizontal, 8).padding(.vertical, 3)
                        .background((c.isEmpty ? Color.green : .red).opacity(0.12), in: Capsule())
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 9)
            if !conflicts.isEmpty {
                Divider()
                DisclosureRow(title: "Conflicting files", count: conflicts.count, open: $showConflicts)
                if showConflicts {
                    CappedRows(items: conflicts.map(File.init), limit: 8, noun: "Files", inset: 36) { f in
                        // Who changed it on the other side: whom to ask about the conflict.
                        let by = Array(Set(s.touching.filter { $0.files.contains(f.id) }.map(\.author))).sorted()
                        HStack(spacing: 7) {
                            FileIcon(path: f.id, size: 14)
                            Text((f.id as NSString).lastPathComponent).lineLimit(1)
                            Text((f.id as NSString).deletingLastPathComponent).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
                            Spacer(minLength: 8)
                            if !by.isEmpty { Text("changed on \(s.target) by " + by.joined(separator: ", ")).font(.caption).foregroundStyle(.tertiary).lineLimit(1) }
                        }
                        .font(.system(size: 12))
                        .padding(.leading, 36).padding(.trailing, 12).padding(.vertical, 4)
                        .openable(f.id, nil, onOpen)
                    }
                }
            }
            if !s.touching.isEmpty {
                Divider()
                DisclosureRow(title: "Commits on \(s.target) that change these files", count: s.touching.count, open: $showCommits)
                if showCommits {
                    CappedRows(items: s.touching, limit: 8, noun: "Commits", inset: 36) { c in
                        HStack(spacing: 8) {
                            Text(c.id).font(.system(size: 11, design: .monospaced)).foregroundStyle(.secondary)
                            Text(c.subject).lineLimit(1)
                            Text("\(c.author) · \(c.date)").foregroundStyle(.secondary).lineLimit(1)
                            Spacer(minLength: 8)
                            Text(c.files.map { ($0 as NSString).lastPathComponent }.joined(separator: ", "))
                                .font(.caption).foregroundStyle(.tertiary).lineLimit(1)
                        }
                        .font(.system(size: 12))
                        .padding(.leading, 36).padding(.trailing, 12).padding(.vertical, 4)
                    }
                }
            }
        }
    }
}

/// A full-width row that shows or hides what's under it: "▸ Conflicting files  7".
private struct DisclosureRow: View {
    var title: String
    var count: Int
    @Binding var open: Bool

    var body: some View {
        Button { withAnimation(.easeOut(duration: 0.15)) { open.toggle() } } label: {
            HStack(spacing: 6) {
                Image(systemName: "chevron.right").font(.system(size: 9, weight: .semibold)).foregroundStyle(.secondary)
                    .rotationEffect(.degrees(open ? 90 : 0)).frame(width: 18)
                Text(title).font(.system(size: 12))
                Text("\(count)").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                    .padding(.horizontal, 6).padding(.vertical, 1)
                    .background(Color.primary.opacity(0.07), in: Capsule())
                Spacer()
            }
            .padding(.horizontal, 12).padding(.vertical, 7)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .pointerStyle(.link)
    }
}

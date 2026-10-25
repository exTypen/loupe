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

    var body: some View {
        Group {
            if let reports, computedFor == key {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 12) {
                        ForEach(reports) { r in LensCard(report: r, onOpen: onOpen) }
                        if reports.count == 1 {
                            Text("Nothing structural to report beyond the files: no schema, dependency, configuration or code-definition changes.")
                                .font(.callout).foregroundStyle(.secondary).padding(.horizontal, 4)
                        }
                    }
                    .padding(16)
                    .frame(maxWidth: 1100, alignment: .leading)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            } else {
                VStack(spacing: 8) {
                    ProgressView()
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
}

private struct LensCard: View {
    var report: LensReport
    var onOpen: (String, Int?) -> Void
    @State private var collapsed = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button { withAnimation(.easeOut(duration: 0.15)) { collapsed.toggle() } } label: {
                HStack(spacing: 8) {
                    Image(systemName: report.icon).foregroundStyle(.tint).frame(width: 20)
                    Text(report.title).font(.headline)
                    Text(report.summary).foregroundStyle(.secondary).lineLimit(1)
                    Spacer()
                    Image(systemName: "chevron.down").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                        .rotationEffect(.degrees(collapsed ? -90 : 0))
                }
                .padding(12)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if !collapsed {
                Divider()
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(report.sections) { s in LensSectionView(section: s, onOpen: onOpen) }
                }
                .padding(12)
            }
        }
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator))
    }
}

private struct LensSectionView: View {
    var section: LensSection
    var onOpen: (String, Int?) -> Void
    @State private var expanded = false
    private let limit = 12

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            if section.facts.count > 1 || section.title.contains("/") {
                Text(section.title).font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
            }
            ForEach(expanded ? section.facts : Array(section.facts.prefix(limit))) { f in FactRow(fact: f, onOpen: onOpen) }
            if section.facts.count > limit && !expanded {
                Button("Show \(section.facts.count - limit) more") { expanded = true }.buttonStyle(.link).font(.caption)
            }
        }
    }
}

private struct FactRow: View {
    var fact: LensFact
    var onOpen: (String, Int?) -> Void
    @State private var hover = false

    var body: some View {
        let (glyph, tint): (String, Color) = switch fact.kind {
        case .added: ("plus.circle.fill", .green)
        case .removed: ("minus.circle.fill", .red)
        case .changed: ("circle.lefthalf.filled", .orange)
        case .renamed: ("arrow.right.circle.fill", .purple)
        case .info: ("circle.fill", .secondary)
        case .warning: ("exclamationmark.triangle.fill", .yellow)
        }
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: glyph).foregroundStyle(tint).font(.caption)
            Text(fact.title).font(.callout.monospaced().weight(.medium)).textSelection(.enabled).lineLimit(2)
            if let d = fact.detail { Text(d).font(.callout).foregroundStyle(.secondary).lineLimit(2).textSelection(.enabled) }
            Spacer(minLength: 0)
            if fact.path != nil {
                Image(systemName: "arrow.up.right").font(.caption2).foregroundStyle(.tertiary).opacity(hover ? 1 : 0)
            }
        }
        .padding(.vertical, 2).padding(.horizontal, 4)
        .background(hover && fact.path != nil ? Color.primary.opacity(0.06) : .clear, in: RoundedRectangle(cornerRadius: 4))
        .contentShape(Rectangle())
        .onHover { hover = $0 }
        .onTapGesture { if let p = fact.path { onOpen(p, fact.line) } }
        .help(fact.path.map { "\($0)\(fact.line.map { ":\($0)" } ?? "")" } ?? "")
    }
}

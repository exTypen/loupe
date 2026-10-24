import SwiftUI
import AppKit

// MARK: - Parsing conflict markers

/// A file with conflict markers, split into the text around the conflicts and the conflicts themselves.
/// Handles the diff3 style (`|||||||` base section) too.
struct ConflictText {
    struct Conflict {
        var current: [Substring]
        var base: [Substring]?
        var incoming: [Substring]
        var currentLabel: String
        var incomingLabel: String
    }
    enum Segment {
        case text([Substring])
        case conflict(Conflict)
    }
    var segments: [Segment] = []
    var conflictCount: Int { segments.reduce(0) { if case .conflict = $1 { $0 + 1 } else { $0 } } }

    /// Each segment with the index of the conflict it is (counting conflicts only), for numbering and resolving.
    var numbered: [(segment: Segment, conflict: Int)] {
        var k = -1
        return segments.map { seg in
            if case .conflict = seg { k += 1 }
            return (seg, k)
        }
    }

    init(_ text: String) {
        var plain: [Substring] = []
        var cur: Conflict?
        var section = 0   // 1 current, 2 base, 3 incoming
        func marker(_ l: Substring, _ c: Character) -> Bool {
            l.count >= 7 && l.prefix(7).allSatisfy { $0 == c } && (l.count == 7 || l[l.index(l.startIndex, offsetBy: 7)] == " ")
        }
        func label(_ l: Substring) -> String { l.dropFirst(7).trimmingCharacters(in: .whitespaces) }
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            if cur == nil, marker(line, "<") {
                if !plain.isEmpty { segments.append(.text(plain)); plain = [] }
                cur = Conflict(current: [], base: nil, incoming: [], currentLabel: label(line), incomingLabel: "")
                section = 1
            } else if cur != nil, section == 1, marker(line, "|") {
                cur!.base = []; section = 2
            } else if cur != nil, section < 3, line == "=======" {
                section = 3
            } else if cur != nil, section == 3, marker(line, ">") {
                cur!.incomingLabel = label(line)
                segments.append(.conflict(cur!)); cur = nil; section = 0
            } else if cur != nil {
                switch section {
                case 1: cur!.current.append(line)
                case 2: cur!.base!.append(line)
                default: cur!.incoming.append(line)
                }
            } else {
                plain.append(line)
            }
        }
        if !plain.isEmpty { segments.append(.text(plain)) }
    }

    /// The file with conflict `index` (counting conflicts only) replaced by `lines`.
    func resolving(_ index: Int, with lines: [Substring]) -> String {
        var out: [Substring] = []
        var k = 0
        for seg in segments {
            switch seg {
            case .text(let t): out += t
            case .conflict(let c):
                if k == index { out += lines } else {
                    out.append(Substring("<<<<<<< " + c.currentLabel)); out += c.current
                    if let b = c.base { out.append("|||||||"); out += b }
                    out.append("======="); out += c.incoming
                    out.append(Substring(">>>>>>> " + c.incomingLabel))
                }
                k += 1
            }
        }
        return out.joined(separator: "\n")
    }
}

// MARK: - Banner

/// Above the Changes split while an operation is half done: what it is, what's left, and the way out.
struct OperationBanner: View {
    var model: WorktreeModel

    var body: some View {
        let n = model.conflicted.count
        HStack(spacing: 10) {
            Image(systemName: n > 0 ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
                .foregroundStyle(n > 0 ? .orange : .green)
            VStack(alignment: .leading, spacing: 1) {
                if let op = model.operation {
                    Text("\(op.verb) \(model.operationSubject)").fontWeight(.medium).lineLimit(1).truncationMode(.tail)
                } else {
                    Text("Conflicts").fontWeight(.medium)
                }
                Text(n > 0 ? "\(n) file\(n == 1 ? "" : "s") with conflicts. Resolve each one, then continue."
                           : model.operation == nil ? "" : "All conflicts are resolved.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if let op = model.operation {
                Button("Abort \(op.command.capitalized)") { Task { await model.finishOperation(abort: true) } }
                    .help("Go back to how things were before the \(op.command)")
                Button("Continue") { Task { await model.finishOperation(abort: false) } }
                    .buttonStyle(.borderedProminent)
                    .disabled(n > 0)
                    .help(n > 0 ? "Resolve the conflicts first" : "Finish the \(op.command)")
            }
        }
        .controlSize(.small)
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background((model.conflicted.isEmpty ? Color.green : Color.orange).opacity(0.08))
    }
}

// MARK: - Resolver

/// Replaces the diff for a conflicted file: the file as it is on disk, each conflict as a card with its two sides
/// and a button per resolution. Resolving the last one stages the file, which is what marks it resolved.
struct ConflictPane: View {
    var model: WorktreeModel
    var path: String
    @State private var text: ConflictText?
    @State private var unreadable = false

    private var url: URL { URL(fileURLWithPath: model.path).appendingPathComponent(path) }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if let text, text.conflictCount > 0 {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(text.numbered.enumerated()), id: \.offset) { _, item in
                            switch item.segment {
                            case .text(let lines): ContextLines(lines: lines)
                            case .conflict(let c):
                                let k = item.conflict
                                ConflictCard(conflict: c, index: k, total: text.conflictCount) { lines in write(text.resolving(k, with: lines)) }
                                    .padding(.vertical, 6)
                            }
                        }
                    }
                    .padding(.horizontal, 12).padding(.vertical, 8)
                }
            } else if text != nil || unreadable {
                ContentUnavailableView {
                    Label(unreadable ? "No Text to Merge" : "No Conflict Markers Left", systemImage: unreadable ? "doc.questionmark" : "checkmark.circle")
                } description: {
                    Text(unreadable ? "One side deleted or replaced this file, or it's binary. Keep one side with the buttons above."
                                    : "The file looks resolved. Mark it resolved to stage it.")
                }
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .task(id: model.files) { load() }   // the watcher refreshes status when the file changes on disk
    }

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
            Text(path).fontWeight(.medium).lineLimit(1).truncationMode(.head).textSelection(.enabled)
            if let n = text?.conflictCount, n > 0 { Text(n == 1 ? "1 conflict" : "\(n) conflicts").foregroundStyle(.secondary) }
            Spacer()
            Group {
                Button("Open in Editor") { NSWorkspace.shared.open(url) }
                Button("Use Current") { Task { await model.resolve(path, take: .current) } }
                    .help("Keep the whole file as it is on the current branch")
                Button("Use Incoming") { Task { await model.resolve(path, take: .incoming) } }
                    .help("Take the whole file from the incoming change")
                Button("Mark Resolved") { Task { await model.resolve(path, take: nil) } }
                    .buttonStyle(.borderedProminent)
                    .disabled((text?.conflictCount ?? 0) > 0)
                    .help("Stage the file as it is on disk")
            }
            .controlSize(.small)
        }
        .padding(.horizontal, 12).padding(.vertical, 7)
        .background(.bar)
    }

    private func load() {
        guard let data = try? Data(contentsOf: url), let s = String(data: data, encoding: .utf8) else { unreadable = true; text = nil; return }
        unreadable = false
        text = ConflictText(s)
    }

    private func write(_ s: String) {
        do {
            try Data(s.utf8).write(to: url, options: .atomic)
            let t = ConflictText(s)
            text = t
            if t.conflictCount == 0 { Task { await model.resolve(path, take: nil) } }
        } catch { model.error = error.localizedDescription }
    }
}

/// Unchanged text between conflicts; long runs fold to their first and last lines.
private struct ContextLines: View {
    var lines: [Substring]
    @State private var expanded = false

    var body: some View {
        let fold = lines.count > 10 && !expanded
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array((fold ? Array(lines.prefix(3)) : lines).enumerated()), id: \.offset) { _, l in CodeLine(text: l) }
            if fold {
                Button { expanded = true } label: {
                    Label("\(lines.count - 6) unchanged lines", systemImage: "ellipsis").font(.caption).foregroundStyle(.secondary)
                }
                .buttonStyle(.plain).padding(.vertical, 4)
                ForEach(Array(lines.suffix(3).enumerated()), id: \.offset) { _, l in CodeLine(text: l) }
            }
        }
        .foregroundStyle(.secondary)
    }
}

private struct CodeLine: View {
    var text: Substring
    var body: some View {
        Text(text.isEmpty ? " " : String(text)).font(.system(.body, design: .monospaced)).lineLimit(1).textSelection(.enabled)
    }
}

private struct ConflictCard: View {
    var conflict: ConflictText.Conflict
    var index: Int
    var total: Int
    var resolve: ([Substring]) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Text("Conflict \(index + 1) of \(total)").font(.callout.weight(.semibold))
                Spacer()
                Button("Use Current") { resolve(conflict.current) }
                Button("Use Incoming") { resolve(conflict.incoming) }
                Button("Use Both") { resolve(conflict.current + conflict.incoming) }.help("Current first, then incoming")
            }
            .controlSize(.small)
            .padding(8)
            side("Current", conflict.currentLabel, conflict.current, .blue)
            side("Incoming", conflict.incomingLabel, conflict.incoming, .purple)
        }
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.orange.opacity(0.4)))
    }

    private func side(_ title: String, _ label: String, _ lines: [Substring], _ tint: Color) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 4) {
                Text(title).font(.caption.weight(.semibold)).foregroundStyle(tint)
                if !label.isEmpty { Text(label).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle) }
            }
            .padding(.horizontal, 8).padding(.vertical, 4)
            VStack(alignment: .leading, spacing: 0) {
                if lines.isEmpty { Text("(nothing)").font(.caption).foregroundStyle(.tertiary).padding(.vertical, 2) }
                ForEach(Array(lines.enumerated()), id: \.offset) { _, l in CodeLine(text: l) }
            }
            .padding(.horizontal, 8).padding(.vertical, 4)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(tint.opacity(0.10))
        }
    }
}

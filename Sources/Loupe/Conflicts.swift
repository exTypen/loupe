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
    private(set) var segments: [Segment] = []
    private(set) var conflictCount = 0
    /// Each segment's offset and the index of the conflict it is (counting conflicts only), for numbering and
    /// resolving. Built once: the conflict pane's body reads it on every update.
    private(set) var numbered: [(offset: Int, segment: Segment, conflict: Int)] = []

    init(_ text: String) {
        var plain: [Substring] = []
        var cur: Conflict?
        var section = 0   // 1 current, 2 base, 3 incoming
        func marker(_ l: Substring, _ c: Character) -> Bool {
            l.count >= 7 && l.prefix(7).allSatisfy { $0 == c } && (l.count == 7 || l[l.index(l.startIndex, offsetBy: 7)] == " ")
        }
        func label(_ l: Substring) -> String { l.dropFirst(7).trimmingCharacters(in: .whitespacesAndNewlines) }
        for line in Merge3.split(text) {
            if cur == nil, marker(line, "<") {
                if !plain.isEmpty { segments.append(.text(plain)); plain = [] }
                cur = Conflict(current: [], base: nil, incoming: [], currentLabel: label(line), incomingLabel: "")
                section = 1
            } else if cur != nil, section == 1, marker(line, "|") {
                cur!.base = []; section = 2
            } else if cur != nil, section < 3, line == "=======" || line == "=======\r" {
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
        var k = -1
        numbered = segments.enumerated().map { i, seg in
            if case .conflict = seg { k += 1 }
            return (i, seg, k)
        }
        conflictCount = k + 1
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

extension Array {
    subscript(safe i: Int) -> Element? { indices.contains(i) ? self[i] : nil }
}

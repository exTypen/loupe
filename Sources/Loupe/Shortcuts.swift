import SwiftUI
import AppKit
import Observation

/// A key plus modifiers, as stored in UserDefaults and shown in menus.
struct Shortcut: Codable, Hashable {
    var key: String                  // a lowercase character, or a name from `Shortcut.special`
    var modifiers: UInt              // NSEvent.ModifierFlags raw value: command, shift, option, control only

    static let special: [String: (code: UInt16, equivalent: KeyEquivalent, glyph: String)] = [
        "up": (126, .upArrow, "↑"), "down": (125, .downArrow, "↓"), "left": (123, .leftArrow, "←"), "right": (124, .rightArrow, "→"),
        "return": (36, .return, "↩"), "tab": (48, .tab, "⇥"), "space": (49, .space, "Space"), "delete": (51, .delete, "⌫"),
    ]

    init(_ key: String, _ modifiers: NSEvent.ModifierFlags = .command) { self.key = key; self.modifiers = modifiers.rawValue }

    var flags: NSEvent.ModifierFlags { NSEvent.ModifierFlags(rawValue: modifiers) }
    var equivalent: KeyEquivalent { Self.special[key]?.equivalent ?? KeyEquivalent(Character(key)) }
    var eventModifiers: EventModifiers {
        var m: EventModifiers = []
        if flags.contains(.command) { m.insert(.command) }
        if flags.contains(.shift) { m.insert(.shift) }
        if flags.contains(.option) { m.insert(.option) }
        if flags.contains(.control) { m.insert(.control) }
        return m
    }
    var display: String {
        var s = ""
        if flags.contains(.control) { s += "⌃" }
        if flags.contains(.option) { s += "⌥" }
        if flags.contains(.shift) { s += "⇧" }
        if flags.contains(.command) { s += "⌘" }
        return s + (Self.special[key]?.glyph ?? key.uppercased())
    }

    /// The shortcut a key-down event types, or nil for keys that can't be one (bare letters, modifiers alone).
    init?(event e: NSEvent) {
        let mods = e.modifierFlags.intersection([.command, .shift, .option, .control])
        if let name = Self.special.first(where: { $0.value.code == e.keyCode })?.key {
            guard !mods.isEmpty else { return nil }
            self.init(name, mods)
            return
        }
        // Without modifiers applied, so ⇧1 records as 1, not !.
        guard let c = e.characters(byApplyingModifiers: [])?.lowercased(), c.count == 1, !c.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }),
              mods.contains(.command) || mods.contains(.control) || mods.contains(.option) else { return nil }
        self.init(c, mods)
    }
}

enum ShortcutAction: String, CaseIterable, Identifiable {
    case openRepository, goToWorktree, toggleSidebar, changesTab, historyTab, pullsTab, refresh, commit, nextFile, previousFile, markViewed, back
    case fullMode, navigateBack, navigateForward
    var id: String { rawValue }

    var title: String {
        switch self {
        case .openRepository: "Open Repository"
        case .goToWorktree: "Go to Worktree"
        case .toggleSidebar: "Toggle Sidebar"
        case .changesTab: "Changes"
        case .historyTab: "History"
        case .pullsTab: "Pull Requests"
        case .refresh: "Refresh"
        case .commit: "Commit"
        case .nextFile: "Next File"
        case .previousFile: "Previous File"
        case .markViewed: "Mark Viewed & Next"
        case .back: "Back to List"
        case .fullMode: "Compact / Full Mode"
        case .navigateBack: "Navigate Back"
        case .navigateForward: "Navigate Forward"
        }
    }

    var group: String {
        switch self {
        case .openRepository, .goToWorktree, .toggleSidebar, .refresh: "General"
        case .changesTab, .historyTab, .pullsTab: "Tabs"
        case .commit: "Changes"
        case .nextFile, .previousFile, .markViewed, .back, .fullMode, .navigateBack, .navigateForward: "Review"
        }
    }

    var standard: Shortcut? {
        switch self {
        case .openRepository: Shortcut("o")
        case .goToWorktree: Shortcut("k")
        case .toggleSidebar: Shortcut("s", [.command, .shift])
        case .changesTab: Shortcut("1")
        case .historyTab: Shortcut("2")
        case .pullsTab: Shortcut("3")
        case .refresh: Shortcut("r")
        case .commit: Shortcut("return")
        case .nextFile: Shortcut("down", .option)
        case .previousFile: Shortcut("up", .option)
        case .markViewed: Shortcut("v", [.command, .shift])
        case .back: Shortcut("[")
        case .fullMode: Shortcut("f", [.command, .option])
        case .navigateBack: Shortcut("-", .control)
        case .navigateForward: Shortcut("-", [.control, .shift])
        }
    }
}

/// The user's shortcuts. Menus and buttons read them through `.shortcut(_:)`, so a change applies at once.
@MainActor @Observable
final class Shortcuts {
    static let shared = Shortcuts()
    private static let key = "shortcuts"
    /// Only what differs from the defaults; an entry with nil means "no shortcut".
    private var custom: [String: Shortcut?] = (try? JSONDecoder().decode([String: Shortcut?].self, from: UserDefaults.standard.data(forKey: key) ?? Data())) ?? [:] {
        didSet { UserDefaults.standard.set(try? JSONEncoder().encode(custom), forKey: Self.key); revision += 1 }
    }
    private(set) var revision = 0   // read by the App body, so menus are rebuilt with the new key equivalents

    func shortcut(_ a: ShortcutAction) -> Shortcut? {
        if let entry = custom[a.rawValue] { return entry }
        return a.standard
    }
    func isCustom(_ a: ShortcutAction) -> Bool { custom[a.rawValue] != nil }

    /// Sets `a`'s shortcut; any other action that had the same one loses it. Returns that action.
    @discardableResult func set(_ s: Shortcut?, for a: ShortcutAction) -> ShortcutAction? {
        let taken = s.flatMap { s in ShortcutAction.allCases.first { $0 != a && shortcut($0) == s } }
        if let taken { custom[taken.rawValue] = .some(nil) }
        custom[a.rawValue] = s == a.standard ? nil : .some(s)
        return taken
    }

    func reset() { custom = [:] }
}

extension View {
    /// The user's shortcut for `action`, if it has one.
    @ViewBuilder func shortcut(_ action: ShortcutAction) -> some View {
        if let s = Shortcuts.shared.shortcut(action) { keyboardShortcut(s.equivalent, modifiers: s.eventModifiers) } else { self }
    }
}

// MARK: - Settings

struct ShortcutSettings: View {
    @State private var shortcuts = Shortcuts.shared
    @State private var recording: ShortcutAction?
    @State private var note: String?

    var body: some View {
        Form {
            ForEach(["General", "Tabs", "Changes", "Review"], id: \.self) { group in
                Section(group) {
                    ForEach(ShortcutAction.allCases.filter { $0.group == group }) { a in
                        LabeledContent(a.title) {
                            HStack(spacing: 6) {
                                ShortcutRecorder(shortcut: shortcuts.shortcut(a), recording: Binding(get: { recording == a }, set: { recording = $0 ? a : nil })) { s in
                                    let taken = shortcuts.set(s, for: a)
                                    note = taken.map { "\(s?.display ?? "") was used by “\($0.title)”, which now has no shortcut." }
                                }
                                if shortcuts.isCustom(a) {
                                    Button { shortcuts.set(a.standard, for: a); note = nil } label: { Image(systemName: "arrow.uturn.backward") }
                                        .buttonStyle(.borderless).help("Restore \(a.standard?.display ?? "no shortcut")")
                                }
                            }
                        }
                    }
                }
            }
            Section {
                HStack {
                    if let note { Text(note).font(.caption).foregroundStyle(.orange) }
                    Spacer()
                    Button("Restore Defaults") { shortcuts.reset(); note = nil }
                }
            } footer: {
                Text("Click a shortcut, then press the new keys. Esc cancels, ⌫ removes the shortcut.").font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

/// A shortcut field: shows the keys; click it and type new ones.
struct ShortcutRecorder: View {
    var shortcut: Shortcut?
    @Binding var recording: Bool
    var onChange: (Shortcut?) -> Void
    @State private var monitor: Any?

    var body: some View {
        Button { recording.toggle() } label: {
            Text(recording ? "Type shortcut…" : shortcut?.display ?? "None")
                .font(.body.monospaced())
                .foregroundStyle(recording ? Color.accentColor : shortcut == nil ? .secondary : .primary)
                .frame(minWidth: 110)
        }
        .buttonStyle(.bordered)
        .onChange(of: recording) { _, on in on ? start() : stop() }
        .onDisappear { stop() }
    }

    private func start() {
        stop()
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { e in
            if e.keyCode == 53 { recording = false; return nil }                                      // Esc
            if e.keyCode == 51, e.modifierFlags.intersection([.command, .shift, .option, .control]).isEmpty {   // bare ⌫
                onChange(nil); recording = false; return nil
            }
            guard let s = Shortcut(event: e) else { NSSound.beep(); return nil }
            onChange(s); recording = false
            return nil
        }
    }

    private func stop() { if let monitor { NSEvent.removeMonitor(monitor) }; monitor = nil }
}

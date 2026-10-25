import AppKit
import SwiftUI

// MARK: - Themes

/// A code color scheme: the editor's background and text, line numbers, and one color per token kind.
struct CodeTheme: Identifiable, Hashable {
    let id: String
    let name: String
    let dark: Bool
    let background: UInt32, foreground: UInt32, lineNumber: UInt32
    /// In `TokenKind` order: keyword, string, comment, number, type, function, macro, attribute, property.
    let tokens: [UInt32]

    static func rgb(_ v: UInt32, _ alpha: CGFloat = 1) -> NSColor {
        NSColor(srgbRed: CGFloat((v >> 16) & 0xff) / 255, green: CGFloat((v >> 8) & 0xff) / 255, blue: CGFloat(v & 0xff) / 255, alpha: alpha)
    }

    // Diff colors are the same in every theme, as translucent tints over its background.
    var added: UInt32 { dark ? 0x2EA043 : 0x1A7F37 }
    var removed: UInt32 { dark ? 0xF85149 : 0xCF222E }

    static let all: [CodeTheme] = [
        // Dark
        CodeTheme(id: "xcode-dark", name: "Xcode Default (Dark)", dark: true, background: 0x1F1F24, foreground: 0xDFDFE0, lineNumber: 0x747478,
                  tokens: [0xFF7AB2, 0xFF8170, 0x7F8C98, 0xD9C97C, 0x6BDFFF, 0x67B7A4, 0xFD8F3F, 0xBF8555, 0x4EB0CC]),
        CodeTheme(id: "vscode-dark", name: "VS Code Dark Modern", dark: true, background: 0x1F1F1F, foreground: 0xCCCCCC, lineNumber: 0x6E7681,
                  tokens: [0x569CD6, 0xCE9178, 0x6A9955, 0xB5CEA8, 0x4EC9B0, 0xDCDCAA, 0xC586C0, 0x4EC9B0, 0x9CDCFE]),
        CodeTheme(id: "jetbrains-dark", name: "JetBrains Dark", dark: true, background: 0x1E1F22, foreground: 0xBCBEC4, lineNumber: 0x4B5059,
                  tokens: [0xCF8E6D, 0x6AAB73, 0x7A7E85, 0x2AACB8, 0xBCBEC4, 0x56A8F5, 0x56A8F5, 0xB3AE60, 0xC77DBB]),
        CodeTheme(id: "darcula", name: "JetBrains Darcula", dark: true, background: 0x2B2B2B, foreground: 0xA9B7C6, lineNumber: 0x606366,
                  tokens: [0xCC7832, 0x6A8759, 0x808080, 0x6897BB, 0xA9B7C6, 0xFFC66D, 0x9876AA, 0xBBB529, 0x9876AA]),
        CodeTheme(id: "github-dark", name: "GitHub Dark", dark: true, background: 0x0D1117, foreground: 0xE6EDF3, lineNumber: 0x6E7681,
                  tokens: [0xFF7B72, 0xA5D6FF, 0x8B949E, 0x79C0FF, 0xFFA657, 0xD2A8FF, 0xD2A8FF, 0x79C0FF, 0x79C0FF]),
        CodeTheme(id: "one-dark", name: "One Dark", dark: true, background: 0x282C34, foreground: 0xABB2BF, lineNumber: 0x495162,
                  tokens: [0xC678DD, 0x98C379, 0x7F848E, 0xD19A66, 0xE5C07B, 0x61AFEF, 0x56B6C2, 0xD19A66, 0xE06C75]),
        CodeTheme(id: "dracula", name: "Dracula", dark: true, background: 0x282A36, foreground: 0xF8F8F2, lineNumber: 0x6272A4,
                  tokens: [0xFF79C6, 0xF1FA8C, 0x6272A4, 0xBD93F9, 0x8BE9FD, 0x50FA7B, 0xFFB86C, 0x50FA7B, 0x8BE9FD]),
        CodeTheme(id: "monokai", name: "Monokai", dark: true, background: 0x272822, foreground: 0xF8F8F2, lineNumber: 0x90908A,
                  tokens: [0xF92672, 0xE6DB74, 0x88846F, 0xAE81FF, 0x66D9EF, 0xA6E22E, 0xA6E22E, 0xA6E22E, 0xFD971F]),
        CodeTheme(id: "tokyo-night", name: "Tokyo Night", dark: true, background: 0x1A1B26, foreground: 0xA9B1D6, lineNumber: 0x3B4261,
                  tokens: [0xBB9AF7, 0x9ECE6A, 0x565F89, 0xFF9E64, 0x2AC3DE, 0x7AA2F7, 0x7DCFFF, 0xE0AF68, 0x73DACA]),
        CodeTheme(id: "catppuccin-mocha", name: "Catppuccin Mocha", dark: true, background: 0x1E1E2E, foreground: 0xCDD6F4, lineNumber: 0x6C7086,
                  tokens: [0xCBA6F7, 0xA6E3A1, 0x9399B2, 0xFAB387, 0xF9E2AF, 0x89B4FA, 0x94E2D5, 0xFAB387, 0xB4BEFE]),
        CodeTheme(id: "nord", name: "Nord", dark: true, background: 0x2E3440, foreground: 0xD8DEE9, lineNumber: 0x4C566A,
                  tokens: [0x81A1C1, 0xA3BE8C, 0x616E88, 0xB48EAD, 0x8FBCBB, 0x88C0D0, 0x5E81AC, 0xD08770, 0x8FBCBB]),
        CodeTheme(id: "solarized-dark", name: "Solarized Dark", dark: true, background: 0x002B36, foreground: 0x839496, lineNumber: 0x586E75,
                  tokens: [0x859900, 0x2AA198, 0x586E75, 0xD33682, 0xB58900, 0x268BD2, 0xCB4B16, 0x6C71C4, 0x268BD2]),
        // Light
        CodeTheme(id: "xcode-light", name: "Xcode Default (Light)", dark: false, background: 0xFFFFFF, foreground: 0x000000, lineNumber: 0xA6A6A6,
                  tokens: [0x9B2393, 0xC41A16, 0x5D6C79, 0x1C00CF, 0x0B4F79, 0x326D74, 0x643820, 0x815F03, 0x0F68A0]),
        CodeTheme(id: "vscode-light", name: "VS Code Light Modern", dark: false, background: 0xFFFFFF, foreground: 0x3B3B3B, lineNumber: 0x6E7681,
                  tokens: [0x0000FF, 0xA31515, 0x008000, 0x098658, 0x267F99, 0x795E26, 0xAF00DB, 0x267F99, 0x001080]),
        CodeTheme(id: "intellij-light", name: "IntelliJ Light", dark: false, background: 0xFFFFFF, foreground: 0x080808, lineNumber: 0xAEB3C2,
                  tokens: [0x0033B3, 0x067D17, 0x8C8C8C, 0x1750EB, 0x080808, 0x00627A, 0x00627A, 0x9E880D, 0x871094]),
        CodeTheme(id: "github-light", name: "GitHub Light", dark: false, background: 0xFFFFFF, foreground: 0x1F2328, lineNumber: 0x8C959F,
                  tokens: [0xCF222E, 0x0A3069, 0x6E7781, 0x0550AE, 0x953800, 0x8250DF, 0x8250DF, 0x0550AE, 0x0550AE]),
        CodeTheme(id: "one-light", name: "One Light", dark: false, background: 0xFAFAFA, foreground: 0x383A42, lineNumber: 0x9D9D9F,
                  tokens: [0xA626A4, 0x50A14F, 0xA0A1A7, 0x986801, 0xC18401, 0x4078F2, 0x0184BC, 0x986801, 0xE45649]),
        CodeTheme(id: "catppuccin-latte", name: "Catppuccin Latte", dark: false, background: 0xEFF1F5, foreground: 0x4C4F69, lineNumber: 0x8C8FA1,
                  tokens: [0x8839EF, 0x40A02B, 0x7C7F93, 0xFE640B, 0xDF8E1D, 0x1E66F5, 0x179299, 0xFE640B, 0x7287FD]),
        CodeTheme(id: "solarized-light", name: "Solarized Light", dark: false, background: 0xFDF6E3, foreground: 0x657B83, lineNumber: 0x93A1A1,
                  tokens: [0x859900, 0x2AA198, 0x93A1A1, 0xD33682, 0xB58900, 0x268BD2, 0xCB4B16, 0x6C71C4, 0x268BD2]),
    ]

    static func named(_ id: String, dark: Bool) -> CodeTheme {
        all.first { $0.id == id } ?? all.first { $0.id == (dark ? "xcode-dark" : "xcode-light") }!
    }
}

/// Every color the code views draw, resolved once per theme change: dynamic colors only index into it.
final class CodePalette {
    let background, foreground, metaForeground, lineNumber, addMark, delMark, addBG, delBG, addHL, delHL, gutterBG, hunkBG, emptyBG: NSColor
    let tokens: [NSColor]

    init(_ t: CodeTheme) {
        let rgb = CodeTheme.rgb
        background = rgb(t.background, 1); foreground = rgb(t.foreground, 1); metaForeground = rgb(t.foreground, 0.6)
        lineNumber = rgb(t.lineNumber, 1); addMark = rgb(t.added, 1); delMark = rgb(t.removed, 1)
        addBG = rgb(t.added, t.dark ? 0.17 : 0.12); delBG = rgb(t.removed, t.dark ? 0.17 : 0.11)
        addHL = rgb(t.added, t.dark ? 0.4 : 0.3); delHL = rgb(t.removed, t.dark ? 0.4 : 0.28)
        gutterBG = rgb(t.foreground, 0.03); emptyBG = rgb(t.foreground, 0.03)
        hunkBG = rgb(t.tokens[Int(TokenKind.function.rawValue)], t.dark ? 0.14 : 0.09)
        tokens = t.tokens.map { rgb($0, 1) }
    }
}

/// A theme for light and for dark, a font and a line height, as an editor ships them.
struct EditorProfile: Identifiable {
    let id: String, name: String
    let light: String, dark: String
    let font: String            // "" → the system monospaced font (SF Mono)
    let size: Double
    let lineHeight: Double      // × the font size

    static let all = [
        EditorProfile(id: "xcode", name: "Xcode", light: "xcode-light", dark: "xcode-dark", font: "", size: 12, lineHeight: 1.5),
        EditorProfile(id: "vscode", name: "VS Code", light: "vscode-light", dark: "vscode-dark", font: "Menlo", size: 12, lineHeight: 1.5),
        EditorProfile(id: "jetbrains", name: "JetBrains", light: "intellij-light", dark: "jetbrains-dark", font: "JetBrains Mono", size: 13, lineHeight: 1.45),
        EditorProfile(id: "github", name: "GitHub", light: "github-light", dark: "github-dark", font: "", size: 12, lineHeight: 1.65),
    ]
}

// MARK: - Settings

extension Notification.Name {
    /// The editor's theme or font changed: code views re-measure and redraw.
    static let editorStyleChanged = Notification.Name("editorStyleChanged")
}

/// The editor's look, persisted in UserDefaults. Code views read it through `Theme` and `Syntax.color`.
@Observable
final class EditorSettings {
    nonisolated(unsafe) static let shared = EditorSettings()
    private static let d = UserDefaults.standard

    var lightTheme = d.string(forKey: "editorLightTheme") ?? "xcode-light" { didSet { changed() } }
    var darkTheme = d.string(forKey: "editorDarkTheme") ?? "xcode-dark" { didSet { changed() } }
    var fontFamily = d.string(forKey: "editorFont") ?? "" { didSet { changed() } }
    var fontSize = d.object(forKey: "editorFontSize") as? Double ?? 12 { didSet { changed() } }
    var lineHeightFactor = d.object(forKey: "editorLineHeight") as? Double ?? 1.5 { didSet { changed() } }

    // Measured once per change: rows read these on every draw.
    @ObservationIgnored private(set) var font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
    @ObservationIgnored private(set) var smallFont = NSFont.monospacedDigitSystemFont(ofSize: 10.5, weight: .regular)
    @ObservationIgnored private(set) var charWidth: CGFloat = 7
    @ObservationIgnored private(set) var lineHeight: CGFloat = 18
    @ObservationIgnored private(set) var textY: CGFloat = 1
    @ObservationIgnored private var batching = false
    @ObservationIgnored private var palettes: [CodePalette] = []   // [light, dark]
    @ObservationIgnored private var fonts: [CGFloat: NSFont] = [:]

    private init() { measure() }

    func theme(dark: Bool) -> CodeTheme { CodeTheme.named(dark ? darkTheme : lightTheme, dark: dark) }
    func palette(dark: Bool) -> CodePalette { palettes[dark ? 1 : 0] }

    /// The editor font at another size, for code shown outside the diff (blame, overview snippets).
    func font(size: CGFloat) -> NSFont {
        if let f = fonts[size] { return f }
        let f = (fontFamily.isEmpty ? nil : NSFontManager.shared.font(withFamily: fontFamily, traits: [], weight: 5, size: size))
            ?? .monospacedSystemFont(ofSize: size, weight: .regular)
        if fonts.count >= 32 { fonts = [:] }
        fonts[size] = f
        return f
    }

    var fontInstalled: Bool { fontFamily.isEmpty || NSFontManager.shared.availableMembers(ofFontFamily: fontFamily) != nil }

    var profile: EditorProfile? {
        EditorProfile.all.first { $0.light == lightTheme && $0.dark == darkTheme && $0.font == fontFamily && $0.size == fontSize && $0.lineHeight == lineHeightFactor }
    }

    func apply(_ p: EditorProfile) {
        batching = true
        lightTheme = p.light; darkTheme = p.dark; fontFamily = p.font; fontSize = p.size; lineHeightFactor = p.lineHeight
        batching = false
        changed()
    }

    private func changed() {
        guard !batching else { return }
        let d = Self.d
        d.set(lightTheme, forKey: "editorLightTheme"); d.set(darkTheme, forKey: "editorDarkTheme"); d.set(fontFamily, forKey: "editorFont")
        d.set(fontSize, forKey: "editorFontSize"); d.set(lineHeightFactor, forKey: "editorLineHeight")
        measure()
        NotificationCenter.default.post(name: .editorStyleChanged, object: nil)
    }

    private func measure() {
        fonts = [:]
        palettes = [CodePalette(theme(dark: false)), CodePalette(theme(dark: true))]
        font = font(size: fontSize)
        smallFont = fontFamily.isEmpty ? .monospacedDigitSystemFont(ofSize: fontSize - 1.5, weight: .regular) : font(size: fontSize - 1.5)
        charWidth = ("M" as NSString).size(withAttributes: [.font: font]).width
        lineHeight = (fontSize * lineHeightFactor).rounded()
        textY = ((lineHeight - (font.ascender - font.descender + font.leading)) / 2).rounded(.down)
    }

    /// Fixed-pitch families installed on this Mac, for the font picker.
    static let monospacedFamilies: [String] = NSFontManager.shared.availableFontFamilies.filter { family in
        guard let f = NSFontManager.shared.font(withFamily: family, traits: [], weight: 5, size: 12) else { return false }
        return f.isFixedPitch && !family.hasPrefix(".")
    }
}

extension Font {
    /// The editor's font at `size`, for SwiftUI code views.
    static func editor(_ size: CGFloat) -> Font { Font(EditorSettings.shared.font(size: size) as CTFont) }
}

// MARK: - Settings pane

struct EditorSettingsView: View {
    @Bindable private var s = EditorSettings.shared

    var body: some View {
        Form {
            Section {
                Picker("Preset", selection: Binding(get: { s.profile?.id ?? "custom" }, set: { id in EditorProfile.all.first { $0.id == id }.map(s.apply) })) {
                    ForEach(EditorProfile.all) { Text($0.name).tag($0.id) }
                    if s.profile == nil { Divider(); Text("Custom").tag("custom") }
                }
                Picker("Light theme", selection: $s.lightTheme) {
                    ForEach(CodeTheme.all.filter { !$0.dark }) { ThemeLabel(theme: $0).tag($0.id) }
                }
                Picker("Dark theme", selection: $s.darkTheme) {
                    ForEach(CodeTheme.all.filter(\.dark)) { ThemeLabel(theme: $0).tag($0.id) }
                }
            } footer: {
                Text("Code follows the app's appearance: the light theme in Light mode, the dark one in Dark mode.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section {
                Picker("Font", selection: $s.fontFamily) {
                    Text("SF Mono (System)").tag("")
                    if !s.fontInstalled { Text("\(s.fontFamily) (not installed)").tag(s.fontFamily) }
                    Divider()
                    ForEach(EditorSettings.monospacedFamilies, id: \.self) { Text($0).font(.custom($0, size: 13)).tag($0) }
                }
                LabeledContent("Size") {
                    Stepper("\(s.fontSize.formatted()) pt", value: $s.fontSize, in: 9...24, step: 1)
                }
                LabeledContent("Line height") {
                    HStack {
                        Slider(value: $s.lineHeightFactor, in: 1.2...2.0, step: 0.05).frame(width: 180)
                        Text(s.lineHeightFactor.formatted(.number.precision(.fractionLength(2))) + "×").monospacedDigit().frame(width: 44, alignment: .trailing)
                    }
                }
            } footer: {
                if !s.fontInstalled {
                    Text("\(s.fontFamily) isn't installed, so code uses SF Mono. Install it from its site (JetBrains Mono: jetbrains.com/lp/mono) and reopen this pane.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            Section("Preview") {
                EditorPreview().frame(height: 9 * s.lineHeight + 2)
            }
        }
        .formStyle(.grouped)
    }
}

/// A theme's name with a strip of its colors.
private struct ThemeLabel: View {
    var theme: CodeTheme
    var body: some View {
        HStack {
            Text(theme.name)
            Spacer()
            HStack(spacing: 0) {
                ForEach([theme.background] + theme.tokens.prefix(6), id: \.self) { c in Color(nsColor: CodeTheme.rgb(c)).frame(width: 8, height: 12) }
            }
            .clipShape(RoundedRectangle(cornerRadius: 2))
        }
    }
}

/// A short diff in the current settings, through the real diff view.
private struct EditorPreview: View {
    private static let file = DiffParser.parse("""
        diff --git a/Sources/Cache.swift b/Sources/Cache.swift
        --- a/Sources/Cache.swift
        +++ b/Sources/Cache.swift
        @@ -12,7 +12,8 @@ final class Cache<Key: Hashable, Value> {
             /// Entries older than this are dropped on read.
             @Published var ttl: TimeInterval = 300
             func value(for key: Key) -> Value? {
        -        guard let e = store[key] else { return nil }
        +        guard let e = store[key], e.date.timeIntervalSinceNow > -ttl else { return nil }
        +        hits += 1
                 return e.value
             }

        """).first ?? FileDiff()

    var body: some View {
        DiffTableView(file: Self.file).clipShape(RoundedRectangle(cornerRadius: 6))
    }
}

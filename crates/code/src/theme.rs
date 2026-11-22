//! Code color schemes (EditorTheme.swift's data): plain 0xRRGGBB values, no UI types.

use crate::syntax::TokenKind;

/// A code color scheme: the editor's background and text, line numbers, and one color per token kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeTheme {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    pub background: u32,
    pub foreground: u32,
    pub line_number: u32,
    /// In `TokenKind` order: keyword, string, comment, number, type, function, macro, attribute, property.
    pub tokens: [u32; 9],
}

/// A color with an alpha, for the translucent tints the code views draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tint {
    pub rgb: u32,
    pub alpha: f32,
}

const fn t(id: &'static str, name: &'static str, dark: bool, background: u32, foreground: u32, line_number: u32, tokens: [u32; 9]) -> CodeTheme {
    CodeTheme { id, name, dark, background, foreground, line_number, tokens }
}

impl CodeTheme {
    pub const ALL: &'static [CodeTheme] = &[
        // Dark
        t("xcode-dark", "Xcode Default (Dark)", true, 0x1F1F24, 0xDFDFE0, 0x747478, [0xFF7AB2, 0xFF8170, 0x7F8C98, 0xD9C97C, 0x6BDFFF, 0x67B7A4, 0xFD8F3F, 0xBF8555, 0x4EB0CC]),
        t("vscode-dark", "VS Code Dark Modern", true, 0x1F1F1F, 0xCCCCCC, 0x6E7681, [0x569CD6, 0xCE9178, 0x6A9955, 0xB5CEA8, 0x4EC9B0, 0xDCDCAA, 0xC586C0, 0x4EC9B0, 0x9CDCFE]),
        t("jetbrains-dark", "JetBrains Dark", true, 0x1E1F22, 0xBCBEC4, 0x4B5059, [0xCF8E6D, 0x6AAB73, 0x7A7E85, 0x2AACB8, 0xBCBEC4, 0x56A8F5, 0x56A8F5, 0xB3AE60, 0xC77DBB]),
        t("darcula", "JetBrains Darcula", true, 0x2B2B2B, 0xA9B7C6, 0x606366, [0xCC7832, 0x6A8759, 0x808080, 0x6897BB, 0xA9B7C6, 0xFFC66D, 0x9876AA, 0xBBB529, 0x9876AA]),
        t("github-dark", "GitHub Dark", true, 0x0D1117, 0xE6EDF3, 0x6E7681, [0xFF7B72, 0xA5D6FF, 0x8B949E, 0x79C0FF, 0xFFA657, 0xD2A8FF, 0xD2A8FF, 0x79C0FF, 0x79C0FF]),
        t("one-dark", "One Dark", true, 0x282C34, 0xABB2BF, 0x495162, [0xC678DD, 0x98C379, 0x7F848E, 0xD19A66, 0xE5C07B, 0x61AFEF, 0x56B6C2, 0xD19A66, 0xE06C75]),
        t("dracula", "Dracula", true, 0x282A36, 0xF8F8F2, 0x6272A4, [0xFF79C6, 0xF1FA8C, 0x6272A4, 0xBD93F9, 0x8BE9FD, 0x50FA7B, 0xFFB86C, 0x50FA7B, 0x8BE9FD]),
        t("monokai", "Monokai", true, 0x272822, 0xF8F8F2, 0x90908A, [0xF92672, 0xE6DB74, 0x88846F, 0xAE81FF, 0x66D9EF, 0xA6E22E, 0xA6E22E, 0xA6E22E, 0xFD971F]),
        t("tokyo-night", "Tokyo Night", true, 0x1A1B26, 0xA9B1D6, 0x3B4261, [0xBB9AF7, 0x9ECE6A, 0x565F89, 0xFF9E64, 0x2AC3DE, 0x7AA2F7, 0x7DCFFF, 0xE0AF68, 0x73DACA]),
        t("catppuccin-mocha", "Catppuccin Mocha", true, 0x1E1E2E, 0xCDD6F4, 0x6C7086, [0xCBA6F7, 0xA6E3A1, 0x9399B2, 0xFAB387, 0xF9E2AF, 0x89B4FA, 0x94E2D5, 0xFAB387, 0xB4BEFE]),
        t("nord", "Nord", true, 0x2E3440, 0xD8DEE9, 0x4C566A, [0x81A1C1, 0xA3BE8C, 0x616E88, 0xB48EAD, 0x8FBCBB, 0x88C0D0, 0x5E81AC, 0xD08770, 0x8FBCBB]),
        t("solarized-dark", "Solarized Dark", true, 0x002B36, 0x839496, 0x586E75, [0x859900, 0x2AA198, 0x586E75, 0xD33682, 0xB58900, 0x268BD2, 0xCB4B16, 0x6C71C4, 0x268BD2]),
        // Light
        t("xcode-light", "Xcode Default (Light)", false, 0xFFFFFF, 0x000000, 0xA6A6A6, [0x9B2393, 0xC41A16, 0x5D6C79, 0x1C00CF, 0x0B4F79, 0x326D74, 0x643820, 0x815F03, 0x0F68A0]),
        t("vscode-light", "VS Code Light Modern", false, 0xFFFFFF, 0x3B3B3B, 0x6E7681, [0x0000FF, 0xA31515, 0x008000, 0x098658, 0x267F99, 0x795E26, 0xAF00DB, 0x267F99, 0x001080]),
        t("intellij-light", "IntelliJ Light", false, 0xFFFFFF, 0x080808, 0xAEB3C2, [0x0033B3, 0x067D17, 0x8C8C8C, 0x1750EB, 0x080808, 0x00627A, 0x00627A, 0x9E880D, 0x871094]),
        t("github-light", "GitHub Light", false, 0xFFFFFF, 0x1F2328, 0x8C959F, [0xCF222E, 0x0A3069, 0x6E7781, 0x0550AE, 0x953800, 0x8250DF, 0x8250DF, 0x0550AE, 0x0550AE]),
        t("one-light", "One Light", false, 0xFAFAFA, 0x383A42, 0x9D9D9F, [0xA626A4, 0x50A14F, 0xA0A1A7, 0x986801, 0xC18401, 0x4078F2, 0x0184BC, 0x986801, 0xE45649]),
        t("catppuccin-latte", "Catppuccin Latte", false, 0xEFF1F5, 0x4C4F69, 0x8C8FA1, [0x8839EF, 0x40A02B, 0x7C7F93, 0xFE640B, 0xDF8E1D, 0x1E66F5, 0x179299, 0xFE640B, 0x7287FD]),
        t("solarized-light", "Solarized Light", false, 0xFDF6E3, 0x657B83, 0x93A1A1, [0x859900, 0x2AA198, 0x93A1A1, 0xD33682, 0xB58900, 0x268BD2, 0xCB4B16, 0x6C71C4, 0x268BD2]),
    ];

    /// The theme with `id`, or Xcode's for the appearance when there's none (a stored id from a newer version).
    pub fn named(id: &str, dark: bool) -> &'static CodeTheme {
        let fallback = if dark { "xcode-dark" } else { "xcode-light" };
        Self::ALL.iter().find(|t| t.id == id).or_else(|| Self::ALL.iter().find(|t| t.id == fallback)).unwrap()
    }

    pub fn token(&self, k: TokenKind) -> u32 {
        self.tokens[k as usize]
    }

    // Diff colors are the same in every theme, as translucent tints over its background.
    pub fn added(&self) -> u32 {
        if self.dark { 0x2EA043 } else { 0x1A7F37 }
    }
    pub fn removed(&self) -> u32 {
        if self.dark { 0xF85149 } else { 0xCF222E }
    }

    /// Every color the code views draw, as CodePalette resolved them.
    pub fn palette(&self) -> CodePalette {
        let d = self.dark;
        let tint = |rgb, alpha| Tint { rgb, alpha };
        CodePalette {
            background: tint(self.background, 1.0),
            foreground: tint(self.foreground, 1.0),
            meta_foreground: tint(self.foreground, 0.6),
            line_number: tint(self.line_number, 1.0),
            add_mark: tint(self.added(), 1.0),
            del_mark: tint(self.removed(), 1.0),
            add_bg: tint(self.added(), if d { 0.17 } else { 0.12 }),
            del_bg: tint(self.removed(), if d { 0.17 } else { 0.11 }),
            add_hl: tint(self.added(), if d { 0.4 } else { 0.3 }),
            del_hl: tint(self.removed(), if d { 0.4 } else { 0.28 }),
            gutter_bg: tint(self.foreground, 0.03),
            hunk_bg: tint(self.token(TokenKind::Function), if d { 0.14 } else { 0.09 }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CodePalette {
    pub background: Tint,
    pub foreground: Tint,
    pub meta_foreground: Tint,
    pub line_number: Tint,
    pub add_mark: Tint,
    pub del_mark: Tint,
    pub add_bg: Tint,
    pub del_bg: Tint,
    pub add_hl: Tint,
    pub del_hl: Tint,
    pub gutter_bg: Tint,
    pub hunk_bg: Tint,
}

/// A theme for light and for dark, a font and a line height, as an editor ships them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub light: &'static str,
    pub dark: &'static str,
    /// "" → the system monospaced font (SF Mono)
    pub font: &'static str,
    pub size: f64,
    /// × the font size
    pub line_height: f64,
}

impl EditorProfile {
    pub const ALL: &'static [EditorProfile] = &[
        EditorProfile { id: "xcode", name: "Xcode", light: "xcode-light", dark: "xcode-dark", font: "", size: 12.0, line_height: 1.5 },
        EditorProfile { id: "vscode", name: "VS Code", light: "vscode-light", dark: "vscode-dark", font: "Menlo", size: 12.0, line_height: 1.5 },
        EditorProfile { id: "jetbrains", name: "JetBrains", light: "intellij-light", dark: "jetbrains-dark", font: "JetBrains Mono", size: 13.0, line_height: 1.45 },
        EditorProfile { id: "github", name: "GitHub", light: "github-light", dark: "github-dark", font: "", size: 12.0, line_height: 1.65 },
    ];
}

/// The editor's look as the Swift app persisted it (UserDefaults keys and defaults), so an install keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorSettings {
    pub light_theme: String,
    pub dark_theme: String,
    pub font_family: String,
    pub font_size: f64,
    pub line_height_factor: f64,
}

impl EditorSettings {
    pub const KEY_LIGHT_THEME: &'static str = "editorLightTheme";
    pub const KEY_DARK_THEME: &'static str = "editorDarkTheme";
    pub const KEY_FONT: &'static str = "editorFont";
    pub const KEY_FONT_SIZE: &'static str = "editorFontSize";
    pub const KEY_LINE_HEIGHT: &'static str = "editorLineHeight";
    /// The ranges the settings pane allows.
    pub const FONT_SIZES: std::ops::RangeInclusive<f64> = 9.0..=24.0;
    pub const LINE_HEIGHTS: std::ops::RangeInclusive<f64> = 1.2..=2.0;

    pub fn theme(&self, dark: bool) -> &'static CodeTheme {
        CodeTheme::named(if dark { &self.dark_theme } else { &self.light_theme }, dark)
    }

    /// The preset these settings match exactly, if any ("Custom" otherwise).
    pub fn profile(&self) -> Option<&'static EditorProfile> {
        EditorProfile::ALL.iter().find(|p| {
            p.light == self.light_theme && p.dark == self.dark_theme && p.font == self.font_family && p.size == self.font_size && p.line_height == self.line_height_factor
        })
    }

    pub fn apply(&mut self, p: &EditorProfile) {
        *self = Self { light_theme: p.light.into(), dark_theme: p.dark.into(), font_family: p.font.into(), font_size: p.size, line_height_factor: p.line_height };
    }

    /// Row height in points: font size × factor, rounded.
    pub fn line_height(&self) -> f64 {
        (self.font_size * self.line_height_factor).round()
    }
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self { light_theme: "xcode-light".into(), dark_theme: "xcode-dark".into(), font_family: String::new(), font_size: 12.0, line_height_factor: 1.5 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_and_profiles() {
        assert_eq!(CodeTheme::named("nope", true).id, "xcode-dark");
        assert_eq!(CodeTheme::named("dracula", true).token(TokenKind::Keyword), 0xFF79C6);
        for p in EditorProfile::ALL {
            assert!(CodeTheme::ALL.iter().any(|t| t.id == p.light && !t.dark) && CodeTheme::ALL.iter().any(|t| t.id == p.dark && t.dark));
        }
        let mut s = EditorSettings::default();
        assert_eq!(s.profile().map(|p| p.id), Some("xcode"));
        s.apply(&EditorProfile::ALL[2]);
        assert_eq!((s.profile().map(|p| p.id), s.line_height()), (Some("jetbrains"), 19.0));
    }
}

//! Settings › Editor (EditorTheme.swift's EditorSettingsView): preset, light and dark code themes, font, size, line
//! height and a preview. Saved under the Swift keys; the dark theme drives every diff's syntax colors at once.
use std::sync::{Arc, OnceLock};

use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, HighlightStyle, Hsla, IntoElement, ParentElement as _, StatefulInteractiveElement as _,
    SharedString, Styled as _, StyledText, Window, div, font, prelude::FluentBuilder as _, px,
    component::slider::{Slider, SliderEvent, SliderState},
};
use loupe_code::theme::{CodeTheme, EditorProfile, EditorSettings};
use loupe_forge::settings as prefs;

use super::{Settings, foot, picker, row};
use crate::{diff, state::adapt, theme, ui};

pub fn load() -> EditorSettings {
    let p = prefs::shared();
    let d = EditorSettings::default();
    EditorSettings {
        light_theme: p.string(EditorSettings::KEY_LIGHT_THEME).unwrap_or(d.light_theme),
        dark_theme: p.string(EditorSettings::KEY_DARK_THEME).unwrap_or(d.dark_theme),
        font_family: p.string(EditorSettings::KEY_FONT).unwrap_or(d.font_family),
        font_size: p.f64(EditorSettings::KEY_FONT_SIZE).unwrap_or(d.font_size),
        line_height_factor: p.f64(EditorSettings::KEY_LINE_HEIGHT).unwrap_or(d.line_height_factor),
    }
}

fn save(s: &EditorSettings) {
    let mut p = prefs::shared();
    p.set_string(EditorSettings::KEY_LIGHT_THEME, &s.light_theme);
    p.set_string(EditorSettings::KEY_DARK_THEME, &s.dark_theme);
    p.set_string(EditorSettings::KEY_FONT, &s.font_family);
    p.set_f64(EditorSettings::KEY_FONT_SIZE, s.font_size);
    p.set_f64(EditorSettings::KEY_LINE_HEIGHT, s.line_height_factor);
}

pub fn line_slider(value: f64, cx: &mut Context<Settings>) -> Entity<SliderState> {
    let s = cx.new(|_| SliderState::new().min(1.2).max(2.0).step(0.05).default_value(value as f32));
    cx.subscribe(&s, |me: &mut Settings, _, e: &SliderEvent, cx| {
        let v = match e { SliderEvent::Change(v) | SliderEvent::Release(v) => v.start() };
        let v = ((v as f64) * 20.).round() / 20.; // 0.05 steps, stored exactly like the Swift slider's
        if v != me.editor.line_height_factor { me.set_editor(|s| s.line_height_factor = v, cx) }
    }).detach();
    s
}

impl Settings {
    /// Every change saves, recolors the app's diffs (the dark theme) and redraws every window.
    pub(super) fn set_editor(&mut self, f: impl FnOnce(&mut EditorSettings), cx: &mut Context<Self>) {
        f(&mut self.editor);
        save(&self.editor);
        adapt::apply_code_theme();
        cx.refresh_windows();
    }

    pub(super) fn editor_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.editor.clone();
        let me = cx.entity();
        let fonts = families(window);
        let installed = s.font_family.is_empty() || fonts.contains(&s.font_family);
        // The slider follows a preset or a restore too.
        if (self.line_slider.read(cx).value().start() as f64 - s.line_height_factor).abs() > 0.001 {
            let v = s.line_height_factor as f32;
            self.line_slider.update(cx, |sl, cx| sl.set_value(v, window, cx));
        }

        let preset_label = s.profile().map_or("Custom", |p| p.name);
        let preset = {
            let (me, custom) = (me.clone(), s.profile().is_none());
            let cur = s.profile().map(|p| p.id);
            picker("preset", preset_label, move |mut m, _, _| {
                for p in EditorProfile::ALL {
                    let me = me.clone();
                    m = m.item(ui_item(p.name, cur == Some(p.id)).on_click(move |_, _, cx| me.update(cx, |v, cx| v.set_editor(|s| s.apply(p), cx))));
                }
                if custom { m = m.separator().item(ui_item("Custom", true)) }
                m
            })
        };
        let theme_picker = |id: &'static str, dark: bool| {
            let me = me.clone();
            let cur = s.theme(dark).id;
            picker(id, s.theme(dark).name, move |mut m, _, _| {
                for t in CodeTheme::ALL.iter().filter(|t| t.dark == dark) {
                    let me = me.clone();
                    m = m.item(gpui_kit::component::menu::PopupMenuItem::element(move |_, _| theme_label(t)).checked(t.id == cur)
                        .on_click(move |_, _, cx| me.update(cx, |v, cx| v.set_editor(|s| if dark { s.dark_theme = t.id.into() } else { s.light_theme = t.id.into() }, cx))));
                }
                m
            })
        };
        let font_label = if s.font_family.is_empty() { "SF Mono (System)".to_string() }
                         else if installed { s.font_family.clone() } else { format!("{} (not installed)", s.font_family) };
        let font_picker = {
            let (me, cur, fonts) = (me.clone(), s.font_family.clone(), fonts.clone());
            picker("font", font_label, move |mut m, _, _| {
                let set = |name: String| { let me = me.clone(); move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| { let n = name.clone(); me.update(cx, |v, cx| v.set_editor(|s| s.font_family = n, cx)) } };
                m = m.item(ui_item("SF Mono (System)", cur.is_empty()).on_click(set(String::new())));
                if !installed { m = m.item(ui_item(format!("{cur} (not installed)"), true)) }
                m = m.separator();
                for f in fonts.iter() { m = m.item(ui_item(f.clone(), *f == cur).on_click(set(f.clone()))) }
                m.max_h(px(420.)).scrollable(true)
            })
        };
        let step = |id: &'static str, icon: &str, d: f64, cx: &mut Context<Self>| {
            let ok = EditorSettings::FONT_SIZES.contains(&(s.font_size + d));
            ui::icon_button(id, icon).when(!ok, |b| b.opacity(0.35))
                .when(ok, |b| b.on_click(cx.listener(move |v, _, _, cx| v.set_editor(|s| s.font_size += d, cx))))
        };

        div().flex().flex_col().gap_4()
            .child(ui::card()
                .child(row("Preset", preset))
                .child(row("Light theme", theme_picker("light-theme", false)))
                .child(row("Dark theme", theme_picker("dark-theme", true)))
                .child(foot("Code follows the app's appearance: the light theme in Light mode, the dark one in Dark mode. Loupe is dark-only for now, so diffs use the dark theme.")))
            .child(ui::card()
                .child(row("Font", font_picker))
                .child(row("Size", div().flex().items_center().gap_1()
                    .child(step("size-down", "minus", -1., cx))
                    .child(div().w(px(44.)).text_center().child(format!("{} pt", s.font_size)))
                    .child(step("size-up", "plus", 1., cx))))
                .child(row("Line height", div().flex().items_center().gap_3()
                    .child(div().w(px(180.)).child(Slider::new(&self.line_slider)))
                    .child(div().w(px(44.)).text_right().text_color(theme::text_muted()).child(format!("{:.2}×", s.line_height_factor)))))
                .when(!installed, |c| c.child(foot(format!("{} isn't installed, so code uses SF Mono. Install it from its site (JetBrains Mono: jetbrains.com/lp/mono) and reopen this pane.", s.font_family))))
                .child(foot("Font, size and line height apply to this preview; the app's diffs keep their built-in font until the renderer reads them.")))
            .child(ui::card()
                .child(ui::card_header("Preview", ""))
                .child(div().p_3().child(preview(&s, if installed && !s.font_family.is_empty() { s.font_family.clone() } else { system_mono(&fonts) }))))
    }
}

fn ui_item(label: impl Into<SharedString>, checked: bool) -> gpui_kit::component::menu::PopupMenuItem {
    gpui_kit::component::menu::PopupMenuItem::new(label).checked(checked)
}

/// A theme's name with a strip of its colors (ThemeLabel).
fn theme_label(t: &CodeTheme) -> impl IntoElement {
    div().flex().items_center().gap_3().w(px(260.))
        .child(div().flex_1().child(t.name))
        .child(div().flex().rounded(px(2.)).overflow_hidden()
            .children(std::iter::once(t.background).chain(t.tokens.iter().copied().take(6)).map(|c| div().w(px(8.)).h(px(12.)).bg(rgb(c, 1.)))))
}

fn rgb(v: u32, alpha: f32) -> Hsla { let mut c: Hsla = gpui_kit::rgb(v).into(); c.a = alpha; c }

/// Fixed-pitch families installed here, measured once: 'i' and 'W' advance the same.
fn families(window: &mut Window) -> Arc<Vec<String>> {
    static F: OnceLock<Arc<Vec<String>>> = OnceLock::new();
    F.get_or_init(|| {
        let ts = window.text_system();
        let mut v: Vec<String> = ts.all_font_names().into_iter().filter(|n| !n.starts_with('.')).filter(|n| {
            let id = ts.resolve_font(&font(n.clone()));
            matches!((ts.advance(id, px(12.), 'i'), ts.advance(id, px(12.), 'W')), (Ok(a), Ok(b)) if a.width > px(0.) && a.width == b.width)
        }).collect();
        v.sort();
        v.dedup();
        Arc::new(v)
    }).clone()
}

/// What "SF Mono (System)" draws with: SF Mono where it's installed as a family, else the app's code font.
fn system_mono(fonts: &[String]) -> String {
    if fonts.iter().any(|f| f == "SF Mono") { "SF Mono".into() } else { theme::CODE_FONT.into() }
}

const PREVIEW: &str = "diff --git a/Sources/Cache.swift b/Sources/Cache.swift
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

";

/// The preview diff and its syntax, parsed once.
fn preview_file() -> &'static (diff::FileDiff, diff::Syntax) {
    static P: OnceLock<(diff::FileDiff, diff::Syntax)> = OnceLock::new();
    P.get_or_init(|| {
        let f = adapt::file_diff(&loupe_core::diff::parse(PREVIEW).remove(0));
        let syn = (adapt::highlighter())(&f);
        (f, syn)
    })
}

/// A short diff drawn in the chosen theme, font, size and line height (EditorPreview), 9 rows tall.
fn preview(s: &EditorSettings, family: String) -> impl IntoElement {
    let (f, syn) = preview_file();
    let t = s.theme(true);
    let pal = t.palette();
    let tint = |x: loupe_code::theme::Tint| rgb(x.rgb, x.alpha);
    let lh = px(s.line_height() as f32);
    let size = px(s.font_size as f32);
    let num = |n: Option<u32>| div().w(px(34.)).pr_2().text_right().child(n.map(|n| n.to_string()).unwrap_or_default());
    div().h(lh * 9. + px(2.)).overflow_hidden().rounded(px(6.)).bg(rgb(t.background, 1.))
        .font_family(SharedString::from(family)).text_size(size).line_height(lh)
        .children((0..f.lines.len()).map(|i| {
            let l = &f.lines[i];
            let text = f.line_text(i).to_string();
            match l.kind {
                diff::LineKind::Hunk => div().h(lh).px_3().flex().items_center().bg(tint(pal.hunk_bg)).text_color(tint(pal.meta_foreground))
                    .font_family(theme::UI_FONT).font_weight(FontWeight::NORMAL).child(text).into_any_element(),
                _ => {
                    let (bg, mark) = match l.kind {
                        diff::LineKind::Added => (Some(tint(pal.add_bg)), "+"),
                        diff::LineKind::Removed => (Some(tint(pal.del_bg)), "−"),
                        _ => (None, ""),
                    };
                    let hl: Vec<_> = syn.get(i).into_iter().flatten()
                        .filter(|sp| (sp.range.end as usize) <= text.len())
                        .map(|sp| (sp.range.start as usize..sp.range.end as usize, HighlightStyle { color: Some(rgb(t.tokens[sp.kind as usize % 9], 1.)), ..Default::default() }))
                        .collect();
                    div().h(lh).flex().items_center().whitespace_nowrap().when_some(bg, |d, c| d.bg(c))
                        .child(div().flex().flex_none().h_full().items_center().bg(tint(pal.gutter_bg)).text_color(tint(pal.line_number))
                            .child(num(l.old_line)).child(num(l.new_line)))
                        .child(div().flex_none().w(px(18.)).text_center().text_color(if mark == "+" { tint(pal.add_mark) } else { tint(pal.del_mark) }).child(mark))
                        .child(div().flex_none().text_color(rgb(t.foreground, 1.)).child(StyledText::new(text).with_highlights(hl)))
                        .into_any_element()
                }
            }
        }))
}

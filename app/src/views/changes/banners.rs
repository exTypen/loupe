//! The bars above the split (CH-08): an operation in progress (`merge::banner`) or else the pull request offer
//! (`pr_create::banner`), git-lfs missing (LF-02), uninitialized submodules (SM-06).
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Entity, FontWeight, Hsla, IntoElement, StatefulInteractiveElement as _, ParentElement as _, Styled as _, div,
    prelude::FluentBuilder as _, px,
};
use loupe_core::{lfs, submodules};
use loupe_forge::settings;

use super::ChangesView;
use crate::state::WorktreeModel;
use crate::{theme, ui};

fn bar(icon: &str, color: Hsla, title: String, caption: String) -> gpui_kit::Div {
    div().flex().items_center().gap_2p5().px_3().py_2().flex_none().bg(color.opacity(0.08)).border_b_1().border_color(theme::border())
        .child(ui::icon(icon).text_color(color))
        .child(div().flex().flex_col().flex_1().min_w_0()
            .child(div().truncate().font_weight(FontWeight::MEDIUM).child(title))
            .when(!caption.is_empty(), |d| d.child(div().truncate().text_size(px(11.5)).text_color(theme::text_muted()).child(caption))))
}

fn small(p: gpui_kit::Stateful<gpui_kit::Div>) -> gpui_kit::Stateful<gpui_kit::Div> { p.h(px(24.)).px_2p5().text_size(px(12.)) }

impl ChangesView {
    pub(crate) fn render_banners(&mut self, m: &Entity<WorktreeModel>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = vec![];
        // CH-08: the operation banner (merge's, MG-01..03) wins; otherwise the pull request offer (PC-01..04).
        match crate::views::merge::banner(m, cx) {
            Some(b) => out.push(b),
            None => out.extend(crate::views::pr_create::banner(m, cx)),
        }
        let s = &m.read(cx).state;
        if s.lfs.used && !lfs::installed() && !settings::shared().bool("lfsMissingBannerDismissed") {
            out.push(bar("hard-drive", theme::accent(), "This repository uses Git LFS, but git-lfs isn't installed".into(),
                    format!("Large files stay as pointers until it is. In Terminal: {}", lfs::INSTALL_HINT))
                .child(small(ui::pill("lfs-copy", "Copy Command")).on_click(|_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(lfs::INSTALL_HINT.into()))))
                .child(small(ui::pill("lfs-dismiss", "Dismiss")).on_click(cx.listener(|_, _, _, cx| { settings::shared().set_bool("lfsMissingBannerDismissed", true); cx.notify() })))
                .into_any_element());
        }
        let u = &s.uninitialized_submodules;
        if !u.is_empty() && !s.submodule_banner_hidden {
            let k = u.len();
            let title = if k == 1 { "A submodule isn't initialized".to_string() } else { format!("{k} submodules aren't initialized") };
            let caption = u.iter().take(3).cloned().collect::<Vec<_>>().join(", ") + &if k > 3 { format!(" and {} more", k - 3) } else { String::new() };
            let busy = s.busy.is_some();
            out.push(bar("package", theme::blue(), title, caption)
                .child(small(ui::pill("sub-later", "Not Now")).on_click(cx.listener(|v, _, _, cx| if let Some(m) = v.selected() { m.update(cx, |m, cx| { m.state.submodule_banner_hidden = true; cx.notify() }) })))
                .child(small(ui::pill_primary("sub-init", "Initialize", theme::blue())).when(busy, |d| d.opacity(0.4))
                    .when(!busy, |d| d.on_click(cx.listener(|v, _, _, cx| {
                        let Some(m) = v.selected() else { return };
                        let env = { let w = m.read(cx); v.app.read(cx).git_auth(w.remote.as_ref(), &w.path_str()) };
                        m.update(cx, |m, cx| m.perform("Initializing submodules", move |p| submodules::init(p, &env), cx).detach())
                    }))))
                .into_any_element());
        }
        out
    }
}

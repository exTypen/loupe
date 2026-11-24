//! Avatars (PullRequestsView.swift's Avatar and AvatarCache): a circular image, or a coloured circle with the
//! login's first letter while it loads or when it can't be fetched. One request per URL however many rows ask
//! (loupe_forge::avatar coalesces and bounds the bytes); the decoded gpui images are kept here, bounded too.
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, EntityId, FontWeight, Image, ImageFormat, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, hsla, img, px,
};
use loupe_forge::User;

use crate::theme;

const KEEP: usize = 500;

enum Slot {
    Loading(Vec<EntityId>),
    Ready(Option<Arc<Image>>),
}

#[derive(Default)]
struct Cache {
    slots: HashMap<String, Slot>,
    order: VecDeque<String>,
}

thread_local! { static CACHE: RefCell<Cache> = RefCell::new(Cache::default()); }

fn format_of(b: &[u8]) -> Option<ImageFormat> {
    Some(match b {
        [0x89, b'P', b'N', b'G', ..] => ImageFormat::Png,
        [0xff, 0xd8, ..] => ImageFormat::Jpeg,
        [b'G', b'I', b'F', ..] => ImageFormat::Gif,
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => ImageFormat::Webp,
        _ if b.starts_with(b"<svg") || b.starts_with(b"<?xml") => ImageFormat::Svg,
        _ => return None,
    })
}

/// The image for `url` if it's here; otherwise starts loading it (once) and re-renders `requester` when it lands.
fn image(url: &str, requester: EntityId, cx: &mut App) -> Option<Arc<Image>> {
    let start = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        match c.slots.get_mut(url) {
            Some(Slot::Ready(i)) => return Err(i.clone()),
            Some(Slot::Loading(w)) => { if !w.contains(&requester) { w.push(requester) } Ok(false) }
            None => { c.slots.insert(url.into(), Slot::Loading(vec![requester])); Ok(true) }
        }
    });
    let start = match start { Err(i) => return i, Ok(s) => s };
    if start {
        let u = url.to_string();
        cx.spawn(async move |cx| {
            let key = u.clone();
            let got = cx.background_executor().spawn(async move {
                let bytes = loupe_forge::avatar::load(&u)?;
                let f = format_of(&bytes)?;
                Some(Arc::new(Image::from_bytes(f, bytes.to_vec())))
            }).await;
            let waiting = CACHE.with(|c| {
                let mut c = c.borrow_mut();
                let w = match c.slots.insert(key.clone(), Slot::Ready(got)) { Some(Slot::Loading(w)) => w, _ => vec![] };
                c.order.push_back(key);
                while c.order.len() > KEEP { if let Some(old) = c.order.pop_front() { c.slots.remove(&old); } }
                w
            });
            cx.update(|cx| for id in waiting { cx.notify(id) });
        }).detach();
    }
    None
}

/// The hue the Swift app gave each person, at saturation 0.45 and brightness 0.75 (HSB → HSL).
fn initials_color(login: &str) -> gpui_kit::Hsla {
    let (s, v) = (0.45f32, 0.75f32);
    let l = v * (1. - s / 2.);
    let sl = if l == 0. || l == 1. { 0. } else { (v - l) / l.min(1. - l) };
    hsla(loupe_forge::avatar::initials_hue(login), sl, l, 1.)
}

/// A `size` px avatar with the login as tooltip. `owner` is the view that re-renders once the image arrives.
pub fn avatar(user: &User, size: f32, owner: EntityId, cx: &mut App) -> AnyElement {
    let url = user.avatar_url.clone().filter(|u| !u.is_empty());
    let image = url.and_then(|u| image(&u, owner, cx));
    let login: SharedString = user.login.clone().into();
    let tip = login.clone();
    let base = div().id(SharedString::from(format!("av-{}-{size}", user.login))).flex_none().size(px(size)).rounded_full()
        .overflow_hidden().border_1().border_color(theme::border_strong())
        .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx));
    match image {
        Some(i) => base.child(img(i).size_full().rounded_full()).into_any_element(),
        None => base.flex().items_center().justify_center().bg(initials_color(&login))
            .text_size(px(size * 0.45)).font_weight(FontWeight::SEMIBOLD).text_color(gpui_kit::white())
            .child(login.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default())
            .into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_image_formats() {
        assert_eq!(format_of(&[0x89, b'P', b'N', b'G', 0]), Some(ImageFormat::Png));
        assert_eq!(format_of(&[0xff, 0xd8, 0xff]), Some(ImageFormat::Jpeg));
        assert_eq!(format_of(b"<html>"), None);
    }
}

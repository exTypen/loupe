//! One row's slice of the commit graph (History.swift's GraphCell): rows stack with no gap, so the slices join into
//! lines. Painted, not laid out: a few strokes per row.
use gpui_kit::{Bounds, Hsla, IntoElement, PathBuilder, Styled as _, canvas, fill, point, px, size};
use loupe_core::history::GraphRow;

use crate::theme;

pub const LANE: f32 = 11.;
pub const NODE_Y: f32 = 21.;

/// Lane 0, the main line, is the accent; the others cycle (SwiftUI's green, purple, pink, teal, yellow, red, indigo,
/// mint, cyan, brown, orange).
pub fn lane_color(i: usize) -> Hsla {
    if i == 0 { return theme::accent() }
    const P: [u32; 11] = [0x34c759, 0xaf52de, 0xff2d55, 0x30b0c7, 0xffcc00, 0xff3b30, 0x5856d6, 0x00c7be, 0x32ade6, 0xa2845e, 0xff9500];
    gpui_kit::rgb(P[(i - 1) % P.len()]).into()
}

pub fn cell(row: GraphRow, lanes: usize) -> impl IntoElement {
    let lanes = row.width().min(lanes);
    canvas(|_, _, _| {}, move |b, _, window, _| {
        let x = |i: usize| b.left() + px(i as f32 * LANE + LANE / 2.);
        let (top, bottom, y, c) = (b.top(), b.bottom(), b.top() + px(NODE_Y), x(row.col));
        let mut stroke = |build: &dyn Fn(&mut PathBuilder), color: Hsla| {
            let mut p = PathBuilder::stroke(px(1.6));
            build(&mut p);
            if let Ok(path) = p.build() { window.paint_path(path, color) }
        };
        for &i in row.through.iter().filter(|&&i| i < lanes) {
            stroke(&|p| { p.move_to(point(x(i), top)); p.line_to(point(x(i), bottom)) }, lane_color(i));
        }
        if row.col >= lanes { return } // past the lane cap: lines only
        for &i in row.into.iter().filter(|&&i| i < lanes) {
            stroke(&|p| { p.move_to(point(x(i), top)); p.curve_to(point(c, y), point(x(i), y)) }, lane_color(i));
        }
        for &j in row.out.iter().filter(|&&j| j < lanes) {
            stroke(&|p| { p.move_to(point(c, y)); p.curve_to(point(x(j), bottom), point(x(j), y)) }, lane_color(j));
        }
        // The node: an 8pt dot with a ring in the canvas colour, so lines passing under it break cleanly.
        let dot = |r: f32, color: Hsla| fill(Bounds::new(point(c - px(r), y - px(r)), size(px(2. * r), px(2. * r))), color).corner_radii(px(r));
        window.paint_quad(dot(5.5, theme::canvas()));
        window.paint_quad(dot(4., lane_color(row.col)));
    }).w(px(lanes as f32 * LANE)).flex_none()
}

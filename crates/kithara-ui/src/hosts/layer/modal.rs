use crate::{
    draw::{Pt, Rect, Rgba},
    hosts::solve,
    render::Skin,
};

/// The two corner ticks a framed box carries, top-left and bottom-right, as
/// the four bars that draw them, in the box's own coordinates.
pub(crate) fn tick_marks(bounds: solve::Size, size: f32, width: f32, offset: f32) -> [Rect; 4] {
    let far_x = (bounds.width - offset - width).max(0.0);
    let far_y = (bounds.height - offset - width).max(0.0);
    let tail_x = (bounds.width - offset - size).max(0.0);
    let tail_y = (bounds.height - offset - size).max(0.0);
    let bar = |x, y, w, h| Rect { x, y, w, h };
    [
        bar(offset, offset, size, width),
        bar(offset, offset, width, size),
        bar(tail_x, far_y, size, width),
        bar(far_x, tail_y, width, size),
    ]
}

/// What a modal draws and where, the same for both hosts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ModalChrome {
    pub(crate) background: Rgba,
    pub(crate) border: Rgba,
    pub(crate) scrim: Rgba,
    pub(crate) shadow: Rgba,
    pub(crate) tick: Rgba,
    pub(crate) shadow_offset: Pt,
    pub(crate) blur: f32,
    pub(crate) border_width: f32,
    pub(crate) radius: f32,
    tick_offset: f32,
    tick_size: f32,
    tick_width: f32,
}

impl ModalChrome {
    pub(crate) fn new(skin: &Skin) -> Self {
        let modal = skin.modal;
        let chrome = skin.chrome;
        Self {
            background: skin.rgba(modal.background),
            border: skin.rgba(modal.frame.border),
            scrim: Rgba {
                a: modal.scrim_alpha,
                ..skin.rgba(modal.scrim)
            },
            shadow: Rgba {
                a: modal.shadow.alpha,
                ..skin.rgba(modal.shadow.color)
            },
            tick: skin.rgba(chrome.corner_color),
            shadow_offset: Pt {
                x: modal.shadow.offset_x,
                y: modal.shadow.offset_y,
            },
            blur: modal.shadow.blur,
            border_width: modal.frame.border_width,
            radius: modal.frame.radius,
            tick_offset: chrome.corner_offset,
            tick_size: chrome.corner_size,
            tick_width: chrome.corner_width,
        }
    }

    /// The largest content the window holds: the window less the frame and
    /// the reach of the shadow on every side, so the whole shadow stays in it.
    pub(crate) fn room(self, viewport: solve::Size) -> solve::Size {
        let reach_x = self.blur + self.shadow_offset.x.abs() + self.border_width;
        let reach_y = self.blur + self.shadow_offset.y.abs() + self.border_width;
        solve::Size::new(
            (viewport.width - reach_x * 2.0).max(0.0),
            (viewport.height - reach_y * 2.0).max(0.0),
        )
    }

    /// The framed surface around content of this size, centred on the window.
    pub(crate) fn surface(self, content: solve::Size, viewport: solve::Size) -> Rect {
        let w = content.width + self.border_width * 2.0;
        let h = content.height + self.border_width * 2.0;
        Rect {
            x: ((viewport.width - w) / 2.0).round(),
            y: ((viewport.height - h) / 2.0).round(),
            w,
            h,
        }
    }

    /// Where content stands inside its surface.
    pub(crate) fn content(self, surface: Rect) -> Pt {
        Pt {
            x: surface.x + self.border_width,
            y: surface.y + self.border_width,
        }
    }

    /// The bars of the surface's corner ticks, in window coordinates.
    pub(crate) fn ticks(self, surface: Rect) -> [Rect; 4] {
        tick_marks(
            solve::Size::new(surface.w, surface.h),
            self.tick_size,
            self.tick_width,
            self.tick_offset,
        )
        .map(|bar| Rect {
            x: surface.x + bar.x,
            y: surface.y + bar.y,
            ..bar
        })
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::builtin;

    #[kithara::test]
    fn a_surface_centres_on_whole_pixels_in_an_odd_window() {
        let chrome = ModalChrome::new(builtin::skin());
        let surface = chrome.surface(
            solve::Size::new(100.0, 60.0),
            solve::Size::new(481.0, 320.0),
        );

        assert_eq!(
            surface,
            Rect {
                x: 190.0,
                y: 129.0,
                w: 102.0,
                h: 62.0,
            }
        );
    }
}

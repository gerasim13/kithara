use kithara_platform::time::Duration;

use super::{Repaint, Size2, SizeLimits, TextMeasurer, widget::CustomWidget};
use crate::{
    draw::{DrawListBuilder, Rect},
    interact::{Hit, Input, Outcome},
};

/// One mounted custom widget, with its own action vocabulary already mapped to
/// whatever the host that holds it speaks.
pub(crate) trait MountedCustom<Action> {
    fn measure(&mut self, text: &mut TextMeasurer<'_>, limits: SizeLimits) -> Size2;

    fn input(&mut self, input: Input<'_>, hit: Hit) -> Outcome<Action>;

    #[cfg(feature = "masonry")]
    fn accepts_text_input(&self) -> bool;

    fn frame(&mut self, elapsed: Duration) -> Option<Action>;

    fn paint(&mut self, list: &mut DrawListBuilder, text: &mut TextMeasurer<'_>, bounds: Rect);

    fn repaint(&self) -> Repaint;
}

pub(crate) struct MappedCustom<Widget, Map> {
    widget: Widget,
    map: Map,
}

impl<Widget, Map> MappedCustom<Widget, Map> {
    pub(crate) const fn new(widget: Widget, map: Map) -> Self {
        Self { widget, map }
    }
}

impl<Action, Widget, Map> MountedCustom<Action> for MappedCustom<Widget, Map>
where
    Widget: CustomWidget,
    Map: Fn(Widget::Action) -> Action,
{
    delegate::delegate! {
        to self.widget {
            #[cfg(feature = "masonry")]
            fn accepts_text_input(&self) -> bool;
            fn measure(&mut self, text: &mut TextMeasurer<'_>, limits: SizeLimits) -> Size2;
            #[expr($.map(&self.map))]
            fn input(&mut self, input: Input<'_>, hit: Hit) -> Outcome<Action>;
            #[expr($.map(&self.map))]
            fn frame(&mut self, elapsed: Duration) -> Option<Action>;
            fn paint(&mut self, list: &mut DrawListBuilder, text: &mut TextMeasurer<'_>, bounds: Rect);
            fn repaint(&self) -> Repaint;
        }
    }
}

impl<Action> MountedCustom<Action> for Box<dyn MountedCustom<Action>> {
    delegate::delegate! {
        to (**self) {
            #[cfg(feature = "masonry")]
            fn accepts_text_input(&self) -> bool;
            fn measure(&mut self, text: &mut TextMeasurer<'_>, limits: SizeLimits) -> Size2;
            fn input(&mut self, input: Input<'_>, hit: Hit) -> Outcome<Action>;
            fn frame(&mut self, elapsed: Duration) -> Option<Action>;
            fn paint(&mut self, list: &mut DrawListBuilder, text: &mut TextMeasurer<'_>, bounds: Rect);
            fn repaint(&self) -> Repaint;
        }
    }
}

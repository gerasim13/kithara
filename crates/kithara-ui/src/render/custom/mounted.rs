use kithara_platform::time::Duration;

use super::{Repaint, Size2, SizeLimits, TextMeasurer, widget::CustomWidget};
use crate::{
    draw::{DrawListBuilder, Rect},
    interact::{Hit, Input, Outcome},
    render::skin::CustomSkin,
};

pub(crate) struct MappedCustom<Widget, Map> {
    map: Map,
    widget: Widget,
}

impl<Widget, Map> MappedCustom<Widget, Map> {
    pub(crate) const fn new(widget: Widget, map: Map) -> Self {
        Self { map, widget }
    }
}

impl<Action, Widget, Map> CustomWidget for MappedCustom<Widget, Map>
where
    Widget: CustomWidget,
    Action: std::fmt::Debug + Send + 'static,
    Map: Fn(Widget::Action) -> Action + 'static,
{
    type Action = Action;
    delegate::delegate! {
        to self.widget {
            #[cfg(feature = "masonry")]
            fn accepts_text_input(&self) -> bool;
            fn measure(&mut self, text: &mut TextMeasurer<'_>, limits: SizeLimits) -> Size2;
            #[expr($.map(&self.map))]
            fn input(&mut self, input: Input<'_>, hit: Hit) -> Outcome<Action>;
            #[expr($.map(&self.map))]
            fn frame(&mut self, elapsed: Duration) -> Option<Action>;
            fn paint(
                &mut self,
                list: &mut DrawListBuilder,
                text: &mut TextMeasurer<'_>,
                bounds: Rect,
                skin: &CustomSkin,
            );
            fn repaint(&self) -> Repaint;
        }
    }
}

impl<Widget: CustomWidget + ?Sized> CustomWidget for Box<Widget> {
    type Action = Widget::Action;
    delegate::delegate! {
        to (**self) {
            #[cfg(feature = "masonry")]
            fn accepts_text_input(&self) -> bool;
            fn measure(&mut self, text: &mut TextMeasurer<'_>, limits: SizeLimits) -> Size2;
            fn input(&mut self, input: Input<'_>, hit: Hit) -> Outcome<Self::Action>;
            fn frame(&mut self, elapsed: Duration) -> Option<Self::Action>;
            fn paint(
                &mut self,
                list: &mut DrawListBuilder,
                text: &mut TextMeasurer<'_>,
                bounds: Rect,
                skin: &CustomSkin,
            );
            fn repaint(&self) -> Repaint;
        }
    }
}

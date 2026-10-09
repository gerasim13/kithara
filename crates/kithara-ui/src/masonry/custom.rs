use kithara_platform::time::Duration;

use crate::{
    draw::{DrawListBuilder, Rect},
    interact::{Hit, Input, Outcome},
    render::{
        custom::{CustomWidget, Repaint, Size2, SizeLimits, TextMeasurer},
        skin::CustomSkin,
    },
};

/// Re-speaks one mounted widget in another host's action vocabulary.
pub(crate) struct Respoken<Inner, Map, From> {
    inner: Inner,
    map: Map,
    spoken: std::marker::PhantomData<fn(From)>,
}

impl<Inner, Map, From> Respoken<Inner, Map, From> {
    pub(crate) const fn new(inner: Inner, map: Map) -> Self {
        Self {
            inner,
            map,
            spoken: std::marker::PhantomData,
        }
    }
}

impl<From, To, Inner, Map> CustomWidget for Respoken<Inner, Map, From>
where
    Inner: std::ops::DerefMut<Target = dyn CustomWidget<Action = From>> + 'static,
    Map: Fn(From) -> To + 'static,
    From: std::fmt::Debug + Send + 'static,
    To: std::fmt::Debug + Send + 'static,
{
    type Action = To;

    delegate::delegate! {
        to self.inner {
            fn accepts_text_input(&self) -> bool;
            fn measure(&mut self, text: &mut TextMeasurer<'_>, limits: SizeLimits) -> Size2;
            #[expr($.map(&self.map))]
            fn input(&mut self, input: Input<'_>, hit: Hit) -> Outcome<To>;
            #[expr($.map(&self.map))]
            fn frame(&mut self, elapsed: Duration) -> Option<To>;
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

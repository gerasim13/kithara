use kithara_platform::time::Duration;
use masonry::core::ErasedAction;

pub(crate) use crate::render::custom::{MappedCustom, MountedCustom, Repaint};
use crate::{
    draw::{DrawListBuilder, Rect},
    interact::{Hit, Input, Outcome},
    render::{
        CustomSkin,
        custom::{Size2, SizeLimits, TextMeasurer},
    },
};

/// One action on its way out of this host, still typed but no longer named.
#[derive(Debug)]
pub(crate) struct HostAction(ErasedAction);

impl HostAction {
    pub(crate) fn new<Action>(action: Action) -> Self
    where
        Action: std::fmt::Debug + Send + 'static,
    {
        Self(Box::new(action))
    }

    pub(crate) fn downcast<Action>(self) -> Result<Action, Self>
    where
        Action: std::fmt::Debug + Send + 'static,
    {
        self.0.downcast().map(|action| *action).map_err(Self)
    }

    delegate::delegate! {
        to self.0 {
            pub(crate) fn type_name(&self) -> &'static str;
        }
    }
}

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

impl<From, To, Inner, Map> MountedCustom<To> for Respoken<Inner, Map, From>
where
    Inner: MountedCustom<From>,
    Map: Fn(From) -> To,
{
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

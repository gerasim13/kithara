use crate::{
    draw::{Pt, Rect},
    interact::{Input, Outcome},
    render::{HostLayer, WindowCommand},
};
#[cfg(any(feature = "iced", test))]
use crate::{
    shaping::TextResources,
    solve::{Length, Size},
};

pub(crate) trait WindowLayerProgram {
    type State: Default + 'static;

    fn hit_layer(&self, state: &Self::State, bounds: Rect) -> HostLayer<WindowCommand> {
        self.layer(state, bounds, None)
    }

    fn layer(
        &self,
        state: &Self::State,
        bounds: Rect,
        pointer: Option<Pt>,
    ) -> HostLayer<WindowCommand>;

    #[cfg(any(feature = "iced", test))]
    fn resources(&self) -> Option<&TextResources>;

    #[cfg(any(feature = "iced", test))]
    fn size(&self) -> Size<Length>;

    fn update(
        &self,
        _state: &mut Self::State,
        input: Input<'_>,
        layer: &HostLayer<WindowCommand>,
        pointer: Option<Pt>,
    ) -> (Outcome<WindowCommand>, bool) {
        (layer.handle(input, pointer), false)
    }
}

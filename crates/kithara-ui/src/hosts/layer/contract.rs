use crate::{
    draw::{Pt, Rect},
    hosts::layer::HostLayer,
    interact::{Input, Outcome},
    render::WindowCommand,
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

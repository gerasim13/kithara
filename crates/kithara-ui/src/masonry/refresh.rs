use crate::{
    atoms::{painter::ControlPainter, wave::face::Drawn},
    hosts::controls::{DataRefresh, Draws, Reading},
    masonry::retained::{
        MasonryHost, MasonryNode,
        mount::{Cx, NodeControl, refreshing},
    },
    mount::deck::wave::host::Wave,
};

/// The half of a drawn control only a host that keeps its widgets needs.
///
/// An immediate host asks the document again every frame and never holds a
/// leaf across two. A retained one mounts the leaf once, so a control whose
/// picture follows more than its own endpoint says here how that leaf steps
/// itself afterwards, given the endpoint its reading came from.
pub(crate) trait Refresh: Draws {
    fn refresh(
        &self,
        read: Reading<'_>,
        endpoint: Option<&str>,
    ) -> DataRefresh<<Self::Painter as ControlPainter>::Data>;
}

impl Refresh for Wave<'_> {
    fn refresh(&self, read: Reading<'_>, _endpoint: Option<&str>) -> DataRefresh<Drawn> {
        let scope = read.scope.to_owned();
        let zoom = self
            .zoom
            .map(|binding| read.ctx.ui.resolve(binding.key).to_owned());
        Box::new(move |data, ctx| data.refresh(&ctx, &scope, zoom.as_deref()))
    }
}

impl NodeControl for Wave<'_> {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        refreshing(self, host, cx)
    }
}

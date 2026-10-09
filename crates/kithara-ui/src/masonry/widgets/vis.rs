use crate::{
    hosts::vis::VisFrame,
    render::{ReadValue, document::Ctx},
};

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct VisLeaf {
    #[field(get, vis = "pub(crate)", copy)]
    frame: Option<VisFrame>,
    preset: Option<String>,
}

impl VisLeaf {
    pub(crate) fn new(
        preset: Option<String>,
        value: Option<ReadValue<'_>>,
        ctx: Ctx<'_, '_>,
    ) -> Self {
        Self {
            preset,
            frame: VisFrame::read(value, &ctx),
        }
    }

    pub(crate) fn refresh(&mut self, ctx: Ctx<'_, '_>) -> bool {
        let value = self.preset.as_deref().and_then(|preset| ctx.get(preset));
        let frame = VisFrame::read(value, &ctx);
        self.frame != frame && {
            self.frame = frame;
            true
        }
    }
}

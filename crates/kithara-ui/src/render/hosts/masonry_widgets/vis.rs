use crate::render::{ReadValue, document::Ctx, vis::VisFrame};

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(in crate::render) struct VisLeaf {
    #[field(get, vis = "pub(in crate::render)", copy)]
    frame: Option<VisFrame>,
    preset: Option<String>,
}

impl VisLeaf {
    pub(in crate::render) fn new(
        preset: Option<String>,
        value: Option<ReadValue<'_>>,
        ctx: Ctx<'_, '_>,
    ) -> Self {
        Self {
            preset,
            frame: VisFrame::read(value, &ctx),
        }
    }

    pub(in crate::render) fn refresh(&mut self, ctx: Ctx<'_, '_>) -> bool {
        let value = self.preset.as_deref().and_then(|preset| ctx.get(preset));
        let frame = VisFrame::read(value, &ctx);
        self.frame != frame && {
            self.frame = frame;
            true
        }
    }
}

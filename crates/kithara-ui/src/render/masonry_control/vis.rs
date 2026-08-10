use crate::render::{ReadValue, Reads, vis::VisFrame};

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(super) struct VisLeaf {
    #[field(get, vis = "pub(super)", copy)]
    frame: Option<VisFrame>,
    preset: Option<String>,
}

impl VisLeaf {
    pub(super) fn new(
        preset: Option<String>,
        value: Option<ReadValue<'_>>,
        reads: &dyn Reads,
    ) -> Self {
        Self {
            frame: VisFrame::read(value, reads),
            preset,
        }
    }

    pub(super) fn refresh(&mut self, reads: &dyn Reads) -> bool {
        let value = self.preset.as_deref().and_then(|preset| reads.get(preset));
        let frame = VisFrame::read(value, reads);
        self.frame != frame && {
            self.frame = frame;
            true
        }
    }
}

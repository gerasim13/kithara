use crate::{
    draw::Rect,
    hosts::{layer::HostLayer, picker::PickerMenu},
    render::Skin,
    shaping::TextContext,
};

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct PickerPaint<'a> {
    #[field(get, vis = "pub(crate)", copy)]
    skin: &'a Skin,
    #[field(get, vis = "pub(crate)")]
    selected: Option<usize>,
    items: Vec<&'a str>,
}

impl<'a> PickerPaint<'a> {
    pub(crate) const fn new(items: Vec<&'a str>, selected: Option<usize>, skin: &'a Skin) -> Self {
        Self {
            skin,
            selected,
            items,
        }
    }

    pub(crate) const fn item_count(&self) -> usize {
        self.items.len()
    }

    pub(crate) const fn item_height(&self) -> f32 {
        self.skin.tree.scope_item_height
    }

    pub(crate) fn popup_layer(
        &self,
        text: &mut TextContext,
        anchor: Rect,
        highlighted: Option<usize>,
    ) -> HostLayer<usize> {
        PickerMenu::new(self.skin).layer(text, anchor, self.items.iter().copied(), highlighted)
    }
}
#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{builtin, draw::DrawCmd, hosts::picker::picker_hits};
    #[kithara::test]
    fn popup_commands_are_a_separate_unclipped_frame_list() {
        let skin = builtin::skin();
        let paint = PickerPaint::new(vec!["ZVUK", "LOCAL"], Some(0), skin);
        let mut text = TextContext::from(skin.text_resources.as_ref());
        let bounds = Rect {
            h: skin.tree.scope_item_height,
            w: 72.0,
            x: 0.0,
            y: 0.0,
        };
        let popup = paint.popup_layer(&mut text, bounds, Some(1));

        assert!(
            popup
                .draw()
                .commands()
                .iter()
                .all(|command| !matches!(command, DrawCmd::Clip { .. })),
            "the fresh overlay frame must receive an unclipped popup list"
        );
        assert!(popup.draw().commands().iter().any(|command| {
            matches!(command, DrawCmd::Text { content, .. } if content == "LOCAL")
        }));
        assert!(matches!(
            popup.draw().commands(),
            [
                DrawCmd::Fill { .. },
                DrawCmd::Text { .. },
                DrawCmd::Fill { .. },
                DrawCmd::Text { .. },
                DrawCmd::Stroke { .. },
            ]
        ));
        assert_eq!(popup.hits(), picker_hits(bounds, paint.item_height(), 2));
    }
}

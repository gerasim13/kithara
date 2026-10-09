use crate::{
    atoms::tree::Tree,
    draw::{DrawList, DrawListBuilder, Rect},
    shaping::TextContext,
};

impl Tree {
    pub(crate) fn row_commands(
        &self,
        text: &mut TextContext,
        viewport: Rect,
        offset: f32,
        hovered: Option<usize>,
    ) -> DrawList {
        let mut list = DrawListBuilder::default();
        self.paint_rows(&mut list, text, viewport, offset, hovered);
        list.finish()
    }
}
#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        draw::{DrawCmd, Geom},
        module::IconName,
        render::TreeRow,
    };

    fn rows() -> [TreeRow<'static>; 3] {
        [
            TreeRow {
                depth: 0,
                label: "First",
                icon: IconName::Folder,
                count: None,
                expanded: Some(true),
                page: false,
                selected: false,
                muted: false,
            },
            TreeRow {
                depth: 1,
                label: "Second",
                icon: IconName::Playlist,
                count: Some(2),
                expanded: None,
                page: false,
                selected: true,
                muted: false,
            },
            TreeRow {
                depth: 1,
                label: "Third",
                icon: IconName::Zvuk,
                count: None,
                expanded: None,
                page: false,
                selected: false,
                muted: true,
            },
        ]
    }

    fn commands(offset: f32, viewport: Rect) -> DrawList {
        let skin = builtin::skin();
        let picture = Tree::new(&rows(), None, skin);
        let mut text = TextContext::from(skin.text_resources.as_ref());
        picture.row_commands(&mut text, viewport, offset, None)
    }

    #[kithara::test]
    fn scrolled_rows_are_nested_under_the_viewport_clip() {
        let skin = builtin::skin();
        let viewport = Rect {
            h: 48.0,
            w: 180.0,
            x: 0.0,
            y: 0.0,
        };
        let list = commands(skin.tree.row_height / 2.0, viewport);
        let Some(DrawCmd::Clip { region, list }) = list.commands().first() else {
            panic!("the retained tree must start with its scoped viewport clip");
        };

        assert_eq!(*region, viewport);
        assert!(list.commands().iter().any(|command| {
            matches!(
                command,
                DrawCmd::Fill {
                    geom: Geom::Rect(Rect { y, .. }),
                    ..
                } if *y < viewport.y
            )
        }));
    }

    #[kithara::test]
    fn offset_changes_the_retained_row_positions() {
        let viewport = Rect {
            h: 48.0,
            w: 180.0,
            x: 0.0,
            y: 0.0,
        };

        assert_ne!(
            commands(0.0, viewport),
            commands(builtin::skin().tree.row_height, viewport)
        );
    }

    #[kithara::test]
    fn rows_fully_outside_the_viewport_are_not_retained() {
        let viewport = Rect {
            h: builtin::skin().tree.row_height,
            w: 180.0,
            x: 0.0,
            y: 0.0,
        };
        let list = commands(0.0, viewport);
        let Some(DrawCmd::Clip { list, .. }) = list.commands().first() else {
            panic!("the tree painter must retain a clip");
        };

        assert!(list.commands().iter().all(|command| {
            !matches!(
                command,
                DrawCmd::Text { content, .. } if content == "Second" || content == "Third"
            )
        }));
    }

    #[kithara::test]
    fn content_past_the_bottom_is_scoped_by_the_viewport_clip() {
        let skin = builtin::skin();
        let viewport = Rect {
            h: skin.tree.row_height * 1.5,
            w: 180.0,
            x: 0.0,
            y: 0.0,
        };
        let list = commands(0.0, viewport);
        let Some(DrawCmd::Clip { region, list }) = list.commands().first() else {
            panic!("overflowing Tree rows must remain inside a viewport clip");
        };

        assert_eq!(*region, viewport);
        assert!(list.commands().iter().any(|command| {
            matches!(command, DrawCmd::Text { content, .. } if content == "Second")
        }));
    }

    #[kithara::test]
    fn the_zvuk_row_stays_on_the_neutral_geometry_seam() {
        let skin = builtin::skin();
        let picture = Tree::new(&rows()[2..], None, skin);
        let mut text = TextContext::from(skin.text_resources.as_ref());
        let list = picture.row_commands(
            &mut text,
            Rect {
                h: skin.tree.row_height,
                w: 180.0,
                x: 0.0,
                y: 0.0,
            },
            0.0,
            None,
        );
        let Some(DrawCmd::Clip { list, .. }) = list.commands().first() else {
            panic!("the tree painter must retain a clip");
        };

        assert!(list.commands().iter().any(|command| {
            matches!(
                command,
                DrawCmd::Stroke {
                    geom: Geom::RoundedRect { .. } | Geom::Arc { .. },
                    ..
                }
            )
        }));
    }
}

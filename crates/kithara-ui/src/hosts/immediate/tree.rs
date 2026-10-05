use iced::{
    Background, Element, Length, Padding,
    widget::{Column, Space, container, container::Style as ContainerStyle},
};

use crate::{
    atoms::tree::face::Tree as TreeFace,
    render::{IcedSkin, InputOwner, Published, ReadValue, Skin, Widget, search_input, tree_rows},
};

#[derive(bon::Builder)]
pub(crate) struct Tree<'path, 'query, 'value, 'data, 'skin> {
    skin: &'skin Skin,
    path: &'path str,
    query: Option<&'query str>,
    toggle: bool,
    owner: InputOwner,
    value: Option<&'value ReadValue<'data>>,
}

impl<'a, 'skin: 'a> Widget<'a> for Tree<'_, '_, '_, '_, 'skin> {
    fn view(self) -> Element<'a, Published> {
        let Some(ReadValue::Tree(rows)) = self.value else {
            return Space::new().into();
        };
        let picture = TreeFace::new(rows, self.query, self.skin);
        let toggle = self.toggle.then(|| format!("{}/toggle", self.path));
        let tree = tree_rows(self.path, toggle, picture, self.owner);
        let panel = container(tree)
            .padding(Padding {
                top: self.skin.tree.panel_padding_top,
                right: 0.0,
                bottom: self.skin.tree.panel_padding_bottom,
                left: 0.0,
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .style({
                let background = self.skin.color(self.skin.tree.panel_background);
                move |_| ContainerStyle::default().background(Background::Color(background))
            });

        let search = self.query.map(|query| {
            search_bar(
                &format!("{}/search", self.path),
                query,
                self.skin,
                self.owner,
            )
        });
        Column::new()
            .push(search)
            .push(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

pub(in crate::hosts) fn search_bar<'a>(
    path: &str,
    query: &str,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, Published> {
    container(search_input(path, query, skin, owner))
        .width(Length::Fill)
        .height(Length::Fixed(skin.tree.search_height))
        .into()
}

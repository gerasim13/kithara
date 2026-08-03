use iced::{
    Alignment, Background, Element, Length, Padding,
    alignment::{Horizontal, Vertical},
    widget::{Row, Space, column, container, container::Style as ContainerStyle, row},
};

use crate::{
    render::{
        Icon, InputOwner, ReadValue, Skin, UiEvent, fonts, scope_picker, search_input, shaped_text,
        tree_rows,
    },
    widgets::Widget,
};

#[derive(bon::Builder)]
pub(crate) struct Tree<'path, 'query, 'value, 'data, 'skin> {
    skin: &'skin Skin,
    path: &'path str,
    query: &'query str,
    value: Option<&'value ReadValue<'data>>,
    owner: InputOwner,
}

impl<'a, 'skin: 'a> Widget<'a> for Tree<'_, '_, '_, '_, 'skin> {
    fn view(self) -> Element<'a, UiEvent> {
        let Some(ReadValue::Tree(rows)) = self.value else {
            return Space::new().into();
        };
        let tree = tree_rows(self.path, rows, self.skin, self.owner);
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

        column![
            search_bar(self.path, self.query, self.skin, self.owner),
            panel
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

#[derive(bon::Builder)]
pub(crate) struct ContextBar<'a, 'scope_value, 'value, 'data, 'skin> {
    skin: &'skin Skin,
    path: &'a str,
    scope_value: Option<&'scope_value ReadValue<'data>>,
    value: Option<&'value ReadValue<'data>>,
    scope_items: Vec<&'a str>,
    owner: InputOwner,
}

impl<'a, 'skin: 'a> Widget<'a> for ContextBar<'a, '_, '_, '_, 'skin> {
    fn view(self) -> Element<'a, UiEvent> {
        let Some(ReadValue::Text(label)) = self.value else {
            return Space::new().into();
        };
        let content_height = self.skin.tree.context_height - self.skin.tree.context_divider_width;
        let icon = Icon::Zvuk.view(self.skin.tree.context_icon_size, self.skin.palette.text);
        let breadcrumb = shaped_text((*label).to_owned())
            .font(fonts::mono(self.skin.tree.context_text.weight))
            .size(self.skin.tree.context_text.size)
            .color(self.skin.palette.text_dim);
        let row = if self.scope_items.is_empty() {
            Row::new()
                .push(icon)
                .push(breadcrumb)
                .spacing(self.skin.tree.context_gap)
        } else {
            Row::new()
                .push(icon)
                .push(scope_picker(
                    self.path,
                    self.scope_items,
                    self.scope_value,
                    self.skin,
                    self.owner,
                ))
                .push(
                    shaped_text("\u{203a}")
                        .font(fonts::mono(self.skin.tree.scope_text.weight))
                        .size(self.skin.tree.scope_text.size)
                        .color(self.skin.color(self.skin.tree.scope_chevron_color)),
                )
                .push(breadcrumb)
                .spacing(self.skin.tree.scope_gap)
        }
        .align_y(Alignment::Center);
        let content = container(row)
            .padding([0.0, self.skin.tree.context_padding_x])
            .width(Length::Fill)
            .height(Length::Fixed(content_height))
            .align_y(Vertical::Center)
            .style({
                let background = self.skin.color(self.skin.tree.context_background);
                move |_| ContainerStyle::default().background(Background::Color(background))
            });
        let divider = container(Space::new())
            .width(Length::Fill)
            .height(Length::Fixed(self.skin.tree.context_divider_width))
            .style({
                let color = self.skin.color(self.skin.tree.context_divider);
                move |_| ContainerStyle::default().background(Background::Color(color))
            });

        column![content, divider]
            .width(Length::Fill)
            .height(Length::Fixed(self.skin.tree.context_height))
            .into()
    }
}

fn search_bar<'a>(
    path: &str,
    query: &str,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let icon = container(Icon::Search.view(skin.tree.search_icon_size, skin.palette.muted))
        .width(Length::Fixed(skin.tree.search_icon_width))
        .height(Length::Fill)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center)
        .style({
            let background = skin.color(skin.tree.search_background);
            move |_| ContainerStyle::default().background(Background::Color(background))
        });
    let input = search_input(&format!("{path}/search"), query, skin, owner);

    container(row![icon, input].spacing(1).height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(skin.tree.search_height))
        .style({
            let divider = skin.color(skin.tree.search_divider);
            move |_| ContainerStyle::default().background(Background::Color(divider))
        })
        .into()
}

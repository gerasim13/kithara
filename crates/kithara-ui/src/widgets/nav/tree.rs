use std::fmt;

use iced::{
    Alignment, Background, Border, Element, Length, Padding, Pixels, Shadow, Theme,
    alignment::{Horizontal, Vertical},
    widget::{
        Row, Space, column, container, container::Style as ContainerStyle, overlay::menu,
        pick_list, row, text_input, text_input::Style as TextInputStyle,
    },
};
use num_traits::ToPrimitive;

use crate::{
    render::{
        ControlAction, Icon, InputOwner, ReadValue, Skin, UiEvent, control_event, fonts,
        shaped_text, tree_rows,
    },
    widgets::Widget,
};

#[derive(bon::Builder)]
pub(crate) struct Tree<'path, 'query, 'value, 'data, 'skin> {
    path: &'path str,
    query: &'query str,
    value: Option<&'value ReadValue<'data>>,
    owner: InputOwner,
    skin: &'skin Skin,
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

        column![search_bar(self.query, self.skin), panel]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

#[derive(bon::Builder)]
pub(crate) struct ContextBar<'a, 'scope_value, 'value, 'data, 'skin> {
    path: &'a str,
    scope_items: Vec<&'a str>,
    scope_value: Option<&'scope_value ReadValue<'data>>,
    value: Option<&'value ReadValue<'data>>,
    skin: &'skin Skin,
}

impl<'a> Widget<'a> for ContextBar<'a, '_, '_, '_, '_> {
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

#[derive(Clone, Copy, PartialEq)]
struct ScopeOption<'a> {
    index: usize,
    label: &'a str,
}

impl fmt::Display for ScopeOption<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label)
    }
}

fn scope_picker<'a>(
    path: &'a str,
    items: Vec<&'a str>,
    value: Option<&ReadValue<'_>>,
    skin: &Skin,
) -> Element<'a, UiEvent> {
    let options: Vec<_> = items
        .into_iter()
        .enumerate()
        .map(|(index, label)| ScopeOption { index, label })
        .collect();
    let last = options.len().saturating_sub(1);
    let selected = match value {
        Some(ReadValue::Scalar(value)) => {
            let index = value.round().to_usize().map_or(0, |index| index.min(last));
            options.get(index).copied()
        }
        _ => None,
    };
    let padding_y = ((skin.tree.scope_item_height - skin.tree.scope_text.size) / 2.0).max(0.0);
    pick_list(options, selected, move |option| {
        control_event(path, ControlAction::SelectIndex(option.index))
    })
    .padding(Padding {
        top: padding_y,
        right: skin.tree.scope_padding_x,
        bottom: padding_y,
        left: skin.tree.scope_padding_x,
    })
    .text_size(skin.tree.scope_text.size)
    .text_line_height(Pixels(skin.tree.scope_text.size))
    .font(fonts::mono(skin.tree.scope_text.weight))
    .handle(pick_list::Handle::Arrow {
        size: Some(Pixels(skin.tree.scope_chevron_size)),
    })
    .style({
        let background = skin.color(skin.tree.scope_background);
        let text = skin.color(skin.tree.scope_text_color);
        let chevron = skin.color(skin.tree.scope_chevron_color);
        let border = skin.border(skin.tree.scope_frame);
        move |_theme: &Theme, _status| pick_list::Style {
            text_color: text,
            placeholder_color: text,
            handle_color: chevron,
            background: Background::Color(background),
            border,
        }
    })
    .menu_style({
        let background = skin.color(skin.tree.scope_menu_background);
        let text = skin.color(skin.tree.scope_menu_text);
        let selected_background = skin.color(skin.tree.scope_selected_background);
        let selected_text = skin.color(skin.tree.scope_selected_text);
        let border = skin.border(skin.tree.scope_menu_frame);
        move |_theme: &Theme| menu::Style {
            background: Background::Color(background),
            border,
            text_color: text,
            selected_text_color: selected_text,
            selected_background: Background::Color(selected_background),
            shadow: Shadow::default(),
        }
    })
    .into()
}

fn search_bar(query: &str, skin: &Skin) -> Element<'static, UiEvent> {
    let icon = container(Icon::Search.view(skin.tree.search_icon_size, skin.palette.muted))
        .width(Length::Fixed(skin.tree.search_icon_width))
        .height(Length::Fill)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center)
        .style({
            let background = skin.color(skin.tree.search_background);
            move |_| ContainerStyle::default().background(Background::Color(background))
        });
    let padding_y = ((skin.tree.search_height - skin.tree.search_text.size) / 2.0).max(0.0);
    let input = text_input(&skin.tree.search_placeholder, query)
        .on_input(UiEvent::LibraryQuery)
        .padding(Padding {
            top: padding_y,
            right: skin.tree.search_padding_x,
            bottom: padding_y,
            left: skin.tree.search_padding_x,
        })
        .font(fonts::sans(skin.tree.search_text.weight))
        .size(skin.tree.search_text.size)
        .line_height(Pixels(skin.tree.search_text.size))
        .width(Length::Fill)
        .style({
            let background = skin.color(skin.tree.search_background);
            let palette = skin.palette;
            move |_theme: &Theme, _status| TextInputStyle {
                background: Background::Color(background),
                border: Border::default(),
                icon: palette.muted,
                placeholder: palette.muted,
                value: palette.text,
                selection: palette.accent_soft,
            }
        });

    container(row![icon, input].spacing(1).height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(skin.tree.search_height))
        .style({
            let divider = skin.color(skin.tree.search_divider);
            move |_| ContainerStyle::default().background(Background::Color(divider))
        })
        .into()
}

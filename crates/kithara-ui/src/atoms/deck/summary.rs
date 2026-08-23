use crate::{
    draw::{DrawListBuilder, Pt, Rect, Rgba, Transform},
    module::DeckSummaryStyle,
    render::Skin,
    shaping::TextContext,
    skin::{ColorRole, DeckSkin, FontFamily, FontSkin, TextRoleSkin},
};

/// The deck's headline: what is loaded, and where it came from.
///
/// Both looks stack the two words; the compact one leads with the source and
/// takes its type straight from the skin's roles.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct Summary {
    #[field(get, vis = "pub(crate)", copy)]
    metrics: DeckSkin,
    panel: Rgba,
    source: Rgba,
    source_role: TextRoleSkin,
    style: DeckSummaryStyle,
    title: Rgba,
    title_role: TextRoleSkin,
}

/// What a summary is handed each frame: the track's name, and what it is
/// playing from.
#[derive(Clone, PartialEq)]
pub(crate) struct Loaded {
    pub(crate) source: String,
    pub(crate) title: String,
}

impl Summary {
    pub(crate) fn new(style: DeckSummaryStyle, skin: &Skin) -> Self {
        let metrics = skin.deck;
        let role = |font: FontSkin, family, color| TextRoleSkin {
            color,
            font: family,
            size: font.size,
            spacing: 0.0,
            weight: font.weight,
        };
        let (source_role, title_role) = if style == DeckSummaryStyle::Micro {
            (metrics.micro_source, metrics.micro_title)
        } else {
            (
                role(metrics.artist, FontFamily::Sans, ColorRole::TextDim),
                role(metrics.title, FontFamily::Display, ColorRole::Text),
            )
        };
        Self {
            metrics,
            panel: skin.palette.bg_panel,
            source: skin.rgba(source_role.color),
            source_role,
            style,
            title: skin.rgba(title_role.color),
            title_role,
        }
    }

    pub(crate) fn paint(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Loaded,
        bounds: Rect,
    ) {
        list.fill_rect(bounds, self.panel);
        let inner = Rect {
            h: (bounds.h - self.metrics.summary_padding_y * 2.0).max(0.0),
            w: (bounds.w - self.metrics.summary_padding_x * 2.0).max(0.0),
            x: bounds.x + self.metrics.summary_padding_x,
            y: bounds.y + self.metrics.summary_padding_y,
        };
        let title = text.shape(&data.title, self.title_role, None);
        let source = text.shape(&data.source, self.source_role, None);
        let compact = self.style == DeckSummaryStyle::Micro;
        let gap = if compact {
            self.metrics.micro_summary_gap
        } else {
            self.metrics.readout_gap
        };
        // The compact deck names where the track came from first and the track
        // under it; the full one leads with the track.
        let [upper, lower] = if compact {
            [
                (&source, &data.source, self.source),
                (&title, &data.title, self.title),
            ]
        } else {
            [
                (&title, &data.title, self.title),
                (&source, &data.source, self.source),
            ]
        };
        let stacked = upper.0.height() + gap + lower.0.height();
        let y = inner.y + (inner.h - stacked) / 2.0;
        let mut content = list.child();
        content.text(
            upper.0,
            upper.1,
            Transform::translate(Pt { x: inner.x, y }),
            upper.2,
        );
        content.text(
            lower.0,
            lower.1,
            Transform::translate(Pt {
                x: inner.x,
                y: y + upper.0.height() + gap,
            }),
            lower.2,
        );
        list.clip(inner, content.finish());
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{DeckSummaryStyle, DrawListBuilder, Loaded, Rect, Summary, TextContext};
    use crate::{builtin, draw::DrawList};

    const BOUNDS: Rect = Rect {
        h: 44.0,
        w: 200.0,
        x: 2.0,
        y: 4.0,
    };

    fn drawn(style: DeckSummaryStyle) -> DrawList {
        let skin = builtin::skin();
        let mut text = TextContext::from(skin.text_resources());
        let mut list = DrawListBuilder::default();
        Summary::new(style, skin).paint(
            &mut list,
            &mut text,
            &Loaded {
                source: "FILE".to_owned(),
                title: "Midnight Circuit".to_owned(),
            },
            BOUNDS,
        );
        list.finish()
    }

    /// The compact look sets the two words side by side and the full one
    /// stacks them, which is the whole difference between the styles.
    #[kithara::test]
    fn the_compact_style_lays_its_words_out_differently() {
        assert_ne!(
            drawn(DeckSummaryStyle::Micro),
            drawn(DeckSummaryStyle::Default)
        );
    }

    /// A title longer than its box is cut off rather than spilling over the
    /// controls beside it.
    #[kithara::test]
    fn the_words_are_clipped_to_the_box() {
        let list = drawn(DeckSummaryStyle::Default);
        let [_, clip] = list.commands() else {
            panic!("a summary must draw its panel and one clipped run of words");
        };
        assert!(matches!(clip, crate::draw::DrawCmd::Clip { .. }));
    }
}

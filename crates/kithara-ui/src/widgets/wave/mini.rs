use std::{
    cell::{Cell, RefCell},
    hash::{DefaultHasher, Hash, Hasher},
};

use iced::{
    Element, Event, Length, Rectangle, Renderer, Size, Theme,
    keyboard::{Event as KeyboardEvent, Modifiers},
    mouse::{self, Cursor},
    widget::canvas::{self, Action, Canvas, Frame, Geometry},
};
use kithara_platform::time::Instant;
use num_traits::cast::AsPrimitive;

use crate::{
    backends::IcedBackend,
    draw::{DrawList, DrawListBuilder, Rect, Rgba, replay},
    interact::{
        CursorShape, Hover, Input, iced as iced_interact,
        recognizers::{Scalar, ScalarState, Track},
    },
    module::WaveStyle,
    render::{ReadValue, Reads, Skin, UiEvent, WaveformView, model::derived, scalar, scalar_child},
    text::TextContext,
    widgets::{
        Widget,
        wave::{
            overlay::{Overlay, OverlayPalette},
            paint::{WavePaint, WavePalette},
            snapshot::{OverlayData, WaveformData},
            zoom_math::{clamp_zoom, window_bounds, x_to_norm, zoom_for_wheel},
        },
    },
};

#[derive(bon::Builder)]
pub(crate) struct MiniWave<'path, 'value, 'data, 'scope, 'reads, 'skin> {
    path: &'path str,
    style: WaveStyle,
    badge: Option<&'path str>,
    value: Option<&'value ReadValue<'data>>,
    scope: &'scope str,
    reads: &'reads dyn Reads,
    skin: &'skin Skin,
    zoom: f32,
}

impl<'a, 'skin: 'a> Widget<'a> for MiniWave<'_, '_, '_, '_, '_, 'skin> {
    fn view(self) -> Element<'a, UiEvent> {
        let path = self.path.to_owned();
        let paint = self.paint();
        let show_beats = paint.hero();
        let drag = Scalar::builder()
            .track(if show_beats {
                Track::RelativeHorizontal {
                    scale: paint.zoom,
                    value: paint.progress,
                }
            } else {
                Track::HorizontalClick
            })
            .hover(Hover::new(if show_beats {
                CursorShape::Grab
            } else {
                CursorShape::Pointer
            }))
            .build();
        Canvas::new(MiniWaveCanvas { path, drag, paint })
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

impl<'skin> MiniWave<'_, '_, '_, '_, '_, 'skin> {
    pub(crate) fn painted<'a>(self) -> Element<'a, UiEvent>
    where
        'skin: 'a,
    {
        Canvas::new(self.paint())
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn paint(self) -> MiniWavePaint<'skin> {
        let waveform = match self.value {
            Some(ReadValue::Waveform(waveform)) => Some(*waveform),
            _ => None,
        };
        let bpm = waveform.and_then(|view| view.bpm);
        let progress = match self
            .reads
            .get(&derived("deck.playback.position_normalized", self.scope))
        {
            Some(ReadValue::Scalar(value)) => value.as_(),
            _ => 0.0,
        };
        let zoom = clamp_zoom(self.zoom);
        let waveform = waveform.map(|view| WaveformData {
            buckets: view.buckets.to_vec().into_boxed_slice(),
            beats: view.beats.to_vec().into_boxed_slice(),
            downbeats: view.downbeats.to_vec().into_boxed_slice(),
            loop_region: view.r#loop,
            cues: view.cues.to_vec().into_boxed_slice(),
        });
        let show_beats = self.style == WaveStyle::Hero;
        let wave_revision = show_beats.then(|| wave_revision(waveform.as_ref(), progress, zoom));
        let overlay = show_beats.then(|| OverlayData {
            title: read_text(self.reads, &derived("deck.track.title", self.scope))
                .filter(|title| !title.is_empty())
                .unwrap_or("No track loaded")
                .to_owned(),
            artist: read_text(self.reads, &derived("deck.track.source_kind", self.scope))
                .unwrap_or("no source")
                .to_owned(),
            bpm: bpm.map_or_else(|| em_dash().to_owned(), |value| format!("{value:.2}")),
            key: read_text(self.reads, &derived("deck.track.key", self.scope))
                .unwrap_or(em_dash())
                .to_owned(),
            remain: read_text(self.reads, &derived("deck.playback.remain", self.scope))
                .unwrap_or(em_dash())
                .to_owned(),
            badge: self.badge.unwrap_or_default().to_owned(),
        });
        MiniWavePaint {
            overlay,
            overlay_palette: overlay_palette(self.skin),
            progress,
            skin: self.skin,
            style: self.style,
            waveform,
            wave_revision,
            zoom,
        }
    }
}

struct MiniWaveCanvas<'skin> {
    path: String,
    drag: Scalar,
    paint: MiniWavePaint<'skin>,
}

struct MiniWavePaint<'skin> {
    overlay: Option<OverlayData>,
    overlay_palette: OverlayPalette,
    progress: f32,
    skin: &'skin Skin,
    style: WaveStyle,
    waveform: Option<WaveformData>,
    wave_revision: Option<u64>,
    zoom: f32,
}

#[derive(Default)]
struct MiniWaveState {
    drag: ScalarState,
    loop_start: Option<f32>,
    modifiers: Modifiers,
    paint: MiniWavePaintState,
}

#[derive(Default)]
struct MiniWavePaintState {
    text: RefCell<Option<TextContext>>,
    wave: canvas::Cache,
    wave_revision: Cell<Option<u64>>,
}

impl canvas::Program<UiEvent> for MiniWaveCanvas<'_> {
    type State = MiniWaveState;

    fn draw(
        &self,
        state: &MiniWaveState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        self.paint
            .geometries(&state.paint, renderer, theme, bounds, cursor)
    }

    fn mouse_interaction(
        &self,
        state: &MiniWaveState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> mouse::Interaction {
        if self.paint.has_waveform() {
            self.drag
                .cursor(&state.drag, &iced_interact::hit(bounds, cursor))
                .into()
        } else {
            mouse::Interaction::default()
        }
    }

    fn update(
        &self,
        state: &mut MiniWaveState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        if let Event::Keyboard(KeyboardEvent::ModifiersChanged(modifiers)) = event {
            state.modifiers = *modifiers;
            return None;
        }
        if !self.paint.has_waveform() {
            return None;
        }
        if self.paint.hero() {
            match event {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                    if state.modifiers.shift() && cursor.is_over(bounds) =>
                {
                    let start = self.track_position(bounds, cursor)?;
                    state.loop_start = Some(start);
                    return Some(scalar_child(&self.path, "loop_start", f64::from(start)));
                }
                Event::Mouse(mouse::Event::CursorMoved { .. }) if state.loop_start.is_some() => {
                    let end = self.track_position(bounds, cursor)?;
                    return Some(scalar_child(&self.path, "loop_end", f64::from(end)));
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                    if state.loop_start.take().is_some() =>
                {
                    return Some(Action::capture());
                }
                _ => {}
            }
        }
        if self.paint.hero()
            && let Some(Input::Wheel(scroll)) = iced_interact::input(event)
            && cursor.is_over(bounds)
        {
            let zoom = zoom_for_wheel(self.paint.zoom, scroll.y());
            return Some(scalar_child(&self.path, "zoom", f64::from(zoom)));
        }
        let input = iced_interact::input(event)?;
        let hit = iced_interact::hit(bounds, cursor);
        scalar(
            &self.path,
            self.drag
                .on_input(&mut state.drag, input, &hit, Instant::now())
                .map(f64::from),
        )
    }
}

impl MiniWaveCanvas<'_> {
    fn track_position(&self, bounds: Rectangle, cursor: Cursor) -> Option<f32> {
        let position = cursor.position()?;
        let window = window_bounds(self.paint.progress, self.paint.zoom);
        x_to_norm(position.x - bounds.x, &window, bounds.width)
    }
}

impl canvas::Program<UiEvent> for MiniWavePaint<'_> {
    type State = MiniWavePaintState;

    fn draw(
        &self,
        state: &MiniWavePaintState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        self.geometries(state, renderer, theme, bounds, cursor)
    }
}

impl MiniWavePaint<'_> {
    fn geometries(
        &self,
        state: &MiniWavePaintState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        let paint = self.program();
        let overlay_bounds = paint.overlay_bounds(Rect {
            h: bounds.height,
            w: bounds.width,
            x: bounds.x,
            y: bounds.y,
        });
        let bounds = local_bounds(&bounds);
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.skin.text_resources().into());

        if self.wave_revision.is_none() {
            let mut list = DrawListBuilder::default();
            paint.paint(&mut list, text, bounds, false);
            return vec![geometry(renderer, bounds, &list.finish(), self.skin)];
        }

        let revision = self.wave_revision.unwrap_or_default();
        if state.wave_revision.get() != Some(revision) {
            state.wave.clear();
            state.wave_revision.set(Some(revision));
        }
        let wave = state
            .wave
            .draw(renderer, Size::new(bounds.w, bounds.h), |frame| {
                let mut list = DrawListBuilder::default();
                paint.paint_wave(&mut list, text, bounds);
                replay(
                    &list.finish(),
                    &mut IcedBackend::new(frame, self.skin.text_resources()),
                );
            });
        let mut layers = vec![wave];
        let show_overlay = !cursor.is_over(iced_bounds(overlay_bounds));
        let mut foreground = DrawListBuilder::default();
        paint.paint_foreground(&mut foreground, text, bounds, show_overlay);
        let foreground = foreground.finish();
        if !foreground.commands().is_empty() {
            layers.push(geometry(renderer, bounds, &foreground, self.skin));
        }
        layers
    }

    const fn hero(&self) -> bool {
        matches!(self.style, WaveStyle::Hero)
    }

    fn has_waveform(&self) -> bool {
        self.waveform
            .as_ref()
            .is_some_and(|waveform| !waveform.buckets.is_empty())
    }

    fn program(&self) -> WavePaint<'_> {
        let waveform = self.waveform.as_ref().map(|waveform| WaveformView {
            buckets: &waveform.buckets,
            beats: &waveform.beats,
            downbeats: &waveform.downbeats,
            bpm: None,
            r#loop: waveform.loop_region,
            cues: &waveform.cues,
        });
        let overlay = self.overlay.as_ref().map(|overlay| Overlay {
            title: &overlay.title,
            artist: &overlay.artist,
            bpm: &overlay.bpm,
            key: &overlay.key,
            remain: &overlay.remain,
            badge: &overlay.badge,
            palette: self.overlay_palette,
        });
        WavePaint {
            background: self.skin.rgba(self.skin.wave.background),
            border: self.skin.rgba(self.skin.wave.frame.border),
            cue_badge: self.skin.rgba(self.skin.wave.cue_badge_background),
            cue_text: self.skin.rgba(self.skin.wave.cue_badge_text_color),
            metrics: self.skin.wave,
            overlay,
            palette: WavePalette {
                bg_deep: self.skin.palette.bg_deep.into(),
                line: self.skin.palette.line.into(),
                text_dim: self.skin.palette.text_dim.into(),
                accent: self.skin.palette.accent.into(),
                accent_strong: self.skin.palette.accent_strong.into(),
                wave_low: self.skin.palette.wave_low.into(),
                wave_mid: self.skin.palette.wave_mid.into(),
                wave_high: self.skin.palette.wave_high.into(),
            },
            progress: self.progress,
            style: self.style,
            waveform,
            zoom: self.zoom,
        }
    }
}

fn geometry(renderer: &Renderer, bounds: Rect, list: &DrawList, skin: &Skin) -> Geometry {
    let mut frame = Frame::new(renderer, Size::new(bounds.w, bounds.h));
    replay(
        list,
        &mut IcedBackend::new(&mut frame, skin.text_resources()),
    );
    frame.into_geometry()
}

const fn local_bounds(bounds: &Rectangle) -> Rect {
    Rect {
        h: bounds.height,
        w: bounds.width,
        x: 0.0,
        y: 0.0,
    }
}

const fn iced_bounds(bounds: Rect) -> Rectangle {
    Rectangle {
        height: bounds.h,
        width: bounds.w,
        x: bounds.x,
        y: bounds.y,
    }
}

fn overlay_palette(skin: &Skin) -> OverlayPalette {
    let metrics = skin.wave.overlay;
    OverlayPalette {
        background: with_alpha(skin.rgba(metrics.background), metrics.background_alpha),
        art_background: skin.rgba(metrics.art_background),
        art_border: skin.rgba(metrics.art_frame.border),
        art_label: skin.rgba(metrics.art_label_color),
        title: skin.rgba(metrics.title_color),
        artist: skin.rgba(metrics.artist_color),
        readout_background: skin.rgba(metrics.readout_background),
        readout_border: skin.rgba(metrics.readout_frame.border),
        readout_label: skin.rgba(metrics.readout_label_color),
        bpm: skin.rgba(metrics.bpm_color),
        key: skin.rgba(metrics.key_color),
        remain: skin.rgba(metrics.remain_color),
        badge_background: skin.rgba(metrics.badge_background),
        badge_border: skin.rgba(metrics.badge_frame.border),
        badge_text: skin.rgba(metrics.badge_text_color),
    }
}

const fn with_alpha(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

fn wave_revision(waveform: Option<&WaveformData>, progress: f32, zoom: f32) -> u64 {
    let mut hasher = DefaultHasher::new();
    progress.to_bits().hash(&mut hasher);
    zoom.to_bits().hash(&mut hasher);
    if let Some(waveform) = waveform {
        waveform.buckets.len().hash(&mut hasher);
        for bucket in &waveform.buckets {
            bucket.low.to_bits().hash(&mut hasher);
            bucket.mid.to_bits().hash(&mut hasher);
            bucket.high.to_bits().hash(&mut hasher);
        }
        for mark in waveform.beats.iter().chain(waveform.downbeats.iter()) {
            mark.to_bits().hash(&mut hasher);
        }
        for cue in &waveform.cues {
            cue.to_bits().hash(&mut hasher);
        }
        if let Some([start, end]) = waveform.loop_region {
            start.to_bits().hash(&mut hasher);
            end.to_bits().hash(&mut hasher);
        }
    }
    hasher.finish()
}

const fn em_dash() -> &'static str {
    "\u{2014}"
}

fn read_text<'a>(reads: &'a dyn Reads, endpoint: &str) -> Option<&'a str> {
    match reads.get(endpoint) {
        Some(ReadValue::Text(value)) => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use iced::advanced::widget::{Tree, tree::State};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        draw::{DrawCmd, Geom},
        render::WaveBucket,
    };

    struct EmptyReads;

    impl Reads for EmptyReads {
        fn get(&self, _endpoint: &str) -> Option<ReadValue<'_>> {
            None
        }
    }

    #[kithara::test]
    fn painted_wave_owns_no_interactive_canvas_state() {
        let reads = EmptyReads;
        let wave = || {
            MiniWave::builder()
                .path("overview/a/wave")
                .style(WaveStyle::Default)
                .scope("")
                .reads(&reads)
                .skin(crate::builtin::skin())
                .zoom(crate::widgets::wave::zoom_math::DEFAULT_ZOOM)
                .build()
        };

        let interactive = wave().view();
        let painted = wave().painted();
        let interactive = Tree::new(interactive.as_widget());
        let painted = Tree::new(painted.as_widget());
        assert!(matches!(
            &interactive.state,
            State::Some(state) if state.is::<MiniWaveState>()
        ));
        assert!(matches!(
            &painted.state,
            State::Some(state) if state.is::<MiniWavePaintState>()
        ));
    }

    #[kithara::test]
    fn overlay_hides_only_when_the_cursor_is_inside_the_header_strip() {
        let metrics = crate::builtin::skin_doc().wave.overlay;
        let bounds = Rect {
            h: 300.0,
            w: 400.0,
            x: 100.0,
            y: 50.0,
        };
        let strip = crate::widgets::wave::overlay::strip(bounds, metrics);

        assert!(strip.contains(crate::draw::Pt {
            x: 150.0,
            y: 50.0 + metrics.height / 2.0,
        }));
        assert!(!strip.contains(crate::draw::Pt {
            x: 150.0,
            y: 50.0 + metrics.height + 40.0,
        }));
    }

    #[kithara::test]
    fn overlay_strip_clamps_to_short_bounds() {
        let metrics = crate::builtin::skin_doc().wave.overlay;
        let bounds = Rect {
            h: metrics.height / 2.0,
            w: 200.0,
            x: 0.0,
            y: 0.0,
        };
        assert_eq!(
            crate::widgets::wave::overlay::strip(bounds, metrics).h,
            bounds.h
        );
    }

    #[kithara::test]
    fn hero_wave_paints_every_layer_through_the_draw_seam() {
        struct WaveReads {
            buckets: [WaveBucket; 2],
        }

        impl Reads for WaveReads {
            fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
                let endpoint = endpoint.split_once('@').map_or(endpoint, |(id, _)| id);
                match endpoint {
                    "deck.playback.waveform" => Some(ReadValue::Waveform(WaveformView {
                        buckets: &self.buckets,
                        beats: &[0.25],
                        downbeats: &[0.5],
                        bpm: Some(128.0),
                        r#loop: Some([0.2, 0.6]),
                        cues: &[0.4],
                    })),
                    "deck.playback.position_normalized" => Some(ReadValue::Scalar(0.3)),
                    "deck.track.title" => Some(ReadValue::Text("Track")),
                    "deck.track.source_kind" => Some(ReadValue::Text("Source")),
                    "deck.track.key" => Some(ReadValue::Text("8A")),
                    "deck.playback.remain" => Some(ReadValue::Text("-01:00")),
                    _ => None,
                }
            }
        }

        let reads = WaveReads {
            buckets: [
                WaveBucket {
                    low: 0.25,
                    mid: 0.5,
                    high: 0.75,
                },
                WaveBucket {
                    low: 0.75,
                    mid: 0.5,
                    high: 0.25,
                },
            ],
        };
        let snapshot = MiniWave::builder()
            .path("deck-a/wave")
            .style(WaveStyle::Hero)
            .badge("A")
            .value(&reads.get("deck.playback.waveform").unwrap())
            .scope("@deck=a")
            .reads(&reads)
            .skin(crate::builtin::skin())
            .zoom(1.0)
            .build()
            .paint();
        let mut text = TextContext::from(snapshot.skin.text_resources());
        let mut list = DrawListBuilder::default();
        snapshot.program().paint(
            &mut list,
            &mut text,
            Rect {
                h: 120.0,
                w: 640.0,
                x: 0.0,
                y: 0.0,
            },
            true,
        );
        let list = list.finish();

        assert!(list.commands().iter().any(|command| matches!(
            command,
            DrawCmd::Fill {
                geom: Geom::Rect(_),
                ..
            }
        )));
        assert!(list.commands().iter().any(|command| matches!(
            command,
            DrawCmd::Stroke {
                geom: Geom::Line { .. },
                ..
            }
        )));
        assert!(list.commands().iter().any(|command| matches!(
            command,
            DrawCmd::Text { content, .. } if content == "1"
        )));
        assert!(list.commands().iter().any(|command| matches!(
            command,
            DrawCmd::Clip { list, .. } if list.commands().iter().any(|nested| matches!(
                nested,
                DrawCmd::Text { content, .. } if content == "Track"
            ))
        )));
    }
}

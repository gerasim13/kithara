use std::{cell::RefCell, collections::BTreeSet};

use iced::{
    Size,
    advanced::{
        layout::{Layout, Limits},
        widget::Tree,
    },
};
use kithara_test_utils::kithara;
use kithara_ui::{
    app::{App, Config, Ui},
    builtin,
    compile::{CompiledUi, compile},
    draw::{Pt, Rect},
    error::UiDocError,
    interact::{Input, MOUSE, PointerInput, PointerPhase},
    mock::TestRegistry,
    registry::{EndpointCategory, EndpointDesc, ValueKind},
    render::{Clock, ReadValue, Reads, Skin, UiEvent, WriteValue, tree},
    source::{MemResolver, UiConfig},
    view,
};

use crate::shared::{collect_rows, renderer};

const WINDOW: (u32, u32) = (300, 240);
const PAGES: [&str; 2] = ["scene/alpha-page/caption", "scene/beta-page/caption"];

fn documents(read: &str) -> MemResolver {
    let mut resolver = MemResolver::default();
    resolver.insert(
        "switch.klayout.ron",
        r#"(schema: "kithara.layout", version: 1, id: "switch",
            root: Module(instance: "scene", source: "scene.kmodule.ron",
                size: (w: Fill, h: Fill)))"#,
    );
    resolver.insert(
        "scene.kmodule.ron",
        &format!(
            r#"(schema: "kithara.module", version: 1, id: "scene", chrome: Plain,
                root: Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
                    Pressable(id: "next", press: Command(id: "fixture.next"),
                        child: Spacer(id: "head", size: Some((w: Fill, h: Fixed(26.0))))),
                    Switch(id: "pages", read: {read}, scope: "source", cases: {{
                        "alpha": Include(id: "alpha-page", source: "alpha.kmodule.ron",
                            with: {{ "source": "$source" }}),
                        "beta": Include(id: "beta-page", source: "beta.kmodule.ron",
                            with: {{ "source": "$source" }}),
                    }}),
                ]))"#,
        ),
    );
    for (source, height) in [("alpha", 30), ("beta", 50)] {
        resolver.insert(
            &format!("{source}.kmodule.ron"),
            &format!(
                r#"(schema: "kithara.module", version: 1, id: "{source}",
                    parameters: ["source"], chrome: Plain,
                    root: Text(id: "caption", size: Some((w: Fill, h: Fixed({height}.0))),
                        read: Model(id: "fixture.caption", with: {{ "source": "$source" }})))"#,
            ),
        );
    }
    resolver
}

fn endpoints(kind: ValueKind) -> TestRegistry {
    let mut registry = TestRegistry::default();
    registry.insert(
        EndpointCategory::Model,
        "fixture.selected",
        EndpointDesc::new(kind),
    );
    registry.insert(
        EndpointCategory::Model,
        "fixture.caption",
        EndpointDesc::new(ValueKind::Text).with_scope("source"),
    );
    registry.insert(
        EndpointCategory::Command,
        "fixture.next",
        EndpointDesc::new(ValueKind::Trigger),
    );
    registry
}

fn compiled(read: &str, kind: ValueKind) -> Result<CompiledUi, UiDocError> {
    compile(
        "switch.klayout.ron",
        &documents(read),
        &endpoints(kind),
        builtin::skin_doc(),
        builtin::text_doc(),
        &UiConfig::default(),
        &view::EMPTY,
    )
}

struct Selection {
    selected: Option<&'static str>,
    seen: RefCell<BTreeSet<String>>,
}

impl Selection {
    fn new(selected: Option<&'static str>) -> Self {
        Self {
            selected,
            seen: RefCell::default(),
        }
    }
}

impl Reads for Selection {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        self.seen.borrow_mut().insert(endpoint.to_owned());
        match endpoint {
            "fixture.selected" => self.selected.map(ReadValue::Text),
            "fixture.caption@source=alpha" => Some(ReadValue::Text("Alpha")),
            "fixture.caption@source=beta" => Some(ReadValue::Text("Beta")),
            _ => None,
        }
    }
}

impl App for Selection {
    fn document(&self) -> &str {
        "switch.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn skin(&self) -> &Skin {
        builtin::skin()
    }

    fn update(&mut self, event: UiEvent) {
        if let UiEvent::Write {
            key,
            value: WriteValue::Trigger,
        } = event
            && key == "fixture.next"
        {
            self.selected = match self.selected {
                Some("alpha") => Some("beta"),
                Some("beta") => Some("unknown"),
                Some(_) => None,
                None => Some("alpha"),
            };
        }
    }
}

fn immediate(ui: &CompiledUi, selection: &Selection) -> Vec<Rect> {
    let mut element = tree::render(
        &ui.root,
        ui,
        selection,
        &view::EMPTY,
        builtin::skin(),
        Clock::default(),
        None,
    );
    let mut state = Tree::new(element.as_widget());
    let node = element.as_widget_mut().layout(
        &mut state,
        &renderer(),
        &Limits::new(Size::ZERO, Size::new(300.0, 240.0)),
    );
    let mut rows = Vec::new();
    collect_rows(Layout::new(&node), &mut rows);
    rows.retain(|rect| rect.w > 0.0 && rect.h > 0.0);
    rows
}

#[kithara::test]
fn a_switch_reads_only_the_selected_page_in_its_case_scope() {
    let ui = compiled(r#"Model(id: "fixture.selected")"#, ValueKind::Text)
        .expect("the Switch document must compile");
    for selected in ["alpha", "beta"] {
        let selection = Selection::new(Some(selected));
        let rows = immediate(&ui, &selection);
        assert_eq!(rows.len(), 2, "one header and one selected page stand");
        assert_eq!(
            selection.seen.into_inner(),
            BTreeSet::from([
                "fixture.selected".to_owned(),
                format!("fixture.caption@source={selected}"),
            ]),
            "the chosen case supplies its value to the included page's scope",
        );
    }
}

#[kithara::test]
fn a_switch_without_a_matching_model_value_has_no_page() {
    let ui = compiled(r#"Model(id: "fixture.selected")"#, ValueKind::Text)
        .expect("the Switch document must compile");
    for selected in [None, Some("unknown")] {
        let selection = Selection::new(selected);
        let rows = immediate(&ui, &selection);
        assert_eq!(
            rows.iter().map(|rect| rect.h).collect::<Vec<_>>(),
            [26.0],
            "an unanswered or unmatched selector leaves only the header",
        );
        assert_eq!(
            selection.seen.into_inner(),
            BTreeSet::from(["fixture.selected".to_owned()]),
            "a selector with no matching case reads no page endpoints",
        );
    }
}

fn shown(ui: &mut Ui<'_, Selection>) -> Vec<(&'static str, Rect)> {
    ui.scene().expect("the selected page must draw");
    ["scene/head", PAGES[0], PAGES[1]]
        .into_iter()
        .filter_map(|path| {
            ui.rect_of(path)
                .filter(|rect| rect.w > 0.0 && rect.h > 0.0)
                .map(|rect| (path, rect))
        })
        .collect()
}

fn next(ui: &mut Ui<'_, Selection>) {
    let head = ui.rect_of("scene/head").expect("the next button stands");
    let at = Pt {
        x: head.x + head.w / 2.0,
        y: head.y + head.h / 2.0,
    };
    for phase in [PointerPhase::Move, PointerPhase::Down, PointerPhase::Up] {
        ui.input(Input::Pointer(PointerInput::new(
            MOUSE,
            None,
            phase,
            Some(at),
            1,
        )));
    }
}

#[kithara::test]
fn both_hosts_change_the_switch_page_with_the_model_value() {
    let endpoints = endpoints(ValueKind::Text);
    let resolver = documents(r#"Model(id: "fixture.selected")"#);
    let compiled = compiled(r#"Model(id: "fixture.selected")"#, ValueKind::Text)
        .expect("the Switch document must compile");
    let mut ui = Ui::new(
        Selection::new(Some("alpha")),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        WINDOW,
        1.0,
    )
    .expect("the retained host must mount the Switch document");

    for (selected, page) in [
        (Some("alpha"), Some(PAGES[0])),
        (Some("beta"), Some(PAGES[1])),
        (Some("unknown"), None),
        (None, None),
        (Some("alpha"), Some(PAGES[0])),
    ] {
        assert_eq!(ui.app().selected, selected, "the press advances the model");
        let retained = shown(&mut ui);
        let mut paths = vec!["scene/head"];
        paths.extend(page);
        assert_eq!(
            retained.iter().map(|(path, _)| *path).collect::<Vec<_>>(),
            paths,
            "the retained tree shows exactly the matching case",
        );
        assert_eq!(
            retained.into_iter().map(|(_, rect)| rect).collect::<Vec<_>>(),
            immediate(&compiled, &Selection::new(selected)),
            "both hosts lay out the same selected page after the model changes",
        );
        next(&mut ui);
    }
}

#[kithara::test]
fn a_switch_rejects_a_selector_that_is_not_text() {
    let error = compiled(r#"Model(id: "fixture.selected")"#, ValueKind::Bool)
        .expect_err("a Switch needs a Text selector");
    assert!(
        matches!(error, UiDocError::BindingType { id, expected, got, .. }
            if id == "fixture.selected" && expected == "Text" && got == "Bool"),
    );
}

#[kithara::test]
fn a_switch_rejects_an_undeclared_selector() {
    let error = compiled(r#"Model(id: "fixture.absent")"#, ValueKind::Text)
        .expect_err("the selector must be declared by the registry");
    assert!(
        matches!(error, UiDocError::UnknownEndpoint { id, .. }
            if id == "fixture.absent"),
    );
}

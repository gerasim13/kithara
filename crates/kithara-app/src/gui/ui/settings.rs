use std::{cell::RefCell, rc::Rc};

use ::kithara::ui::{
    app::Ui,
    draw::Pt,
    interact::{Input, Key, Modifiers},
};
use kithara_app_library::Registration;
use kithara_test_utils::kithara;

use crate::gui::{
    retained::Studio,
    test_fixture::{
        Calls, Probe,
        retained::{click, laid, press, studio},
    },
};

const BARS: [&str; 2] = ["bar", "micro-bar"];
const TALL: (u32, u32) = (1280, 760);

type Told = Rc<RefCell<Calls>>;

fn sections() -> (Vec<Registration>, Told, Told) {
    let (alpha, alpha_calls) = Probe::section("alpha", "ALPHA");
    let (beta, beta_calls) = Probe::section("beta", "BETA");
    (vec![alpha, beta], alpha_calls, beta_calls)
}

fn escape(ui: &mut Ui<'_, Studio>) {
    ui.input(Input::KeyPressed {
        key: Key::Escape,
        modifiers: Modifiers::default(),
        text: None,
    });
    ui.input(Input::KeyReleased {
        key: Key::Escape,
        modifiers: Modifiers::default(),
    });
}

fn open(ui: &mut Ui<'_, Studio>, bar: &str) {
    press(ui, &format!("{bar}/settings-open"));
}

fn standing(ui: &mut Ui<'_, Studio>) -> Vec<&'static str> {
    BARS.into_iter()
        .filter(|bar| laid(ui, &format!("{bar}/settings/close-icon")).is_some())
        .collect()
}

/// Each bar opens one window at the first section, its nav turns the
/// section, a section's write reaches only its plugin, and each closer
/// but a press on the scrim shuts the window.
#[kithara::test(native, flash(false))]
fn each_bar_opens_one_window_that_turns_routes_writes_and_closes() {
    const SHORT: (u32, u32) = (1280, 480);
    for (size, bar) in [(TALL, "bar"), (SHORT, "micro-bar")] {
        let (plugins, alpha, beta) = sections();
        studio(plugins, size, |ui| {
            let row = |id: &str| format!("{bar}/settings/detail/{id}/row-face");
            assert!(standing(ui).is_empty(), "{bar}: the window starts shut");
            open(ui, bar);
            assert_eq!(standing(ui), [bar], "{bar}: one window opens");
            assert!(laid(ui, &row("alpha")).is_some());
            assert!(laid(ui, &row("beta")).is_none());

            press(ui, &format!("{bar}/settings/nav/beta/item"));
            assert!(laid(ui, &row("alpha")).is_none());
            press(ui, &row("beta"));
            assert_eq!(beta.borrow().written, ["toggle"]);
            assert!(alpha.borrow().written.is_empty(), "alpha hears nothing");

            for closer in ["close", "done", "escape", "scrim"] {
                if standing(ui).is_empty() {
                    open(ui, bar);
                }
                match closer {
                    "escape" => escape(ui),
                    "scrim" => click(ui, Pt { x: 4.0, y: 4.0 }),
                    "close" => press(ui, &format!("{bar}/settings/close-icon")),
                    _ => press(ui, &format!("{bar}/settings/{closer}-label")),
                }
                let left: &[&str] = if closer == "scrim" { &[bar] } else { &[] };
                assert_eq!(standing(ui), left, "{bar}: after `{closer}`");
            }
        });
    }
}

#[kithara::test(native, flash(false))]
fn without_sections_the_detail_shows_its_default() {
    studio(Vec::new(), TALL, |ui| {
        open(ui, "bar");
        ui.scene()
            .unwrap_or_else(|error| panic!("the studio must draw: {error}"));
        let unfilled = ui.rect_of("bar/settings/unfilled");
        assert!(
            unfilled.is_some_and(|rect| rect.w > 0.0),
            "the default spans the detail: {unfilled:?}"
        );
    });
}

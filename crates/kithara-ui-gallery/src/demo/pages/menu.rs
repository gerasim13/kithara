use kithara_ui::render::{ReadValue, Scope, WriteValue};

mod consts {
    use super::MenuTrack;

    pub(super) const CLUB_MODULES: [bool; 11] = [
        true, true, true, false, true, true, false, false, true, true, true,
    ];
    pub(super) const DISPLAYS: [&str; 3] = ["MACBOOK PRO 16\"", "DELL U2720Q", "IPAD · SIDECAR"];
    pub(super) const LAYOUTS: [&str; 4] = [
        "CLUB · 2 DECKS",
        "STUDIO · 4 DECKS + VST",
        "VISUALS + TIMELINE",
        "NARROW WINDOW · TABS",
    ];
    pub(super) const MAX_WINDOWS: usize = 3;
    pub(super) const MODULES: [&str; 11] = [
        "ov", "mix", "fx1", "fx2", "vcf", "rec", "vis", "tl", "cpu", "net", "buf",
    ];
    pub(super) const NEW_WINDOW_LAYOUT: usize = 3;
    pub(super) const NEW_WINDOW_MODULES: [bool; 11] = [
        true, true, false, false, false, false, false, false, false, false, false,
    ];
    pub(super) const TRACKS: [MenuTrack; 4] = [
        MenuTrack {
            title: "AURORA DRIFT",
            meta: "124 · 8A",
            energy: 0.72,
        },
        MenuTrack {
            title: "NIGHT SHIFT",
            meta: "126 · 5A",
            energy: 0.54,
        },
        MenuTrack {
            title: "PARALLAX",
            meta: "128 · 11B",
            energy: 0.83,
        },
        MenuTrack {
            title: "SLOW BURN",
            meta: "120 · 2A",
            energy: 0.36,
        },
    ];
    pub(super) const VISUAL_MODULES: [bool; 11] = [
        false, false, false, false, false, false, true, true, false, false, false,
    ];
}

struct MenuTrack {
    meta: &'static str,
    title: &'static str,
    energy: f64,
}

struct MenuWindow {
    caption: String,
    title: String,
    modules: [bool; 11],
    display: usize,
    layout: usize,
}

impl MenuWindow {
    const fn new(display: usize, layout: usize, modules: [bool; 11]) -> Self {
        Self {
            display,
            layout,
            modules,
            title: String::new(),
            caption: String::new(),
        }
    }

    fn modules_on(&self) -> usize {
        self.modules.iter().filter(|on| **on).count()
    }
}

pub(crate) struct MenuState {
    count_label: String,
    modules_count: String,
    modules_title: String,
    windows: Vec<MenuWindow>,
    autogain: bool,
    casting: bool,
    mono: bool,
    recording: bool,
    wave_follow: bool,
    active: usize,
}

impl Default for MenuState {
    fn default() -> Self {
        let mut state = Self {
            windows: vec![
                MenuWindow::new(0, 0, consts::CLUB_MODULES),
                MenuWindow::new(1, 2, consts::VISUAL_MODULES),
            ],
            active: 0,
            wave_follow: true,
            autogain: true,
            mono: false,
            recording: false,
            casting: true,
            count_label: String::new(),
            modules_title: String::new(),
            modules_count: String::new(),
        };
        state.rebuild();
        state
    }
}

impl MenuState {
    /// Answers one write the application menu declares. A window, module or
    /// layout is named by the scope the menu row filled in.
    pub(crate) fn write(&mut self, id: &str, scope: Scope<'_>, value: &WriteValue) {
        if *value != WriteValue::Trigger {
            return;
        }
        let window = scope.get("window").and_then(row_index);
        let module = scope.get("module").and_then(module_index_of);
        let layout = scope.get("layout").and_then(row_index);
        match (id, window, module, layout) {
            ("ui.window.open", ..) => self.open_window(),
            ("ui.window.focus", Some(index), ..) => self.focus(index),
            ("ui.window.cycle_display", Some(index), ..) => self.cycle_display(index),
            ("ui.window.close", Some(index), ..) => self.close(index),
            ("ui.module.toggle", _, Some(index), _) => self.toggle_module(index),
            ("ui.layout.apply", .., Some(index)) => self.apply_layout(index),
            ("ui.prefs.toggle_wave_follow", ..) => self.wave_follow = !self.wave_follow,
            ("ui.prefs.toggle_autogain", ..) => self.autogain = !self.autogain,
            ("ui.prefs.toggle_mono", ..) => self.mono = !self.mono,
            ("ui.set.toggle_record", ..) => self.recording = !self.recording,
            ("ui.set.toggle_cast", ..) => self.casting = !self.casting,
            _ => {}
        }
    }

    fn apply_layout(&mut self, index: usize) {
        if index >= consts::LAYOUTS.len() {
            return;
        }
        let active = self.active;
        self.windows[active].layout = index;
        self.rebuild();
    }

    const fn can_open(&self) -> bool {
        self.windows.len() < consts::MAX_WINDOWS
    }

    const fn closable(&self, index: usize) -> bool {
        self.windows.len() > 1 && index != 0
    }

    fn close(&mut self, index: usize) {
        if !self.closable(index) {
            return;
        }
        self.windows.remove(index);
        if self.active >= index {
            self.active -= 1;
        }
        self.rebuild();
    }

    fn cycle_display(&mut self, index: usize) {
        let Some(window) = self.windows.get_mut(index) else {
            return;
        };
        window.display = (window.display + 1) % consts::DISPLAYS.len();
        self.rebuild();
    }

    fn focus(&mut self, index: usize) {
        if index < self.windows.len() {
            self.active = index;
            self.rebuild();
        }
    }

    pub(crate) fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let (id, scope) = Scope::split(endpoint);
        let value = match id {
            "ui.window.count" => ReadValue::Text(&self.count_label),
            "ui.window.can_open" => ReadValue::Bool(self.can_open()),
            "ui.window.active" => {
                ReadValue::Bool(scope.get("window").and_then(row_index)? == self.active)
            }
            "ui.window.hidden" => {
                ReadValue::Bool(scope.get("window").and_then(row_index)? >= self.windows.len())
            }
            "ui.window.close_hidden" => {
                ReadValue::Bool(!self.closable(scope.get("window").and_then(row_index)?))
            }
            "ui.window.title" => ReadValue::Text(
                &self
                    .windows
                    .get(scope.get("window").and_then(row_index)?)?
                    .title,
            ),
            "ui.window.caption" => ReadValue::Text(
                &self
                    .windows
                    .get(scope.get("window").and_then(row_index)?)?
                    .caption,
            ),
            "ui.module.on" => ReadValue::Bool(
                self.windows[self.active].modules[scope.get("module").and_then(module_index_of)?],
            ),
            "ui.modules.title" => ReadValue::Text(&self.modules_title),
            "ui.modules.count" => ReadValue::Text(&self.modules_count),
            "ui.layout.selected" => ReadValue::Bool(
                scope.get("layout").and_then(row_index)? == self.windows[self.active].layout,
            ),
            "ui.layouts.active" => {
                ReadValue::Text(consts::LAYOUTS[self.windows[self.active].layout])
            }
            "ui.prefs.wave_follow" => ReadValue::Bool(self.wave_follow),
            "ui.prefs.autogain" => ReadValue::Bool(self.autogain),
            "ui.prefs.mono" => ReadValue::Bool(self.mono),
            "ui.set.recording" => ReadValue::Bool(self.recording),
            "ui.set.casting" => ReadValue::Bool(self.casting),
            "ui.set.record_hint" => {
                ReadValue::Text(if self.recording { "RECORDING" } else { "⌘R" })
            }
            "ui.set.cast_hint" => ReadValue::Text(if self.casting { "AUDIO LIVE" } else { "OFF" }),
            _ => return None,
        };
        Some(value)
    }

    fn open_window(&mut self) {
        if !self.can_open() {
            return;
        }
        let display = self.windows.len();
        self.windows.push(MenuWindow::new(
            display,
            consts::NEW_WINDOW_LAYOUT,
            consts::NEW_WINDOW_MODULES,
        ));
        self.rebuild();
    }

    fn rebuild(&mut self) {
        for (index, window) in self.windows.iter_mut().enumerate() {
            let number = index + 1;
            window.title = format!("WINDOW {number} · {}", consts::LAYOUTS[window.layout]);
            window.caption = format!(
                "{} · {} MOD.",
                consts::DISPLAYS[window.display],
                window.modules_on()
            );
        }
        let count = self.windows.len();
        self.count_label = if count == 1 {
            "1 WINDOW".to_owned()
        } else {
            format!("{count} WINDOWS")
        };
        let active = self.active + 1;
        self.modules_title = format!("Modules · WINDOW {active}");
        let on = self.windows[self.active].modules_on();
        self.modules_count = format!("{on} OF 11");
    }

    fn toggle_module(&mut self, index: usize) {
        let active = self.active;
        self.windows[active].modules[index] = !self.windows[active].modules[index];
        self.rebuild();
    }
}

pub(crate) struct ContextState {
    action: String,
    selected: usize,
}

impl Default for ContextState {
    fn default() -> Self {
        Self {
            selected: 0,
            action: "—".to_owned(),
        }
    }
}

impl ContextState {
    pub(crate) fn write(&mut self, id: &str, scope: Scope<'_>, value: &WriteValue) {
        let Some(row) = scope
            .get("row")
            .and_then(row_index)
            .filter(|row| *row < consts::TRACKS.len())
        else {
            return;
        };
        match (id, value, scope.get("deck")) {
            ("gallery.menu.select", WriteValue::Trigger, _) => self.selected = row,
            ("gallery.menu.load", WriteValue::Trigger, Some("a")) => self.run(row, "DECK A"),
            ("gallery.menu.load", WriteValue::Trigger, Some("b")) => self.run(row, "DECK B"),
            ("gallery.menu.queue", WriteValue::Trigger, _) => self.run(row, "TO QUEUE"),
            _ => {}
        }
    }

    pub(crate) fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        if endpoint == "gallery.menu.action" {
            return Some(ReadValue::Text(&self.action));
        }
        let (id, scope) = Scope::split(endpoint);
        let row = scope.get("row").and_then(row_index)?;
        let track = consts::TRACKS.get(row)?;
        let value = match id {
            "gallery.menu.selected" => ReadValue::Bool(self.selected == row),
            "gallery.menu.track" => ReadValue::Text(track.title),
            "gallery.menu.bpm" => ReadValue::Text(track.meta),
            "gallery.menu.energy" => ReadValue::Scalar(track.energy),
            _ => return None,
        };
        Some(value)
    }

    fn run(&mut self, row: usize, label: &str) {
        let track = row + 1;
        self.action = format!("{label} · {track}");
    }
}

fn row_index(number: &str) -> Option<usize> {
    number.parse::<usize>().ok()?.checked_sub(1)
}

fn module_index_of(key: &str) -> Option<usize> {
    consts::MODULES.iter().position(|name| *name == key)
}

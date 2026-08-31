/// The one group the app menu expands. Whether the menu itself stands open is
/// state the document keeps, which no application is asked for.
#[derive(Default, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(in crate::gui) struct MenuState {
    #[field(get = are_layouts_open, vis = "pub(in crate::gui)", copy)]
    layouts_open: bool,
    #[field(get = are_modules_open, vis = "pub(in crate::gui)", copy)]
    modules_open: bool,
}

impl MenuState {
    pub(in crate::gui) const fn toggle_layouts(&mut self) {
        self.layouts_open = !self.layouts_open;
    }

    pub(in crate::gui) const fn toggle_modules(&mut self) {
        self.modules_open = !self.modules_open;
    }
}

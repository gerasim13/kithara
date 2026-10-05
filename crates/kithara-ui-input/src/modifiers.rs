/// Active keyboard modifiers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers {
    alt: bool,
    control: bool,
    logo: bool,
    shift: bool,
}

impl Modifiers {
    #[must_use]
    pub const fn new(alt: bool, control: bool, logo: bool, shift: bool) -> Self {
        Self {
            alt,
            control,
            logo,
            shift,
        }
    }

    #[must_use]
    pub const fn alt(self) -> bool {
        self.alt
    }

    /// The platform's command modifier: Command on macOS, Control elsewhere.
    #[must_use]
    pub const fn command(self) -> bool {
        if cfg!(target_os = "macos") {
            self.logo
        } else {
            self.control
        }
    }

    #[must_use]
    pub const fn control(self) -> bool {
        self.control
    }

    #[must_use]
    pub const fn logo(self) -> bool {
        self.logo
    }

    #[must_use]
    pub const fn shift(self) -> bool {
        self.shift
    }
}

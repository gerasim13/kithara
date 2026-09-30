use kithara_ui::{
    draw::Pt,
    render::{ReadValue, WriteValue},
};

/// Where each carried placement of the scene page stands, and which artwork the
/// one that answers a press is showing.
///
/// The point is the application's: the document publishes where a drag left a
/// placement and reads back where it now stands, so both hosts move it by
/// asking the same model rather than each keeping a point of its own.
pub(crate) struct SceneState {
    one: Pt,
    two: Pt,
    sparked: bool,
}

impl Default for SceneState {
    fn default() -> Self {
        Self {
            one: Pt { x: 16.0, y: 32.0 },
            two: Pt { x: 432.0, y: 32.0 },
            sparked: false,
        }
    }
}

impl SceneState {
    /// Answers the press on the artwork, which turns the flag it switches
    /// on, and the point a drag published for one of the placements.
    pub(crate) fn write(&mut self, id: &str, value: &WriteValue) {
        match (id, value) {
            ("gallery.scene.switch", WriteValue::Trigger) => self.sparked = !self.sparked,
            ("gallery.scene.one", WriteValue::Point(at)) => self.one = *at,
            ("gallery.scene.two", WriteValue::Point(at)) => self.two = *at,
            _ => {}
        }
    }

    pub(crate) fn get(&self, endpoint: &str) -> Option<ReadValue<'static>> {
        let value = match endpoint {
            "gallery.scene.one" => ReadValue::Point(self.one),
            "gallery.scene.two" => ReadValue::Point(self.two),
            "gallery.scene.sparked" => ReadValue::Bool(self.sparked),
            _ => return None,
        };
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use kithara_ui::{
        draw::Pt,
        render::{ReadValue, WriteValue},
    };

    use super::SceneState;

    mod consts {
        use super::Pt;

        pub(super) const AT: Pt = Pt { x: 120.0, y: 64.0 };
    }

    #[kithara::test]
    fn a_placement_stands_where_its_drag_published() {
        let mut scene = SceneState::default();

        scene.write("gallery.scene.one", &WriteValue::Point(consts::AT));
        assert_eq!(
            scene.get("gallery.scene.one"),
            Some(ReadValue::Point(consts::AT))
        );
    }

    /// The two placements are two points, so moving one leaves the other where
    /// it was.
    #[kithara::test]
    fn moving_one_placement_leaves_the_other_standing() {
        let mut scene = SceneState::default();
        let before = scene.get("gallery.scene.two");

        scene.write("gallery.scene.one", &WriteValue::Point(consts::AT));

        assert_eq!(scene.get("gallery.scene.two"), before);
    }

    #[kithara::test]
    fn the_press_turns_the_flag_the_artwork_switches_on() {
        let mut scene = SceneState::default();

        assert_eq!(
            scene.get("gallery.scene.sparked"),
            Some(ReadValue::Bool(false))
        );
        scene.write("gallery.scene.switch", &WriteValue::Trigger);
        assert_eq!(
            scene.get("gallery.scene.sparked"),
            Some(ReadValue::Bool(true))
        );
    }
}

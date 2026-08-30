use kithara_ui::{draw::Pt, render::ReadValue};

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
    /// Answers the press on the artwork by turning the flag it switches on.
    pub(crate) fn activate(&mut self, path: &str) -> bool {
        if path != "scene/switch" {
            return false;
        }
        self.sparked = !self.sparked;
        true
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

    /// Takes the point a drag published, if the path is one of the scene's own
    /// placements.
    pub(crate) fn place(&mut self, path: &str, at: Pt) -> bool {
        match path {
            "scene/carry-one" => self.one = at,
            "scene/carry-two" => self.two = at,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use kithara_ui::{draw::Pt, render::ReadValue};

    use super::SceneState;

    const AT: Pt = Pt { x: 120.0, y: 64.0 };

    #[kithara::test]
    fn a_placement_stands_where_its_drag_published() {
        let mut scene = SceneState::default();

        assert!(scene.place("scene/carry-one", AT));
        assert_eq!(scene.get("gallery.scene.one"), Some(ReadValue::Point(AT)));
    }

    /// The two placements are two points, so moving one leaves the other where
    /// it was.
    #[kithara::test]
    fn moving_one_placement_leaves_the_other_standing() {
        let mut scene = SceneState::default();
        let before = scene.get("gallery.scene.two");

        scene.place("scene/carry-one", AT);

        assert_eq!(scene.get("gallery.scene.two"), before);
    }

    #[kithara::test]
    fn a_path_the_scene_does_not_hold_moves_nothing() {
        let mut scene = SceneState::default();
        let before = scene.get("gallery.scene.one");

        assert!(!scene.place("scene/carry-nine", AT));
        assert_eq!(scene.get("gallery.scene.one"), before);
    }

    #[kithara::test]
    fn the_press_turns_the_flag_the_artwork_switches_on() {
        let mut scene = SceneState::default();

        assert_eq!(
            scene.get("gallery.scene.sparked"),
            Some(ReadValue::Bool(false))
        );
        assert!(scene.activate("scene/switch"));
        assert_eq!(
            scene.get("gallery.scene.sparked"),
            Some(ReadValue::Bool(true))
        );
    }

    #[kithara::test]
    fn a_press_elsewhere_leaves_the_artwork_alone() {
        let mut scene = SceneState::default();

        assert!(!scene.activate("scene/carry-one"));
        assert_eq!(
            scene.get("gallery.scene.sparked"),
            Some(ReadValue::Bool(false))
        );
    }
}

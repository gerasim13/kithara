/// Who answers pointer input for the subtree a node renders into: the leaf
/// itself, or the retained engine hosting it.
#[derive(Clone, Copy)]
pub(crate) enum InputOwner {
    Leaf,
    Engine,
}

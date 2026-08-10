use super::{controls::MasonryControl, custom::HostAction};
use crate::{
    atoms::{track_list::face::TrackList as TrackListFace, tree::face::Tree as TreeFace},
    draw::{DrawList, Rect},
    interact::{Hit, Input, Outcome},
    render::{
        Reads, Skin,
        hosted::{TrackListPlan, TreePlan},
    },
    text::TextContext,
};

pub(crate) type TrackListLeaf = ProjectedLeaf<TrackListPlan>;
pub(crate) type TreeLeaf = ProjectedLeaf<TreePlan>;

pub(crate) struct ProjectedLeaf<P> {
    plan: P,
    text: TextContext,
}

pub(crate) trait Projected {
    fn draw_list(&self, text: &mut TextContext, bounds: Rect) -> DrawList;
    fn refresh(&self, reads: &dyn Reads) -> bool;
}

impl<P> ProjectedLeaf<P>
where
    P: Projected,
{
    pub(crate) fn new(plan: P, skin: &Skin) -> Self {
        Self {
            plan,
            text: TextContext::from(skin.text_resources()),
        }
    }
}

impl<P> MasonryControl for ProjectedLeaf<P>
where
    P: Projected,
{
    fn draw_list(&mut self, bounds: Rect) -> DrawList {
        self.plan.draw_list(&mut self.text, bounds)
    }

    fn input(&mut self, _input: Input<'_>, _hit: &Hit) -> Outcome<HostAction> {
        Outcome::IGNORED
    }

    fn accepts_input(&self) -> bool {
        false
    }

    fn refresh(&mut self, reads: &dyn Reads) -> bool {
        self.plan.refresh(reads)
    }
}

impl Projected for TrackListPlan {
    fn draw_list(&self, text: &mut TextContext, bounds: Rect) -> DrawList {
        let Some(drawn) = self.drawn() else {
            return DrawList::default();
        };
        TrackListFace::commands(&self.picture(), text, bounds, &drawn)
    }

    fn refresh(&self, reads: &dyn Reads) -> bool {
        self.refresh(reads)
    }
}

impl Projected for TreePlan {
    fn draw_list(&self, text: &mut TextContext, bounds: Rect) -> DrawList {
        let Some(drawn) = self.drawn() else {
            return DrawList::default();
        };
        TreeFace::commands(&self.picture(), text, bounds, &drawn)
    }

    fn refresh(&self, reads: &dyn Reads) -> bool {
        self.refresh(reads)
    }
}

use super::{SlotWrites, machine::Context};
use crate::{
    error::UiDocError,
    ids::NodeId,
    module::{BindingRef, ControlNode},
    size::SizeSpec,
};

#[derive(Clone, Copy)]
pub(super) struct ControlFields<'a> {
    pub(super) id: &'a NodeId,
    pub(super) read: Option<&'a BindingRef>,
    pub(super) size: Option<SizeSpec>,
    pub(super) write: Option<&'a BindingRef>,
}

#[derive(Default)]
pub(super) struct ExtraBindings {
    pub(super) active: Option<BindingRef>,
    pub(super) columns_state: Option<BindingRef>,
    pub(super) status: Option<BindingRef>,
    pub(super) query: Option<BindingRef>,
    pub(super) scope: Option<BindingRef>,
    pub(super) zoom: Option<BindingRef>,
    pub(super) writes: ExtraWrites,
    pub(super) uniforms: Vec<(String, BindingRef)>,
}

/// The owned side of [`SlotWrites`] that a control's own fields fill.
#[derive(Default)]
pub(super) struct ExtraWrites {
    pub(super) zoom: Option<BindingRef>,
    pub(super) loop_start: Option<BindingRef>,
    pub(super) loop_end: Option<BindingRef>,
    pub(super) query: Option<BindingRef>,
    pub(super) width: Option<BindingRef>,
    pub(super) toggle: Option<BindingRef>,
}

#[derive(Clone, Copy)]
pub(super) struct ExtraBindingRefs<'a> {
    pub(super) active: Option<&'a BindingRef>,
    pub(super) columns_state: Option<&'a BindingRef>,
    pub(super) status: Option<&'a BindingRef>,
    pub(super) query: Option<&'a BindingRef>,
    pub(super) scope: Option<&'a BindingRef>,
    pub(super) zoom: Option<&'a BindingRef>,
    pub(super) writes: SlotWrites<'a>,
}

impl ExtraBindings {
    pub(super) fn as_refs(&self) -> ExtraBindingRefs<'_> {
        let writes = &self.writes;
        ExtraBindingRefs {
            columns_state: self.columns_state.as_ref(),
            status: self.status.as_ref(),
            query: self.query.as_ref(),
            scope: self.scope.as_ref(),
            zoom: self.zoom.as_ref(),
            writes: SlotWrites {
                zoom: writes.zoom.as_ref(),
                loop_start: writes.loop_start.as_ref(),
                loop_end: writes.loop_end.as_ref(),
                query: writes.query.as_ref(),
                width: writes.width.as_ref(),
                toggle: writes.toggle.as_ref(),
                ..SlotWrites::default()
            },
            active: self.active.as_ref(),
        }
    }

    pub(super) fn substitute(
        context: &Context<'_>,
        control: &ControlNode,
        path: &str,
    ) -> Result<Self, UiDocError> {
        let substitute =
            |declared: &Option<BindingRef>| substituted(declared.as_ref(), context, path);
        let mut extra = Self::default();
        match control {
            ControlNode::Table {
                columns_state,
                status,
                write_width,
                ..
            } => {
                extra.columns_state = substitute(columns_state)?;
                extra.status = substitute(status)?;
                extra.writes.width = substitute(write_width)?;
            }
            ControlNode::Tree {
                query,
                write_query,
                toggle,
                ..
            } => {
                extra.query = substitute(query)?;
                extra.writes.query = substitute(write_query)?;
                extra.writes.toggle = substitute(toggle)?;
            }
            ControlNode::ContextBar { scope, .. } => extra.scope = substitute(scope)?,
            ControlNode::Wave {
                zoom,
                write_zoom,
                write_loop_start,
                write_loop_end,
                ..
            } => {
                extra.zoom = substitute(zoom)?;
                extra.writes.zoom = substitute(write_zoom)?;
                extra.writes.loop_start = substitute(write_loop_start)?;
                extra.writes.loop_end = substitute(write_loop_end)?;
            }
            ControlNode::Text { active, .. }
            | ControlNode::Glyph { active, .. }
            | ControlNode::Lottie { active, .. }
            | ControlNode::StatusDot { active, .. } => extra.active = substitute(active)?,
            ControlNode::Shader { uniforms, .. } => {
                extra.uniforms = uniforms
                    .iter()
                    .map(|(name, binding)| Ok((name.clone(), context.substitute(binding, path)?)))
                    .collect::<Result<Vec<_>, UiDocError>>()?;
            }
            _ => {}
        }
        Ok(extra)
    }
}

impl<'a> ControlFields<'a> {
    pub(super) const fn new(
        id: &'a NodeId,
        size: Option<SizeSpec>,
        read: Option<&'a BindingRef>,
        write: Option<&'a BindingRef>,
    ) -> Self {
        Self {
            id,
            read,
            size,
            write,
        }
    }
}

fn substituted(
    declared: Option<&BindingRef>,
    context: &Context<'_>,
    path: &str,
) -> Result<Option<BindingRef>, UiDocError> {
    declared
        .map(|binding| context.substitute(binding, path))
        .transpose()
}

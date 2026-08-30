use crate::{error::UiDocError, ids::SourceUri};

#[derive(Clone, Debug, Default)]
pub(crate) struct NodePath(Vec<String>);

impl NodePath {
    pub(crate) fn push(&self, segment: impl Into<String>) -> Self {
        let mut next = self.0.clone();
        next.push(segment.into());
        Self(next)
    }

    pub(crate) fn render(&self) -> String {
        if self.0.is_empty() {
            "root".to_owned()
        } else {
            format!("root/{}", self.0.join("/"))
        }
    }
}

pub(crate) fn check_block_path(path: &str, origin: &SourceUri) -> Result<(), UiDocError> {
    let reason = if path.contains('.') {
        "block address must not contain '.'"
    } else if path.contains('@') {
        "block address must not contain '@'"
    } else {
        return Ok(());
    };
    Err(UiDocError::InvalidId {
        origin: origin.clone(),
        id: path.to_owned(),
        reason: reason.to_owned(),
    })
}

pub(super) fn check_block_id(id: &str, origin: &SourceUri) -> Result<(), UiDocError> {
    check_id(id, origin)?;
    check_block_path(id, origin)
}

pub(super) fn check_id(id: &str, origin: &SourceUri) -> Result<(), UiDocError> {
    let reason = if id.is_empty() {
        Some("id must not be empty")
    } else if id.contains('/') {
        Some("id must not contain '/'")
    } else if id.starts_with('$') {
        Some("id must not start with '$'")
    } else {
        None
    };
    if let Some(reason) = reason {
        return Err(UiDocError::InvalidId {
            origin: origin.clone(),
            id: id.to_owned(),
            reason: reason.to_owned(),
        });
    }
    Ok(())
}

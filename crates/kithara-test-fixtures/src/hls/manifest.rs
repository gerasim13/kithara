use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Manifest {
    pub(crate) master: String,
    pub(crate) resources: Vec<Resource>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Resource {
    pub(crate) content_type: String,
    pub(crate) file: String,
    pub(crate) route: String,
}

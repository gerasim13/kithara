use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub master: String,
    pub resources: Vec<Resource>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Resource {
    pub content_type: String,
    pub file: String,
    pub route: String,
}

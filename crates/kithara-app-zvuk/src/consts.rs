pub(crate) const ENDPOINT: &str = "https://zvuk.com/api/v1/graphql/";
pub(crate) const TRACK_FIELDS: &str = "id title duration artists { title } release { title image { src } } collectionItemData { itemStatus }";
pub(crate) const MEDIA_FIELDS: &str = "... on Track { id streamV3 { hls expire } }";

pub(crate) use ::reqwest::Client;

use crate::{metrics::ConnectionMetrics, types::NetOptions};

pub(crate) type BackendError = ::reqwest::Error;

pub(crate) fn build_client(
    _options: &NetOptions,
    _metrics: &ConnectionMetrics,
) -> Result<Client, BackendError> {
    Client::builder().build()
}

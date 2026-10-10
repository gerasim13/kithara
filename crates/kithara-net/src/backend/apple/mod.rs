mod client;
mod delegate;
mod request;
mod response;
mod session;
mod stream;

pub use client::HttpClient;

#[cfg(all(test, feature = "client-apple", target_os = "macos"))]
mod tests;

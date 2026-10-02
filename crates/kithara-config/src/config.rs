/// Retained configuration that exposes an owned control-thread snapshot.
///
/// Snapshotting may clone or allocate and does not imply realtime safety.
/// Validation, preparation and application stay with the domain owner.
/// ```compile_fail
/// #[derive(kithara_config::Config)]
/// struct Resource<T> { #[config(value)] resource: T }
/// ```
pub trait Config {
    /// Readable values; construction-only resources and secrets are excluded.
    type Values;

    /// Reads values without exposing excluded construction inputs.
    /// ```compile_fail
    /// #[derive(kithara_config::Config)]
    /// struct Secret { #[config(skip = "credential")] token: String }
    /// let config = Secret::builder().token(String::from("private")).build();
    /// let _ = kithara_config::Config::values(&config).token;
    /// ```
    fn values(&self) -> Self::Values;
}

/// An owner that retains the configuration governing its behavior.
///
/// Consumers should read settings through this reference. A snapshot from
/// [`Config::values`] is for observation, not a second mutable source.
pub trait ConfigOwner {
    /// The retained configuration type.
    type Config: Config;

    /// The configuration used by this owner.
    fn config(&self) -> &Self::Config;
}

/// A configuration with typed, validated live changes.
///
/// Domain preparation and publication still happen at the owner's boundary.
pub trait UpdatableConfig: Config {
    /// A requested change.
    type Update;
    /// Domain validation error, or [`core::convert::Infallible`].
    type Error;

    /// Commits an accepted change to this configuration.
    ///
    /// On error, the configuration keeps its previous accepted values.
    ///
    /// # Errors
    ///
    /// Returns the configuration's domain validation error when the requested
    /// values are invalid.
    fn apply_update(&mut self, update: Self::Update) -> Result<(), Self::Error>;
}

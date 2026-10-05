#[cfg(feature = "stretch-bungee")]
mod bungee;
#[cfg(feature = "stretch-signalsmith")]
mod signalsmith;

#[cfg(feature = "stretch-bungee")]
pub(crate) use bungee::BungeeElastic;
#[cfg(feature = "stretch-signalsmith")]
pub(crate) use signalsmith::SignalsmithElastic;

#[cfg(feature = "stretch-identity")]
mod identity;
#[cfg(feature = "stretch-identity")]
pub(crate) use identity::IdentityElastic;

#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
mod varispeed;
#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
pub(crate) use varispeed::VarispeedElastic;

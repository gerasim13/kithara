#[cfg(feature = "stretch-bungee")]
mod bungee;
#[cfg(feature = "stretch-signalsmith")]
mod signalsmith;

#[cfg(feature = "stretch-bungee")]
pub(crate) use bungee::BungeeElastic;
#[cfg(feature = "stretch-signalsmith")]
pub(crate) use signalsmith::SignalsmithElastic;

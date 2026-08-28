// The watchdog is off under Miri as well as when the feature is: it measures
// real seconds and ends a run by aborting the process, and Miri interprets
// instructions some hundred times slower than the machine executes them. What
// it would report there is the interpreter, and the way it reports is to take
// the test binary down with it.
#[cfg(all(feature = "hang", not(miri)))]
pub(crate) mod real;
#[cfg(all(feature = "hang", not(miri)))]
pub use real::*;

#[cfg(any(not(feature = "hang"), miri))]
mod noop;
#[cfg(any(not(feature = "hang"), miri))]
pub use noop::*;

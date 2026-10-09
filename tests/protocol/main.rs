//! Protocol checks: the server binary over stdio, against a fake niri, a fake Noctalia and
//! fake programs in a directory of the test's own. Nothing here reaches the real desktop,
//! clipboard or audit log.
//!
//! Every module is `cfg(test)`, which always holds here, so Clippy treats the helpers as
//! test code too.

#[cfg(test)]
mod client;
#[cfg(test)]
mod fixture;
#[cfg(test)]
mod niri;
#[cfg(test)]
mod noctalia;

#[cfg(test)]
mod act;
#[cfg(test)]
mod audit;
#[cfg(test)]
mod cancellation;
#[cfg(test)]
mod control;
#[cfg(test)]
mod errors;
#[cfg(test)]
mod events;
#[cfg(test)]
mod lease;
#[cfg(test)]
mod lifecycle;
#[cfg(test)]
mod recover;
#[cfg(test)]
mod screenshot;
#[cfg(test)]
mod session;
#[cfg(test)]
mod shell;
#[cfg(test)]
mod wait;

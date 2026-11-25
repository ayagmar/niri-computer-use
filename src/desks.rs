//! Background desks: a nested niri the agent owns, beside the owner's desktop, so an
//! agent can work while the owner keeps using theirs. `desk start` runs a desk's
//! processes under one supervisor; `serve --desk` joins a ready one. See
//! `docs/architecture.md`, "Desks".

mod accessibility;
pub(crate) mod check;
pub(crate) mod cli;
mod config;
pub(crate) mod environment;
pub(crate) mod hold;
pub(crate) mod join;
pub(crate) mod layout;
pub(crate) mod name;
mod output;
mod programs;
pub(crate) mod record;
pub(crate) mod scope;
mod serial;
mod steps;
mod supervisor;

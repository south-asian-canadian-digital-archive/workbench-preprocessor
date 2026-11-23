//! `organise` as a library: the CLI definitions plus re-exports of the
//! workspace crates, so `organise::sacda::...` works for the tests and for
//! anyone using it as a dependency.

pub mod cli;

pub use cli::{Cli, Command};
pub use sacda;
pub use sacda_tui as tui;

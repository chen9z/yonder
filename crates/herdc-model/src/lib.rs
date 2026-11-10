//! Herdr's world as herdr-app keeps it: spaces, tabs and their split trees
//! (`layout`, `model`), and the machine layout documents shared through
//! each core's `herdr-app` document (`mirror`); what runs in a terminal
//! (`programs`) and the shell agents start in (`shell`). Vendored so every
//! screen agrees on the document format and the rules (see NOTICE).

// Vendored as upstream has it, so their diffs apply cleanly.
#![allow(clippy::large_enum_variant)]

pub mod layout;
pub mod mirror;
pub mod model;
pub mod programs;
pub mod shell;

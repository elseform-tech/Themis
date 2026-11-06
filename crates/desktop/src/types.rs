//! Wire types: an exact serde mirror of `web/src/lib/types.ts`.
//!
//! That file is the contract's source of truth. Every struct here serializes
//! with the same field names (the TS side uses `snake_case` throughout), and
//! [`ThreadEvent`] is internally tagged on `kind` with the same variant names.

mod approvals;
mod automations;
mod chat;
mod common;
mod diffs;
mod events;
mod settings;
mod skills;
mod threads;

pub use approvals::*;
pub use automations::*;
pub use chat::*;
pub use common::*;
pub use diffs::*;
pub use events::*;
pub use settings::*;
pub use skills::*;
pub use threads::*;

#[cfg(test)]
mod tests;

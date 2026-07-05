//! fluxfang-core: pure, no-I/O domain logic shared across the app.
//!
//! Defines the one shared rule/condition format (used by emitter matching,
//! emission filtering, and alert content-matching) and the per-data-source
//! field catalog. No async, no DB, no side effects — see later tasks (3.2
//! `eval`, 3.3 SQL translation, 2.1 password hashing, 8.1 secret encryption)
//! for the rest of this crate.

pub mod catalog;
pub mod rule;

pub use catalog::{catalog_for, FieldDef, FieldType};
pub use rule::{Condition, MatchMode, Op, Rule};

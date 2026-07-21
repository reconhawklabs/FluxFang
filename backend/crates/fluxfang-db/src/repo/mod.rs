//! One module per aggregate repository. Each file owns the SQL for a single
//! table (or tightly-related pair) and exposes a unit struct with async
//! associated functions taking `&PgPool` — no repo holds its own pool, so
//! callers are free to share one pool across every repo.
//!
//! ## sqlx: runtime `query_as` vs. compile-time `query_as!`
//!
//! These repos use the **runtime** `sqlx::query_as::<_, T>()` /
//! `sqlx::query()` functions, not the compile-time-checked `query!`/
//! `query_as!` macros. The macros need a live `DATABASE_URL` (or a
//! committed `.sqlx` offline cache) available at `cargo build`/`cargo check`
//! time for every workspace member, which would force CI and every
//! contributor's machine to have Postgres reachable (or a cache kept in
//! sync) just to compile. The runtime functions push that requirement out
//! to `cargo test` only, where it's already required for the DB round-trip
//! tests. The trade-off is column/type typos are caught at test time
//! instead of compile time — acceptable given the repo test suite exercises
//! every query.

pub mod ai_audit;
pub mod alert_method;
pub mod alert_rule;
pub mod app_config;
pub mod cotravel;
pub mod data_source;
pub mod emission;
pub mod emitter;
pub mod emitter_association;
pub mod entity;
pub mod location;
pub mod notification;
pub mod sensor;
pub mod session;
pub mod zone;
pub mod zone_membership;

pub use ai_audit::AiAuditRepo;
pub use alert_method::AlertMethodRepo;
pub use alert_rule::AlertRuleRepo;
pub use app_config::AppConfigRepo;
pub use cotravel::{CoTravelCandidate, CoTravelFilter, CoTravelRepo, IgnoredEmitter};
pub use data_source::DataSourceRepo;
pub use emission::EmissionRepo;
pub use emitter::EmitterRepo;
pub use emitter_association::{AssociatedEmitter, EmitterAssociationRepo};
pub use entity::EntityRepo;
pub use location::LocationRepo;
pub use notification::NotificationRepo;
pub use sensor::SensorRepo;
pub use session::SessionRepo;
pub use zone::ZoneRepo;
pub use zone_membership::ZoneMembershipRepo;

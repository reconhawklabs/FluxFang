//! Ingest: turns raw capture-layer data into stored, session-bounded rows.
//!
//! Task 5.1 adds [`session::SessionManager`] (session bounding + the
//! host's own GPS trajectory log). Later tasks add the emission ingest
//! pipeline (5.2), alert evaluation (5.3), and zone-membership tracking
//! (5.4) alongside it.

pub mod session;

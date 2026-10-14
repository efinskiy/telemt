//! Immutable shared configuration versions and explicit managed enrollment.
pub(crate) mod model;
mod import;
mod validation;
mod rollout;
mod routes;
pub(crate) use routes::route;
pub(crate) use rollout::{tick, enqueue, lock_profile};
pub(crate) use model::domain;

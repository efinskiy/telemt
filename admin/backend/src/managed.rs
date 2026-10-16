//! Outbound node management: enrolled identities, immutable bundles and bounded local API calls.
mod enrollment;
mod addresses;
mod transport;
pub(crate) mod admission;
pub(crate) use enrollment::{route,agent};
pub(crate) use transport::request;

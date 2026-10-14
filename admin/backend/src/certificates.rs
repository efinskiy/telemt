//! Central certificate lifecycle; only the coordinator holds ACME and DNS credentials.
mod routes;
mod worker;
mod provider;
pub(crate) use routes::route;
pub(crate) use worker::tick;

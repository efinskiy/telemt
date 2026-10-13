//! Executes the production limiter on platforms where unrelated proxy modules cannot compile.
#![allow(dead_code, unused_imports)]
mod config;
mod traffic_limiter;

#[cfg(test)]
mod disabled_benchmark;

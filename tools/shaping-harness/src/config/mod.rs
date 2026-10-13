//! Minimal dependency contracts; the shaper and limiter themselves are linked from production source.
use serde::{Serialize,Deserialize};
pub(crate) const MAX_RATE_LIMIT_BPS: u64 = 100_000_000_000;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct RateLimitBps { pub up_bps:u64, pub down_bps:u64 }
#[derive(Clone,Debug,PartialEq,Eq,Hash)]
pub enum CidrRateLimitKey { Network(ipnetwork::IpNetwork), AutoV4(u8), AutoV6(u8), AutoDual(u8) }
#[path = "../../../../src/config/types/shaping.rs"]
mod shaping;
pub use shaping::ShapingConfig;

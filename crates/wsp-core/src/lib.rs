#![deny(unsafe_code)]

pub mod agentmd;
pub mod config;
pub mod discovery;
pub mod filelock;
pub mod gc;
pub mod git;
pub mod giturl;
pub mod lang;
pub mod mirror;
pub mod output;
pub mod template;
pub(crate) mod util;
pub mod workspace;

// Test helpers exposed to dependent crates (e.g. crates/wsp) via the
// "test-utils" feature. Plain #[cfg(test)] is NOT sufficient here — items
// gated with cfg(test) in a library are invisible when a dependent crate
// compiles its own test suite. Always use any(test, feature = "test-utils")
// for helpers that crates/wsp tests need to call.
#[cfg(any(test, feature = "test-utils"))]
pub mod testutil;

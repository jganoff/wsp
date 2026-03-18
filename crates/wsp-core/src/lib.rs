//! Core library for the [wsp](https://github.com/jganoff/wsp) multi-repo workspace manager.
//!
//! `wsp-core` provides all the building blocks for creating, managing, and
//! querying multi-repo workspaces programmatically — without shelling out to
//! the `wsp` binary.
//!
//! # Quick start
//!
//! ```no_run
//! use wsp_core::config::Paths;
//! use wsp_core::workspace;
//! use std::collections::BTreeMap;
//!
//! // Resolve paths from the environment (XDG_DATA_HOME / HOME).
//! let paths = Paths::resolve().unwrap();
//!
//! // List all workspaces.
//! let names = workspace::list_all(&paths.workspaces_dir).unwrap();
//!
//! // Detect which workspace the current directory belongs to.
//! if let Ok(ws_dir) = workspace::detect(std::env::current_dir().unwrap().as_path()) {
//!     let meta = workspace::load_metadata(&ws_dir).unwrap();
//!     println!("in workspace: {}", meta.name);
//! }
//! ```
//!
//! # Module overview
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`config`] | Global config (`~/.local/share/wsp/config.yaml`) and path resolution |
//! | [`workspace`] | Create, list, delete, and inspect workspaces |
//! | [`template`] | Template lifecycle — import, export, apply |
//! | [`filelock`] | Atomic read-modify-write for config and metadata files |
//! | [`git`] | Git operations via `std::process::Command` |
//! | [`giturl`] | Parse and validate SSH/HTTPS git URLs |
//! | [`mirror`] | Bare mirror management |
//! | [`gc`] | Deferred deletion (garbage collection) for workspaces |
//! | [`discovery`] | Discover `.wsp.yaml` templates in repos and mirrors |
//! | [`lang`] | Pluggable language integrations (Go `go.work`, etc.) |
//! | [`output`] | JSON-serializable output structs shared with the `wsp` CLI |
//! | [`agentmd`] | Generate `AGENTS.md` / `CLAUDE.md` workspace context files |
//!
//! # File locking
//!
//! All read-modify-write operations on config or metadata must go through
//! [`filelock::with_config`], [`filelock::with_metadata`], or
//! [`filelock::with_template`].  Never call `load` → modify → `save` directly
//! outside of tests — doing so races with concurrent `wsp` invocations.
//!
//! # Security model
//!
//! Functions in the [`git`] module (including [`git::run`]) accept arguments
//! that are passed directly to the `git` subprocess **without shell
//! interpretation**, so injection via argument values is not possible.
//! However, several git config keys (`core.sshCommand`, `core.hooksPath`,
//! etc.) cause git to execute arbitrary commands on the next operation.
//! **Never derive git config keys or values from untrusted input.**
//! Use [`config::validate_git_config_key`] at the boundary where keys enter
//! your application; [`workspace::apply_git_config`] enforces this
//! automatically.

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

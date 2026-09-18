//! bridgewatch's engine: a bridge-aware model of GitLab CI.
//!
//! Every other GitLab tray monitor defines "status" as the newest pipeline on a
//! branch. On a parent/child estate that is wrong twice over. The newest
//! pipeline is frequently an hourly **schedule** that is red by design, and the
//! parent's own status says nothing about what happened inside the child
//! pipelines its `trigger:*` jobs created — so nobody can say "the website
//! deployed while the android bridge failed". bridgewatch walks the bridges,
//! filters by pipeline source, and derives a deploy verdict from marker jobs the
//! user names.
//!
//! The crate is GUI-free on purpose: it has no Tauri dependency, and everything
//! above the [`client::Transport`] seam is tested against recorded fixtures of
//! real pipelines.
//!
//! # Shape
//!
//! - [`config`] — the TOML file, its JSON Schema, and format-preserving edits.
//! - [`token`] — resolving a credential without ever storing or logging one.
//! - [`client`] — the five GitLab endpoints, behind a transport seam.
//! - [`verdict`] — pure functions from responses to the view model.
//! - [`poll`] — what to fetch, how often, and what to keep.
//! - [`notify`] — what is worth interrupting somebody over.
//! - [`wizard`] — the first-run setup wizard's steps, with no UI in them.
//!
//! # Getting a snapshot
//!
//! ```no_run
//! use bridgewatch_core::{config, poll::Poller, token::SystemTokenProvider};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let loaded = config::load(&config::resolve_path(None))?;
//! let mut poller = Poller::from_config(&loaded.config, &SystemTokenProvider)?;
//! let mut snapshots = poller.subscribe();
//! let tick = poller.tick().await;
//! println!("{}", tick.snapshot.icon_state);
//! # let _ = snapshots.borrow_and_update();
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod client;
pub mod config;
pub mod model;
pub mod notify;
pub mod poll;
pub mod status;
pub mod token;
pub mod verdict;
pub mod wizard;

pub use config::Config;
pub use verdict::{IconState, Snapshot};

/// The crate version, for user agents and `--version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The README's examples, compiled so they cannot rot.
///
/// P2 builds the GUI from that file; an example in it that does not compile is
/// worse than no example.
#[doc = include_str!("../README.md")]
#[cfg(doctest)]
pub struct ReadmeExamples;

//! Deciding what to fetch on a tick, before fetching any of it.
//!
//! Separating the plan from the execution is what makes "an idle tick costs one
//! request per watch" a testable claim rather than a hope.

use crate::config::WatchRules;
use crate::model::{Bridge, Pipeline};
use crate::poll::cache::PipelineCache;

/// What one pipeline needs this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelinePlan {
    /// The pipeline id.
    pub id: u64,
    /// Fetch `/jobs`.
    pub fetch_jobs: bool,
    /// Fetch `/bridges`.
    pub fetch_bridges: bool,
}

impl PipelinePlan {
    /// How many requests this pipeline costs.
    pub fn request_count(&self) -> usize {
        usize::from(self.fetch_jobs) + usize::from(self.fetch_bridges)
    }
}

/// One watch's plan for one tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Pipelines that need refreshing, in display order.
    pub pipelines: Vec<PipelinePlan>,
    /// Pipelines served entirely from cache.
    pub cached: Vec<u64>,
}

impl Plan {
    /// Requests this plan costs, including the one list request every tick
    /// makes.
    pub fn request_count(&self) -> usize {
        1 + self
            .pipelines
            .iter()
            .map(PipelinePlan::request_count)
            .sum::<usize>()
    }

    /// True when nothing but the list request is needed.
    pub fn is_idle(&self) -> bool {
        self.pipelines.is_empty()
    }
}

/// GitLab.com's documented ceiling for authenticated API traffic, per user per
/// minute. Self-managed instances set their own; this is the one a default
/// install meets, and the budget test measures against it.
pub const GITLAB_COM_API_PER_MINUTE: usize = 2000;

/// The Free plan's burst limit in GitLab.com's PROPOSED per-plan rate limits,
/// published but not in effect as of 2026-09-18. Kept beside the current one so
/// the budget test states the risk instead of hiding it.
pub const GITLAB_COM_FREE_PROPOSED_PER_MINUTE: usize = 100;

/// The worst-case request count for one tick, in the shape the poller spends
/// it.
///
/// `watches[w][p]` is the number of dived children of the `p`-th pipeline that
/// watch `w` has to re-fetch this tick. Each watch costs one list; each such
/// pipeline costs its `/jobs` and `/bridges`; each child costs one `/jobs`.
/// A live pipeline is re-fetched every tick, so for a live estate this is the
/// steady state, not a cold-start spike. `dive.depth` above 1 adds a
/// `/bridges` per walked child and is not modelled here.
pub fn live_tick_requests(watches: &[&[usize]]) -> usize {
    watches
        .iter()
        .map(|pipelines| 1 + pipelines.iter().map(|children| 2 + children).sum::<usize>())
        .sum()
}

/// Requests a minute for `per_tick` requests every `interval_secs`, rounding
/// the tick count up. A zero interval is read as one second, as the policy
/// itself does.
pub fn per_minute(interval_secs: u64, per_tick: usize) -> usize {
    let interval = interval_secs.max(1) as usize;
    60usize.div_ceil(interval) * per_tick
}

/// Plan the pipeline fetches for a tick, given the list rows it just received.
pub fn plan(rows: &[Pipeline], cache: &PipelineCache) -> Plan {
    let mut pipelines = Vec::new();
    let mut cached = Vec::new();
    for row in rows {
        if cache.needs_refresh(row) {
            pipelines.push(PipelinePlan {
                id: row.id,
                fetch_jobs: true,
                fetch_bridges: true,
            });
        } else {
            cached.push(row.id);
        }
    }
    Plan { pipelines, cached }
}

/// Which children to fetch jobs for, given this tick's bridges and the previous
/// tick's.
///
/// A child is fetched when the dive rules select it **and** either its status is
/// live, or its status changed since the cached copy, or nothing is held for it
/// at all. A settled child whose status has not moved cannot have new jobs.
///
/// ⛔ `held` is the third condition and it is the one that makes a failed child
/// request recoverable. A timeout or a 429 on `/pipelines/<child>/jobs` leaves
/// the detail stored without that child; if "fetched before" were inferred from
/// the bridge list alone, the next tick would see an unchanged settled bridge
/// and never ask again, so one transient error would hide a failed child for the
/// life of the process.
pub fn child_fetches(
    bridges: &[Bridge],
    previous: Option<&[Bridge]>,
    held: &std::collections::HashSet<u64>,
    rules: &WatchRules,
) -> Vec<(u64, Option<u64>)> {
    let mut out = Vec::new();
    for bridge in bridges {
        if !rules.should_dive(&bridge.name, &bridge.status) {
            continue;
        }
        let Some(down) = &bridge.downstream_pipeline else {
            continue;
        };
        let changed = match previous.and_then(|p| p.iter().find(|b| b.name == bridge.name)) {
            None => true,
            Some(prev) => {
                prev.status != bridge.status
                    || prev.downstream_pipeline.as_ref().map(|d| (&d.status, d.id))
                        != Some((&down.status, down.id))
            }
        };
        if bridge.status.is_live() || down.status.is_live() || changed || !held.contains(&down.id) {
            out.push((down.id, down.project_id));
        }
    }
    out
}

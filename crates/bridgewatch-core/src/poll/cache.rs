//! The pipeline cache.
//!
//! A settled pipeline's jobs cannot change, so re-fetching them every minute is
//! pure rate-limit spend. The cache keys on pipeline id and stores the
//! `(status, updated_at)` the snapshot was built from; a settled pipeline is
//! re-fetched only when its list row's `updated_at` moves.

use std::collections::{HashMap, HashSet};

use crate::model::{Pipeline, PipelineDetail};
use crate::status::Status;

/// One cached pipeline, with the revision it was built from.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// Everything fetched for the pipeline.
    pub detail: PipelineDetail,
    /// The `(status, updated_at)` of the list row this was built from.
    pub revision: (Status, Option<String>),
    /// At least one request that makes up this detail failed, so what is stored
    /// is missing a child's jobs.
    ///
    /// ⛔ Without this the miss is permanent. A settled pipeline whose revision
    /// has not moved is never re-planned, so the child whose fetch timed out is
    /// never asked for again and the bridge keeps reading its own status — which
    /// for a `trigger:*` job without `strategy: depend` is `success` from the
    /// moment the child was created.
    pub incomplete: bool,
}

/// Pipeline details, keyed by pipeline id.
#[derive(Debug, Clone, Default)]
pub struct PipelineCache {
    entries: HashMap<u64, CacheEntry>,
}

impl PipelineCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The cached detail for a pipeline, if any.
    pub fn get(&self, id: u64) -> Option<&CacheEntry> {
        self.entries.get(&id)
    }

    /// Whether this list row needs its jobs and bridges fetched again.
    ///
    /// A live pipeline always does. A settled one does only when its revision
    /// has moved, which is the whole saving — unless what is cached is known to
    /// be incomplete, in which case the saving would be permanent silence.
    pub fn needs_refresh(&self, row: &Pipeline) -> bool {
        match self.entries.get(&row.id) {
            None => true,
            Some(entry) => {
                entry.incomplete || row.status.is_live() || entry.revision != row.revision()
            }
        }
    }

    /// Store a freshly fetched detail against the row it was built from.
    pub fn insert(&mut self, row: &Pipeline, detail: PipelineDetail) {
        self.insert_partial(row, detail, false)
    }

    /// The same, for a detail one of whose requests failed.
    ///
    /// `incomplete` is what forces the next tick to ask again.
    pub fn insert_partial(&mut self, row: &Pipeline, detail: PipelineDetail, incomplete: bool) {
        self.entries.insert(
            row.id,
            CacheEntry {
                detail,
                revision: row.revision(),
                incomplete,
            },
        );
    }

    /// Drop everything not in `keep`. Called once a tick so a cache cannot grow
    /// without bound on a busy project.
    pub fn retain(&mut self, keep: &HashSet<u64>) {
        self.entries.retain(|id, _| keep.contains(id));
    }

    /// How many pipelines are cached.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

//! How long a deploy usually takes, learned from the watch's own history.
//!
//! The tray answers "did my push deploy". While it is still running, the next
//! question is "how long until it does", and the watch already knows: every
//! pipeline it has seen deploy is one measurement of that, taken from data the
//! poller had fetched anyway. Nothing here sends a request.
//!
//! One sample is `marker.finished_at - pipeline.created_at` for a pipeline whose
//! deploy outcome is `live`, that is, from the push to the moment the marker
//! job finished. The estimate is the median of the last [`MAX_SAMPLES`], and it
//! is only offered once there are [`MIN_SAMPLES`] of them.
//!
//! ⚠️ Samples are keyed by the watch's `id`, which is the only identity a watch
//! has: renaming a watch starts its history again, and pointing an existing id
//! at another project keeps the old project's samples until newer ones push
//! them out. Neither is worth a second key in the file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::verdict::PipelineView;

/// How many samples a watch keeps. The newest pipelines win.
pub const MAX_SAMPLES: usize = 20;

/// How many samples a watch needs before it offers an estimate. Fewer than
/// three is not a typical duration, it is an anecdote.
pub const MIN_SAMPLES: usize = 3;

/// What a running row shows: the usual duration and how far in it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Eta {
    /// The median of the watch's samples, in seconds.
    pub typical_secs: u64,
    /// Seconds since the pipeline was created, at the poll that built the row.
    pub elapsed_secs: u64,
}

/// One pipeline's push-to-deployed time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sample {
    /// The pipeline it was measured on, which is what makes recording it twice
    /// a no-op.
    pub pipeline_id: u64,
    /// `marker.finished_at - pipeline.created_at`, in whole seconds.
    pub secs: u64,
}

/// The median of `samples`, or `None` for none. An even count takes the mean
/// of the middle two, rounded down.
pub fn median(samples: &[u64]) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let mid = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        // Widened, so two samples near `u64::MAX` cannot overflow the sum.
        ((u128::from(sorted[mid - 1]) + u128::from(sorted[mid])) / 2) as u64
    } else {
        sorted[mid]
    })
}

/// The estimate for a running row, or `None` when there is nothing honest to
/// say.
///
/// * fewer than [`MIN_SAMPLES`] samples: `None`;
/// * `elapsed_secs` negative, which is clock skew between this machine and the
///   provider on a pipeline created a moment ago: clamped to 0;
/// * `elapsed_secs` more than twice the median: `None`. A deploy that has run
///   for twice its usual time is not "usually ~11m" any more; it is stuck or
///   unusual, and a typical figure beside it only reads as a broken promise.
pub fn estimate(samples: &[u64], elapsed_secs: i64) -> Option<Eta> {
    if samples.len() < MIN_SAMPLES {
        return None;
    }
    let typical_secs = median(samples)?;
    let elapsed_secs = u64::try_from(elapsed_secs).unwrap_or(0);
    if elapsed_secs > typical_secs.saturating_mul(2) {
        return None;
    }
    Some(Eta {
        typical_secs,
        elapsed_secs,
    })
}

/// Seconds from `from` to `to`, both RFC 3339, or `None` when either does not
/// parse. Negative when `to` is earlier.
pub fn seconds_between(from: &str, to: &str) -> Option<i64> {
    let from = DateTime::parse_from_rfc3339(from).ok()?;
    let to = DateTime::parse_from_rfc3339(to).ok()?;
    Some((to - from).num_seconds())
}

/// The sample one deployed pipeline gives, or `None` when it gives none.
///
/// ⛔ A missing or unreadable time is NO sample, not a guess. Falling back to
/// `updated_at` or to "now" would record how long ago the pipeline was polled
/// rather than how long it took to deploy, and on the first `check` after a
/// week away that is a week-long sample in a median of minutes. A duration that
/// is zero or negative is clock skew or a provider bug, and is dropped the same
/// way.
pub fn deploy_sample(created_at: Option<&str>, marker_finished_at: Option<&str>) -> Option<u64> {
    let secs = seconds_between(created_at?, marker_finished_at?)?;
    u64::try_from(secs).ok().filter(|s| *s > 0)
}

/// Whether a row is still on its way to a deploy, which is the only kind of row
/// that carries an estimate.
///
/// A marker in flight (`in_progress`) is the plain case. The other is a live
/// pipeline on a watch that HAS markers, none of which has been seen yet
/// (`absent`): the lane that carries the marker has not started, and that is
/// exactly the stretch of a push somebody is waiting through. Every other
/// outcome has settled (`live`, `failed`, `dead`, `canceled`) or cannot be read
/// (`unknown`), and an estimate beside it would be noise.
pub fn is_running(row: &PipelineView, has_markers: bool) -> bool {
    match row.deploy.as_str() {
        "in_progress" => true,
        "absent" => has_markers && row.live,
        _ => false,
    }
}

/// Every watch's samples, persisted beside the notification ledger.
///
/// ⚠️ One file for the app AND the CLI, where the ledger keeps two. The ledger
/// split because two writers each dropped the other's keys, which re-announced
/// pipelines on screen. Samples cannot be wrong that way: a sample is a fact
/// about one pipeline, so [`EtaHistory::save_merged`] re-reads the file and
/// takes the union before it writes, and a `bridgewatch check` both learns from
/// the app's history and adds to it. The only race left is two writes inside
/// the same few milliseconds, which costs one sample, not a wrong one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EtaHistory {
    /// Samples per watch id, in ascending pipeline id.
    #[serde(default)]
    pub watches: BTreeMap<String, Vec<Sample>>,
}

impl EtaHistory {
    /// `<state or data dir>/bridgewatch/eta.json`, the directory the
    /// notification ledger lives in.
    pub fn default_path() -> PathBuf {
        crate::notify::NotifyLedger::path_named("eta.json")
    }

    /// Load the history. A missing file is an empty history and says nothing,
    /// because that is every first run. An unreadable or corrupt one is an
    /// empty history with one `warn`: the estimate is a convenience, and a bad
    /// file must cost the samples, never the poll.
    pub fn load(path: &Path) -> Self {
        match Self::read(path) {
            Ok(history) => history,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "the deploy-time history cannot be read; starting it again"
                );
                Self::default()
            }
        }
    }

    fn read(path: &Path) -> std::io::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let mut history: Self = serde_json::from_str(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        // Whatever wrote it, the rules for a sample hold on the way in too.
        for samples in history.watches.values_mut() {
            normalise(samples);
        }
        Ok(history)
    }

    /// Record one sample. Returns `false`, and changes nothing, when that
    /// pipeline already has one, when `secs` is zero, or when the pipeline is
    /// older than every one of a full set of samples (it would be trimmed
    /// straight back out).
    pub fn record(&mut self, watch: &str, pipeline_id: u64, secs: u64) -> bool {
        if secs == 0 {
            return false;
        }
        let samples = self.watches.entry(watch.to_string()).or_default();
        if samples.iter().any(|s| s.pipeline_id == pipeline_id) {
            return false;
        }
        samples.push(Sample { pipeline_id, secs });
        normalise(samples);
        samples.iter().any(|s| s.pipeline_id == pipeline_id)
    }

    /// Has this pipeline been measured for this watch?
    pub fn contains(&self, watch: &str, pipeline_id: u64) -> bool {
        self.watches
            .get(watch)
            .is_some_and(|s| s.iter().any(|s| s.pipeline_id == pipeline_id))
    }

    /// A watch's samples, in seconds, oldest pipeline first.
    pub fn samples(&self, watch: &str) -> Vec<u64> {
        self.watches
            .get(watch)
            .map(|s| s.iter().map(|s| s.secs).collect())
            .unwrap_or_default()
    }

    /// Take the union with `other`. Where both hold a pipeline, `self` wins:
    /// it is the same measurement either way.
    pub fn merge(&mut self, other: &EtaHistory) {
        for (watch, theirs) in &other.watches {
            let ours = self.watches.entry(watch.clone()).or_default();
            for sample in theirs {
                if !ours.iter().any(|s| s.pipeline_id == sample.pipeline_id) {
                    ours.push(*sample);
                }
            }
            normalise(ours);
        }
    }

    /// Merge what is on disk into this history, then write the result.
    ///
    /// ⚠ Written to a sibling and renamed over the target, as the ledger is: an
    /// interrupted plain write leaves a truncated file, which `load` would read
    /// as corrupt and every sample would go with it. A file that exists and
    /// does not parse is replaced by this history rather than merged, and was
    /// already warned about by `load`.
    pub fn save_merged(&mut self, path: &Path) -> std::io::Result<()> {
        if let Ok(on_disk) = Self::read(path) {
            self.merge(&on_disk);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "eta.json".into());
        let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
        let result = std::fs::write(&temp, body).and_then(|()| std::fs::rename(&temp, path));
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }
}

/// Sort by pipeline id, drop zero-second and duplicate samples, and keep the
/// newest [`MAX_SAMPLES`].
///
/// ⛔ "Newest" is the highest pipeline id, not the last one recorded. By
/// insertion order a pipeline still on screen that had been trimmed out would
/// be recorded again on the next tick as the newest sample, and then again
/// after the next trim. By id it is the oldest, and is trimmed on arrival.
fn normalise(samples: &mut Vec<Sample>) {
    samples.retain(|s| s.secs > 0);
    samples.sort_by_key(|s| s.pipeline_id);
    samples.dedup_by_key(|s| s.pipeline_id);
    if samples.len() > MAX_SAMPLES {
        samples.drain(..samples.len() - MAX_SAMPLES);
    }
}

/// Seconds from `created_at` to `now`, or `None` when it does not parse.
pub fn elapsed_since(created_at: Option<&str>, now: DateTime<Utc>) -> Option<i64> {
    let created = DateTime::parse_from_rfc3339(created_at?).ok()?;
    Some((now - created.with_timezone(&Utc)).num_seconds())
}

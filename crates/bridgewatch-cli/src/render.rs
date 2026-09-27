//! Human-readable rendering of a [`Snapshot`].

use bridgewatch_core::eta::Eta;
use bridgewatch_core::verdict::{PipelineView, Snapshot, WatchView};

/// The full table `bridgewatch check` prints.
pub fn snapshot(s: &Snapshot) -> String {
    let mut out = String::new();
    out.push_str(&format!("icon: {}\n", s.icon_state));
    for watch in &s.watches {
        out.push_str(&watch_block(watch));
    }
    if !s.errors.is_empty() {
        out.push_str("\nerrors:\n");
        for e in &s.errors {
            out.push_str(&format!("  {e}\n"));
        }
    }
    out
}

fn watch_block(w: &WatchView) -> String {
    let mut out = String::new();
    let role = if w.role.is_primary() {
        "primary"
    } else {
        "secondary"
    };
    let icon = w.icon_state.map(|s| format!(" [{s}]")).unwrap_or_default();
    out.push_str(&format!("\n{} ({role}){icon}\n", w.id));
    if let Some(e) = &w.error {
        out.push_str(&format!("  error: {e}\n"));
    }
    if w.rows.is_empty() {
        out.push_str("  (no pipelines)\n");
    }
    for row in &w.rows {
        out.push_str(&row_block(row));
    }
    out
}

fn row_block(p: &PipelineView) -> String {
    let mut out = String::new();
    let source = p.source.as_deref().unwrap_or("?");
    out.push_str(&format!(
        "  #{} {} {} [{source}] {} -> {} (deploy: {})\n",
        p.id, p.sha7, p.ref_name, p.status, p.state, p.deploy
    ));
    if let Some(eta) = &p.eta {
        out.push_str(&format!("      eta: {}\n", eta_text(eta)));
    }
    if let Some(m) = &p.deploy_marker {
        out.push_str(&format!(
            "      marker: {} in pipeline {}\n",
            m.name, m.pipeline_id
        ));
    }
    for b in &p.bridges {
        let child = b
            .child_id
            .map(|c| format!("child {c}"))
            .unwrap_or_else(|| "no child".to_string());
        let detail = if b.verdict_jobs.is_empty() {
            String::new()
        } else {
            format!(" ({})", b.verdict_jobs.join(", "))
        };
        let dived = if b.dived { "" } else { " [not dived]" };
        out.push_str(&format!(
            "      {:<24} {:<22} {child}{detail}{dived}\n",
            b.name, b.verdict
        ));
    }
    if !p.failures.is_empty() {
        out.push_str(&format!("      failures: {}\n", p.failures.join(", ")));
    }
    if !p.warnings.is_empty() {
        out.push_str(&format!("      warnings: {}\n", p.warnings.join(", ")));
    }
    if !p.gates.is_empty() {
        out.push_str(&format!("      gates: {}\n", p.gates.join(", ")));
    }
    if !p.post_deploy_failures.is_empty() {
        out.push_str(&format!(
            "      after deploy: {}\n",
            p.post_deploy_failures.join(", ")
        ));
    }
    out
}

/// One line per change, for `bridgewatch watch`.
pub fn one_line(s: &Snapshot) -> String {
    let parts: Vec<String> = s
        .watches
        .iter()
        .map(|w| {
            let head = w
                .rows
                .iter()
                .max_by_key(|r| r.id)
                .map(|r| match &r.eta {
                    Some(eta) => format!("{} {} ({})", r.sha7, r.state, eta_text(eta)),
                    None => format!("{} {}", r.sha7, r.state),
                })
                .unwrap_or_else(|| "-".to_string());
            format!("{}={head}", w.id)
        })
        .collect();
    format!(
        "{} {} | {}",
        s.last_poll.format("%H:%M:%S"),
        s.icon_state,
        parts.join("  ")
    )
}

/// "usually ~11m, 6m 30s in": the popover's ETA line, in ASCII.
///
/// The same two halves as the popover's, which puts a middle dot between them;
/// a comma here, because nothing else this prints leaves ASCII.
pub fn eta_text(eta: &Eta) -> String {
    format!(
        "usually {}, {} in",
        approx_duration(eta.typical_secs),
        duration(eta.elapsed_secs)
    )
}

/// "42s", "6m 30s", "1h 02m": the popover's `formatDuration`.
fn duration(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    let minutes = secs / 60;
    if minutes < 60 {
        return format!("{minutes}m {:02}s", secs % 60);
    }
    format!("{}h {:02}m", minutes / 60, minutes % 60)
}

/// "~45s", "~11m", "~1h 05m": a typical duration, to the nearest minute once
/// it is one. The popover's `approxDuration`.
fn approx_duration(secs: u64) -> String {
    if secs < 60 {
        return format!("~{secs}s");
    }
    // Rounded before the unit is chosen, so 59m 40s reads "~1h 00m" and never
    // "~60m".
    let minutes = (secs + 30) / 60;
    if minutes < 60 {
        return format!("~{minutes}m");
    }
    format!("~{}h {:02}m", minutes / 60, minutes % 60)
}

/// A value that changes only when something worth printing changed.
///
/// ⚠ The ETA is deliberately not in it: its elapsed half moves on every tick,
/// and `watch` prints a line per change, so including it would print a line
/// per tick for as long as a deploy runs.
pub fn fingerprint(s: &Snapshot) -> String {
    let mut parts = vec![s.icon_state.as_str().to_string()];
    for w in &s.watches {
        for r in &w.rows {
            parts.push(format!(
                "{}:{}:{}:{}:{}",
                w.id,
                r.id,
                r.state,
                r.deploy,
                r.failures.join(",")
            ));
        }
        if let Some(e) = &w.error {
            parts.push(format!("{}:err:{e}", w.id));
        }
    }
    parts.join("|")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_with_eta(eta: Option<Eta>) -> Snapshot {
        let mut snapshot: Snapshot = serde_json::from_value(serde_json::json!({
            "icon_state": "running",
            "watches": [{
                "id": "main-push", "role": "primary", "icon_state": "running",
                "rows": [{
                    "id": 1, "iid": null, "sha": "0123456789abcdef", "sha7": "0123456",
                    "ref": "main", "source": "push", "status": "running", "web_url": null,
                    "state": "running", "deploy": "in_progress", "deploy_marker": null,
                    "deploy_failures": [], "failures": [], "warnings": [], "gates": [],
                    "post_deploy_failures": [], "sibling_failures": [], "bridges": [],
                    "parent_jobs": [], "updated_at": null,
                    "created_at": "2026-01-01T00:00:00Z", "live": true
                }],
                "error": null
            }],
            "errors": [],
            "last_poll": "2026-01-01T00:06:30Z",
            "request_log": []
        }))
        .expect("a hand-written snapshot deserialises");
        snapshot.watches[0].rows[0].eta = eta;
        snapshot
    }

    #[test]
    fn a_running_row_with_an_estimate_carries_it_on_the_one_line_output_and_in_the_table() {
        let snapshot = snapshot_with_eta(Some(Eta {
            typical_secs: 660,
            elapsed_secs: 390,
        }));
        assert_eq!(
            one_line(&snapshot),
            "00:06:30 running | main-push=0123456 running (usually ~11m, 6m 30s in)"
        );
        assert!(
            super::snapshot(&snapshot).contains("      eta: usually ~11m, 6m 30s in\n"),
            "{}",
            super::snapshot(&snapshot)
        );
    }

    #[test]
    fn a_row_without_an_estimate_prints_exactly_what_it_printed_before_the_estimate_existed() {
        let snapshot = snapshot_with_eta(None);
        assert_eq!(
            one_line(&snapshot),
            "00:06:30 running | main-push=0123456 running"
        );
        assert!(!super::snapshot(&snapshot).contains("eta"));
    }

    #[test]
    fn the_estimate_is_not_part_of_the_fingerprint_so_watch_does_not_print_every_tick() {
        let early = snapshot_with_eta(Some(Eta {
            typical_secs: 660,
            elapsed_secs: 10,
        }));
        let later = snapshot_with_eta(Some(Eta {
            typical_secs: 660,
            elapsed_secs: 400,
        }));
        assert_eq!(fingerprint(&early), fingerprint(&later));
        assert_eq!(fingerprint(&early), fingerprint(&snapshot_with_eta(None)));
    }

    #[test]
    fn durations_read_the_way_the_popover_reads_them() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(59), "59s");
        assert_eq!(duration(65), "1m 05s");
        assert_eq!(duration(3_725), "1h 02m");
        assert_eq!(approx_duration(45), "~45s");
        assert_eq!(approx_duration(89), "~1m");
        assert_eq!(approx_duration(90), "~2m");
        assert_eq!(approx_duration(3_580), "~1h 00m");
        assert_eq!(approx_duration(3_900), "~1h 05m");
    }
}

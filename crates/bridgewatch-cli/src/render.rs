//! Human-readable rendering of a [`Snapshot`].

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
                .map(|r| format!("{} {}", r.sha7, r.state))
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

/// A value that changes only when something worth printing changed.
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

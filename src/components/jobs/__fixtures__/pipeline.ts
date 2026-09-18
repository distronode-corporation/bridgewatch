/**
 * A small, hand-built pipeline for the job-view tests: a parent with jobs in
 * three stages delivered OUT of stage order (GitLab lists newest id first),
 * a running child, a failed child and a passing one.
 *
 * The real recording (`src/lib/__fixtures__/ca41ab28.json`) is used as well
 * where real stage names and ids matter; this one exists because that
 * recording predates durations and has no running job.
 */

import type { BridgeView, JobView, PipelineView } from "../../../lib/types";

export const T0 = Date.parse("2026-09-18T10:00:00Z");

export function job(partial: Partial<JobView> & Pick<JobView, "id" | "name">): JobView {
  return {
    class: "passed",
    status: "success",
    allow_failure: false,
    stage: "test",
    web_url: `https://gitlab.example/p/-/jobs/${partial.id}`,
    started_at: null,
    finished_at: null,
    duration: null,
    ...partial,
  };
}

export function bridge(partial: Partial<BridgeView> & Pick<BridgeView, "name">): BridgeView {
  return {
    status: "success",
    verdict: "passed",
    verdict_jobs: [],
    child_id: 900,
    child_url: `https://gitlab.example/p/-/pipelines/900`,
    child_project_id: 1,
    dived: true,
    jobs: [],
    web_url: `https://gitlab.example/p/-/jobs/1`,
    ...partial,
  };
}

/** Parent jobs, newest id first, the way GitLab's jobs endpoint returns them. */
export const PARENT_JOBS: JobView[] = [
  job({ id: 106, name: "deploy:prod", stage: "deploy", class: "not_built", status: "created" }),
  job({ id: 105, name: "e2e", stage: "test", class: "warning_failure", status: "failed", allow_failure: true, duration: 95 }),
  job({ id: 104, name: "unit", stage: "test", class: "passed", duration: 42.4 }),
  job({ id: 102, name: "build:web", stage: "build", class: "passed", duration: 187 }),
  job({ id: 101, name: "lint", stage: "build", class: "blocking_failure", status: "failed", duration: 12 }),
];

export const RUNNING_JOB = job({
  id: 203,
  name: "compile",
  stage: "build",
  class: "live",
  status: "running",
  // Started 65 s before T0, so at T0 it reads "1m 05s".
  started_at: new Date(T0 - 65_000).toISOString(),
  duration: 60,
});

export function pipeline(overrides: Partial<PipelineView> = {}): PipelineView {
  return {
    id: 5000,
    iid: 42,
    sha: "abcdef0123456789",
    sha7: "abcdef0",
    ref: "main",
    source: "push",
    status: "running",
    web_url: "https://gitlab.example/p/-/pipelines/5000",
    state: "running",
    deploy: "absent",
    deploy_marker: null,
    deploy_failures: [],
    failures: ["lint"],
    warnings: ["e2e"],
    gates: [],
    post_deploy_failures: [],
    sibling_failures: [],
    parent_jobs: PARENT_JOBS,
    bridges: [
      bridge({
        name: "trigger:app",
        status: "running",
        verdict: "running",
        jobs: [
          job({ id: 204, name: "package", stage: "publish", class: "not_built", status: "created" }),
          RUNNING_JOB,
          job({ id: 201, name: "setup", stage: "prepare", class: "passed", duration: 8 }),
        ],
      }),
      bridge({
        name: "trigger:docs",
        verdict: "passed",
        child_id: 901,
        jobs: [job({ id: 301, name: "docs:build", stage: "build", class: "passed", duration: 30 })],
      }),
      bridge({
        name: "trigger:android",
        verdict: "failed",
        status: "failed",
        child_id: 902,
        jobs: [
          job({ id: 402, name: "android:test", stage: "test", class: "blocking_failure", status: "failed", duration: 300 }),
          job({ id: 401, name: "android:build", stage: "build", class: "passed", duration: 600 }),
        ],
      }),
    ],
    updated_at: new Date(T0).toISOString(),
    created_at: new Date(T0 - 600_000).toISOString(),
    live: true,
    ...overrides,
  };
}

#!/usr/bin/env bash
# Record one parent pipeline into a test fixture directory, using `glab api`.
#
#   scripts/record-fixture.sh <pipeline-id> <name> [project] [--list]
#
# Writes crates/bridgewatch-core/tests/fixtures/<name>/ containing:
#
#   fixture.json        { "primary": <id>, "project": <id>, "source": "<url>" }
#   list.json           the pipeline list page (with --list) or a one-row array
#   pipeline-<id>.json  GET /pipelines/<id>
#   jobs.json           GET /pipelines/<id>/jobs          (all pages)
#   bridges.json        GET /pipelines/<id>/bridges       (all pages)
#   child-<id>.json     GET /pipelines/<child>/jobs, per bridge with a downstream
#
# Nothing else is recorded: those are the only endpoints bridgewatch calls.
#
# `bridgewatch fixture record` does the same walk through the client, so it
# works without glab. This script exists because glab is already authenticated
# on a developer's machine and needs no config file.
#
# ⛔ EVERY RESPONSE IS PUT THROUGH AN ALLOW-LIST BEFORE IT IS WRITTEN. Fixtures
# are committed to a public repository and are recorded from a private one; a
# raw GitLab job payload carries the committer's real name and email address,
# the full commit message and title, the pushing user's profile (job title,
# location, employer, social handles) and the runner that ran it, including a
# self-hosted runner's IP address, system id, tags and free-text description.
#
# ⚠️ THE FILTERS BELOW MUST AGREE WITH `PIPELINE_KEYS` AND `JOB_KEYS` IN
# `crates/bridgewatch-core/src/client/fixture.rs`. They are two implementations
# of one rule, and nothing makes them agree automatically. What catches a
# divergence is the `fixtures_contain_no_personal_data` test, which asserts the
# Rust allow-list against every committed fixture — so a file this script writes
# with an extra key fails the suite rather than shipping. Change one, change the
# other, and run `cargo test -p bridgewatch-core --test verdict`.
set -euo pipefail

PIPELINE_ID="${1:?usage: record-fixture.sh <pipeline-id> <name> [project] [--list]}"
NAME="${2:?usage: record-fixture.sh <pipeline-id> <name> [project] [--list]}"
PROJECT="${3:-82468124}"
WANT_LIST="${4:-}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$REPO_ROOT/crates/bridgewatch-core/tests/fixtures/$NAME"

command -v glab >/dev/null || { echo "glab is not installed" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is not installed" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq is not installed (needed to scrub recordings)" >&2; exit 1; }

# The allow-lists. Keep in step with crates/bridgewatch-core/src/client/fixture.rs.
JQ_SCRUB='
def keep($ks): if type == "object" then with_entries(select(.key as $k | $ks | index($k))) else . end;

# PIPELINE_KEYS
def pipeline_obj: keep([
  "id","iid","project_id","sha","ref","status","source",
  "created_at","updated_at","started_at","finished_at","web_url","name"
]);

# JOB_KEYS. Bridges are jobs that also carry downstream_pipeline, so one list
# covers both; the two nested objects that survive are both pipelines.
def job_obj: keep([
  "id","name","stage","status","allow_failure",
  "created_at","started_at","finished_at","duration","web_url","failure_reason",
  "pipeline","downstream_pipeline"
])
| (if type == "object" and has("pipeline") then .pipeline |= pipeline_obj else . end)
| (if type == "object" and has("downstream_pipeline") then .downstream_pipeline |= pipeline_obj else . end);

def scrub_each(f): if type == "array" then map(f) else f end;
'

# scrub <pipeline_obj|job_obj> ; reads stdin, writes stdout
#
# ⚠️ `--sort-keys` is not cosmetic. serde_json emits object keys sorted, so
# without it this script and `client::fixture::scrub` would produce the same
# DATA in a different ORDER, and a fixture re-scrubbed by
# `bridgewatch fixture scrub` would show a whole-file diff for no reason.
# Sorting on both sides makes the two implementations byte-identical, which is
# how their agreement is actually checked.
scrub() {
  jq --sort-keys --indent 2 "${JQ_SCRUB} scrub_each(${1})"
}

mkdir -p "$OUT"

# Follow x-next-page until it is empty, concatenating the JSON arrays.
paginate() {
  local path="$1" page=1 tmp all
  all="$(mktemp)"
  echo '[]' > "$all"
  while :; do
    tmp="$(mktemp)"
    glab api "${path}?per_page=100&page=${page}" > "$tmp"
    python3 - "$all" "$tmp" <<'PY'
import json, sys
a = json.load(open(sys.argv[1]))
b = json.load(open(sys.argv[2]))
json.dump(a + (b if isinstance(b, list) else [b]), open(sys.argv[1], "w"))
PY
    # A short page is the last page. glab does not surface response headers on
    # a plain `api` call, so length is the discriminator available here.
    if [ "$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))))' "$tmp")" -lt 100 ]; then
      rm -f "$tmp"
      break
    fi
    rm -f "$tmp"
    page=$((page + 1))
  done
  python3 -c 'import json,sys; json.dump(json.load(open(sys.argv[1])), sys.stdout, indent=2); print()' "$all"
  rm -f "$all"
}

echo "recording pipeline $PIPELINE_ID from project $PROJECT into $OUT"

glab api "projects/$PROJECT/pipelines/$PIPELINE_ID" \
  | scrub pipeline_obj > "$OUT/pipeline-$PIPELINE_ID.json"

REF="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["ref"])' "$OUT/pipeline-$PIPELINE_ID.json")"

if [ "$WANT_LIST" = "--list" ]; then
  glab api "projects/$PROJECT/pipelines?ref=$REF&order_by=id&sort=desc&per_page=10" \
    | scrub pipeline_obj > "$OUT/list.json"
else
  # Already scrubbed above; wrap the one row in an array.
  jq --indent 2 '[.]' "$OUT/pipeline-$PIPELINE_ID.json" > "$OUT/list.json"
fi

paginate "projects/$PROJECT/pipelines/$PIPELINE_ID/jobs"    | scrub job_obj > "$OUT/jobs.json"
paginate "projects/$PROJECT/pipelines/$PIPELINE_ID/bridges" | scrub job_obj > "$OUT/bridges.json"

python3 - "$OUT/bridges.json" <<'PY' | while read -r child_id child_project; do
import json, sys
for b in json.load(open(sys.argv[1])):
    d = b.get("downstream_pipeline")
    if d:
        print(d["id"], d.get("project_id", ""))
PY
  echo "  child $child_id (project ${child_project:-$PROJECT})"
  paginate "projects/${child_project:-$PROJECT}/pipelines/$child_id/jobs" \
    | scrub job_obj > "$OUT/child-$child_id.json"
done

WEB_URL="$(jq -r '.web_url' "$OUT/pipeline-$PIPELINE_ID.json")"
python3 - "$OUT/fixture.json" "$PIPELINE_ID" "$PROJECT" "$WEB_URL" <<'PY'
import json, sys
json.dump(
    {"primary": int(sys.argv[2]), "project": int(sys.argv[3]), "source": sys.argv[4]},
    open(sys.argv[1], "w"),
    indent=2,
)
open(sys.argv[1], "a").write("\n")
PY

echo "wrote:"
ls -1 "$OUT" | sed 's/^/  /'
echo
echo "Now write $OUT/expected.json. See tests/verdict.rs for the shape."

echo "Recordings were scrubbed through the allow-list on the way in."

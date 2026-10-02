#!/usr/bin/env bash
# Record one parent pipeline into a test fixture directory, using `glab api`.
#
#   scripts/record-fixture.sh <pipeline-id> <name> <project> [--list]
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
# ⛔ EVERY RESPONSE IS PUT THROUGH AN ALLOW-LIST BEFORE IT REACHES THE FIXTURE.
# Fixtures are committed to a public repository and are recorded from a
# private one; a raw GitLab job payload carries the committer's real name and
# email address, the full commit message and title, the pushing user's profile
# (job title, location, employer, social handles) and the runner that ran it,
# including a self-hosted runner's IP address, system id, tags and free-text
# description.
#
# The raw responses are written to a private temp directory (removed on exit,
# whatever happens), scrubbed there by `bridgewatch fixture scrub`, which
# applies `PIPELINE_KEYS` and `JOB_KEYS` from
# `crates/bridgewatch-core/src/client/fixture.rs`, and only then moved into
# the fixture directory. There is one allow-list, the Rust one; this script
# carries no copy of it. The CLI is built with cargo on first use.
set -euo pipefail

USAGE="usage: record-fixture.sh <pipeline-id> <name> <project> [--list]"
PIPELINE_ID="${1:?$USAGE}"
NAME="${2:?$USAGE}"
PROJECT="${3:?$USAGE}"
WANT_LIST="${4:-}"

# Refused before any request: the id is a path segment and a fixture.json
# number, and anything else would only fail at the end.
case "$PIPELINE_ID" in
  ''|*[!0-9]*) echo "pipeline id must be a number, got: $PIPELINE_ID" >&2; exit 64 ;;
esac

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$REPO_ROOT/crates/bridgewatch-core/tests/fixtures/$NAME"

command -v glab >/dev/null || { echo "glab is not installed" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is not installed" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq is not installed" >&2; exit 1; }
command -v cargo >/dev/null || { echo "cargo is not installed (needed to scrub recordings)" >&2; exit 1; }

# Unscrubbed responses live only here, and only until the script exits.
RAW="$(mktemp -d)"
trap 'rm -rf "$RAW"' EXIT

# Follow x-next-page until it is empty, concatenating the JSON arrays.
paginate() {
  local path="$1" page=1 tmp all
  all="$(mktemp "$RAW/page.XXXXXX")"
  echo '[]' > "$all"
  while :; do
    tmp="$(mktemp "$RAW/page.XXXXXX")"
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

glab api "projects/$PROJECT/pipelines/$PIPELINE_ID" > "$RAW/pipeline-$PIPELINE_ID.json"

REF="$(jq -r '.ref' "$RAW/pipeline-$PIPELINE_ID.json")"
# ⛔ The numeric id comes from the pipeline GitLab returned, not from <project>,
# which may be a URL-encoded path (group%2Fproject). Reading the argument made
# a path-form recording fetch and scrub everything, then die writing
# fixture.json, which FixtureTransport::load needs.
PROJECT_ID="$(jq -r '.project_id' "$RAW/pipeline-$PIPELINE_ID.json")"
case "$PROJECT_ID" in
  ''|*[!0-9]*) echo "pipeline $PIPELINE_ID has no numeric project_id: $PROJECT_ID" >&2; exit 1 ;;
esac

if [ "$WANT_LIST" = "--list" ]; then
  glab api "projects/$PROJECT/pipelines?ref=$REF&order_by=id&sort=desc&per_page=10" > "$RAW/list.json"
else
  # The one row, as an array. Scrubbed with everything else below.
  jq '[.]' "$RAW/pipeline-$PIPELINE_ID.json" > "$RAW/list.json"
fi

paginate "projects/$PROJECT/pipelines/$PIPELINE_ID/jobs"    > "$RAW/jobs.json"
paginate "projects/$PROJECT/pipelines/$PIPELINE_ID/bridges" > "$RAW/bridges.json"

jq -r '.[] | .downstream_pipeline // empty | "\(.id) \(.project_id // "")"' "$RAW/bridges.json" \
  | while read -r child_id child_project; do
  echo "  child $child_id (project ${child_project:-$PROJECT})"
  paginate "projects/${child_project:-$PROJECT}/pipelines/$child_id/jobs" > "$RAW/child-$child_id.json"
done

# The paginator's scratch files are not recordings; nothing else is in $RAW.
rm -f "$RAW"/page.*

# The one allow-list, applied in place. `fixture scrub` refuses a directory
# with no recorded files in it, and exits non-zero on any error, so a failed
# scrub stops here with nothing moved.
cargo run -q --locked --manifest-path "$REPO_ROOT/Cargo.toml" -p bridgewatch-cli -- \
  fixture scrub "$RAW" > /dev/null

WEB_URL="$(jq -r '.web_url' "$RAW/pipeline-$PIPELINE_ID.json")"
jq -n --argjson primary "$PIPELINE_ID" --argjson project "$PROJECT_ID" --arg source "$WEB_URL" \
  '{primary: $primary, project: $project, source: $source}' > "$RAW/fixture.json"

mkdir -p "$OUT"
mv "$RAW"/*.json "$OUT"/

echo "wrote:"
ls -1 "$OUT" | sed 's/^/  /'
echo
echo "Now write $OUT/expected.json. See tests/verdict.rs for the shape."
echo "Recordings were scrubbed through the allow-list before they were moved in."

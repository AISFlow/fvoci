#!/usr/bin/env bash
# The verdict of `ci-done`: passes only when every job it waits for passed, or was
# skipped by the plan (plan.sh). A job that failed, was cancelled, or was skipped
# while the plan did not say so fails it, and so does a plan that did not pass, gave
# no list, or named a job that is not waited for.
#
#   NEEDS=<toJSON(needs)> verdict.sh
set -euo pipefail

# The plan's list of the jobs it skips, or null when it gave none. The `$` are jq's.
# shellcheck disable=SC2016
PLAN='. as $needs | ($needs.plan.outputs.skip // "" | try fromjson catch null) as $skip'

jq --raw-output "$PLAN"'
  | to_entries[]
  | "\(.value.result)\t\(.key)"
    + (if .value.result == "skipped" and ($skip | type) == "array" and (.key | IN($skip[]))
       then "\t(skipped by the plan)" else "" end)' <<<"$NEEDS"

jq --exit-status "$PLAN"'
  | $needs.plan.result == "success"
    and ($skip | type) == "array"
    and all($skip[]; . as $job | type == "string" and $job != "plan" and ($needs | has($job)))
    and all($needs | to_entries[];
      .value.result == "success"
      or (.value.result == "skipped" and (.key | IN($skip[]))))' <<<"$NEEDS" >/dev/null

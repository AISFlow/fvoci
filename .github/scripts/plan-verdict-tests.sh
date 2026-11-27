#!/usr/bin/env bash
# The table tests of plan.sh and verdict.sh, on repositories made for each case and
# a stand-in for `gh`. A push to main cannot be run before the workflow is there:
# this is where its rules are tried.
#
#   plan-verdict-tests.sh
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
failed=0

# The stand-in for `gh api --jq <filter> <path>`: the filter applied to FAKE_JSON when
# the path asks for the artifact `tested-<FAKE_TREE>`, to an empty list for any other;
# FAKE_ARTIFACTS_RAW, when set, replaces only the artifacts answer so malformed run ids
# are still checked against /actions/runs/*. For a run, FAKE_RUNS_JSON maps run id (or
# `*` as a wildcard for malformed-id tests) to a run JSON object, or fails as for a run
# GitHub does not have. FAKE_JSON=fail fails every call.
mkdir "$WORK/bin"
cat >"$WORK/bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ $1 == api && $2 == --jq ]] || exit 2
[[ ${FAKE_JSON:-} == fail ]] && exit 1
case $4 in
  *"/actions/artifacts?name=tested-${FAKE_TREE:-}&"*)
    if [[ -n ${FAKE_ARTIFACTS_RAW+x} ]]; then
      printf '%s\n' "$FAKE_ARTIFACTS_RAW"
      exit 0
    fi
    jq --raw-output "$3" <<<"$FAKE_JSON"
    ;;
  */actions/artifacts\?*)
    jq --raw-output "$3" <<<'{"artifacts": []}'
    ;;
  */actions/runs/*)
    id=${4#*/actions/runs/}
    runs_json='{}'
    if [[ -n ${FAKE_RUNS_JSON:-} ]]; then runs_json=$FAKE_RUNS_JSON; fi
    run=$(jq --compact-output --arg id "$id" '.[$id] // .["*"] // empty' <<<"$runs_json")
    [[ -n $run ]] || exit 1
    jq --raw-output "$3" <<<"$run"
    ;;
  *) exit 2 ;;
esac
EOF
chmod +x "$WORK/bin/gh"

RUST='"rust-postgres","rust-sqlite"'
WEB='"web-checks","web-e2e","web-e2e-apple"'
BUILD='"web-build","web-e2e","web-e2e-apple","web-e2e-live"'
# Artifacts as GitHub lists them: of a run (11) of this repository's branch, of a fork's
# run (12), expired (13), of a run (14) whose later attempt failed, missing repo ids (16),
# missing expired flag (17).
OURS='{"expired": false, "workflow_run": {"id": 11, "repository_id": 7, "head_repository_id": 7}}'
FORK='{"expired": false, "workflow_run": {"id": 12, "repository_id": 7, "head_repository_id": 9}}'
OLD='{"expired": true, "workflow_run": {"id": 13, "repository_id": 7, "head_repository_id": 7}}'
RED='{"expired": false, "workflow_run": {"id": 14, "repository_id": 7, "head_repository_id": 7}}'
MISSING_REPO='{"expired": false, "workflow_run": {"id": 16}}'
MISSING_EXPIRED='{"workflow_run": {"id": 17, "repository_id": 7, "head_repository_id": 7}}'

# One workflow run object as GitHub returns it; defaults are a completed, passing ci.yml
# pull_request run for HEAD^2.
run_json() {
  local head_sha=${1:-$(git rev-parse 'HEAD^2')}
  local event=${2:-pull_request}
  local path=${3:-.github/workflows/ci.yml}
  local conclusion=${4:-success}
  local status=${5:-completed}
  jq --null-input --compact-output \
    --arg event "$event" --arg path "$path" \
    --arg conclusion "$conclusion" --arg status "$status" --arg head_sha "$head_sha" \
    '{event: $event, path: $path, status: $status, conclusion: $conclusion, head_sha: $head_sha}'
}

# A new repository with one commit on main, which is also origin/main.
repo() {
  local dir="$WORK/repo-$1"
  git init --quiet --initial-branch main "$dir"
  cd "$dir"
  git config user.name test
  git config user.email test@example.invalid
  put README 'one'
  put apps/web/src/app.ts 'one'
  put crates/model/src/lib.rs 'one'
  git add --all && git commit --quiet --message base
  git update-ref refs/remotes/origin/main HEAD
}

# Writes $2 to the file $1.
put() {
  mkdir -p "$(dirname "$1")"
  printf '%s\n' "$2" >"$1"
}

# Changes the files given as path=text pairs (or removes them, given as -path) and
# commits.
change() {
  local pair
  for pair in "$@"; do
    case $pair in
      -*) git rm --quiet "${pair#-}" ;;
      *) put "${pair%%=*}" "${pair#*=}" ;;
    esac
  done
  git add --all
  git commit --quiet --allow-empty --message change
}

# On a branch from here, changes the files and commits; then merges it here, as
# GitHub's test merge or a merge commit does.
merge_change() {
  local here
  here=$(git branch --show-current)
  git checkout --quiet -b "change-$RANDOM"
  change "$@"
  git checkout --quiet "$here"
  git merge --quiet --no-ff --no-edit "@{-1}"
}

# Runs plan.sh here and compares its list with the expected one.
expect_plan() {
  local name=$1 want="[$2]" got
  shift 2
  got=$(env PATH="$WORK/bin:$PATH" GITHUB_OUTPUT=/dev/stdout REPOSITORY=fvoci/test \
    DEFAULT_BRANCH=main "$@" bash "$HERE/plan.sh" | sed -n 's/^skip=//p')
  if [[ $got == "$want" ]]; then
    echo "ok    plan: $name"
  else
    echo "FAIL  plan: $name: skip $got, not $want"
    failed=1
  fi
}

repo web
merge_change apps/web/src/app.ts=two 'apps/web/src/a name
with a new line.ts=x'
expect_plan "a pull request of the web app's files skips the Rust tests" "$RUST" \
  EVENT=pull_request

repo rust
merge_change crates/model/src/lib.rs=two xtask/src/main.rs=x
expect_plan "a pull request of .rs files skips the web app's tests" "$WEB" EVENT=pull_request

repo rust-data
merge_change crates/model/src/lib.rs=two crates/collab/tests/data/made-here.json=x
expect_plan "data under crates/ (the web app's tests read some) skips nothing" "" \
  EVENT=pull_request

repo root-file
merge_change apps/web/src/app.ts=two deny.toml=x
expect_plan "a file of the root (the web app reads deny.toml) skips nothing" "" \
  EVENT=pull_request

repo cargo
merge_change Cargo.lock=x
expect_plan "Cargo.lock skips nothing" "" EVENT=pull_request

repo both
merge_change apps/web/src/app.ts=two crates/model/src/lib.rs=two
expect_plan "a pull request of both skips nothing" "" EVENT=pull_request

repo workflow
merge_change .github/workflows/ci.yml=x
expect_plan "the workflow skips nothing" "" EVENT=pull_request

repo moved
git checkout --quiet -b moved
git mv apps/web/src/app.ts crates/model/src/app.rs
git commit --quiet --message moved
git checkout --quiet main
git merge --quiet --no-ff --no-edit moved
expect_plan "a file moved from the web app to Rust counts on both sides" "" EVENT=pull_request

repo removed
merge_change -crates/model/src/lib.rs
expect_plan "a removed .rs file counts as Rust" "$WEB" EVENT=pull_request

repo empty
merge_change
expect_plan "a pull request that changes nothing skips nothing" "" EVENT=pull_request

repo unknown-default
merge_change apps/web/src/app.ts=two
expect_plan "a default branch git does not have skips nothing" "" EVENT=pull_request \
  DEFAULT_BRANCH=missing

repo moved-main
git checkout --quiet -b late
change apps/web/src/app.ts=two
git checkout --quiet main
change crates/model/src/lib.rs=moved
git update-ref refs/remotes/origin/main HEAD
git merge --quiet --no-ff --no-edit late
expect_plan "main's own commits since the branch parted are not the pull request's" \
  "$RUST" EVENT=pull_request

# A stack: the lower layer changes the web app, the upper Rust only. The upper's test
# merge has the lower's as its first parent.
repo stack
git checkout --quiet -b lower
change apps/web/src/app.ts=two
git checkout --quiet -b upper
change crates/model/src/lib.rs=two
git checkout --quiet lower
git merge --quiet --no-ff --no-edit upper
expect_plan "the upper layer of a stack is planned on the layers below it too" "" \
  EVENT=pull_request

repo stack-rust
git checkout --quiet -b lower
change crates/model/src/lib.rs=two
git checkout --quiet -b upper
change xtask/src/main.rs=x
git checkout --quiet lower
git merge --quiet --no-ff --no-edit upper
expect_plan "a stack of Rust only skips the web app's tests" "$WEB" EVENT=pull_request

repo dispatch
merge_change apps/web/src/app.ts=two
expect_plan "an unexpected event skips nothing" "" EVENT=workflow_dispatch
expect_plan "a run without an event skips nothing" "" EVENT=

# Pushes to main: BEFORE is main before the push.
repo pushed
before=$(git rev-parse HEAD)
merge_change apps/web/src/app.ts=two
tree=$(git rev-parse 'HEAD^{tree}')
ok_run=$(run_json)
ok_runs="{\"11\": $ok_run}"
all_ok="{\"11\": $ok_run, \"12\": $ok_run, \"13\": $ok_run, \"16\": $ok_run, \"17\": $ok_run}"
expect_plan "a merge whose tree passed skips the jobs of the build" "$RUST,$BUILD" \
  EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$OURS]}" \
  FAKE_RUNS_JSON="$ok_runs"
expect_plan "a merge whose tree passed twice skips the jobs of the build" "$RUST,$BUILD" \
  EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$OLD, $OURS, $OURS]}" \
  FAKE_RUNS_JSON="$ok_runs"
expect_plan "a merge whose tree did not pass skips by its files only" "$RUST" \
  EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON='{"artifacts": []}' \
  FAKE_RUNS_JSON="$ok_runs"
expect_plan "a merge whose tree passed only in a fork's run skips by its files only" "$RUST" \
  EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$FORK]}" \
  FAKE_RUNS_JSON="$all_ok"
expect_plan "a merge whose tree's record expired skips by its files only" "$RUST" \
  EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$OLD]}" \
  FAKE_RUNS_JSON="$all_ok"
expect_plan "a merge whose tree's record lacks repository ids skips by its files only" "$RUST" \
  EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$MISSING_REPO]}" \
  FAKE_RUNS_JSON="$all_ok"
expect_plan "a merge whose tree's record lacks expired flag skips by its files only" "$RUST" \
  EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$MISSING_EXPIRED]}" \
  FAKE_RUNS_JSON="$all_ok"
expect_plan "a merge whose tree's run was a push event skips by its files only" "$RUST" \
  EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$OURS]}" \
  FAKE_RUNS_JSON="{\"11\": $(run_json '' push)}"
expect_plan "a merge whose tree's run was a dispatch event skips by its files only" "$RUST" \
  EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$OURS]}" \
  FAKE_RUNS_JSON="{\"11\": $(run_json '' workflow_dispatch)}"
expect_plan "a merge whose tree's run was another workflow skips by its files only" "$RUST" \
  EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$OURS]}" \
  FAKE_RUNS_JSON="{\"11\": $(run_json '' pull_request .github/workflows/other.yml)}"
expect_plan "a merge whose tree's run was for HEAD instead of HEAD^2 skips by its files only" \
  "$RUST" EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$OURS]}" \
  FAKE_RUNS_JSON="{\"11\": $(run_json "$(git rev-parse HEAD)")}"
expect_plan "a merge whose tree's run was for HEAD^1 instead of HEAD^2 skips by its files only" \
  "$RUST" EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$OURS]}" \
  FAKE_RUNS_JSON="{\"11\": $(run_json "$(git rev-parse 'HEAD^1')")}"
expect_plan "a merge whose tree's run failed at a later attempt skips by its files only" \
  "$RUST" EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$RED]}" \
  FAKE_RUNS_JSON="{\"11\": $ok_run, \"14\": $(run_json '' pull_request .github/workflows/ci.yml failure)}"
expect_plan "a merge whose tree's run was cancelled skips by its files only" \
  "$RUST" EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$RED]}" \
  FAKE_RUNS_JSON="{\"11\": $ok_run, \"14\": $(run_json '' pull_request .github/workflows/ci.yml cancelled)}"
expect_plan "a merge whose tree's run is still in progress skips by its files only" \
  "$RUST" EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$OURS]}" \
  FAKE_RUNS_JSON="{\"11\": $(run_json '' pull_request .github/workflows/ci.yml success in_progress)}"
expect_plan "a merge whose tree passed in one of two runs skips the jobs of the build" \
  "$RUST,$BUILD" EVENT=push BEFORE="$before" FAKE_TREE="$tree" \
  FAKE_JSON="{\"artifacts\": [$RED, $OURS]}" \
  FAKE_RUNS_JSON="{\"11\": $ok_run, \"14\": $(run_json '' pull_request .github/workflows/ci.yml failure)}"
expect_plan "a merge whose tree's run GitHub no longer has skips by its files only" "$RUST" \
  EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON="{\"artifacts\": [$OURS]}" \
  FAKE_RUNS_JSON="{\"15\": $ok_run}"
expect_plan "a merge when GitHub does not answer skips by its files only" "$RUST" \
  EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_JSON=fail FAKE_RUNS_JSON="$ok_runs"
for answer in null '1 2' -1 0x1 '../11' ''; do
  expect_plan "a merge when GitHub answers '$answer' skips by its files only" "$RUST" \
    EVENT=push BEFORE="$before" FAKE_TREE="$tree" FAKE_ARTIFACTS_RAW="$answer" \
    FAKE_JSON='{}' FAKE_RUNS_JSON="{\"*\": $ok_run}"
done

# A merge after main moved: HEAD^2's tree differs from HEAD's tree.
repo diverged-merge
git checkout --quiet -b feature
change apps/web/src/app.ts=two
git checkout --quiet main
change README=two
before=$(git rev-parse HEAD)
git merge --quiet --no-ff --no-edit feature
expect_plan "a merge whose branch's tree passed, not its own, skips by its files only" "$RUST" \
  EVENT=push BEFORE="$before" FAKE_TREE="$(git rev-parse 'HEAD^2^{tree}')" \
  FAKE_JSON="{\"artifacts\": [$OURS]}" FAKE_RUNS_JSON="{\"11\": $(run_json)}"

# A merge made with an edit of its own: its tree is not its branch's.
repo edited-merge
before=$(git rev-parse HEAD)
git checkout --quiet -b edited
change apps/web/src/app.ts=two
git checkout --quiet main
git merge --quiet --no-ff --no-commit edited
put apps/web/src/app.ts 'three'
git add --all && git commit --quiet --no-edit
expect_plan "a merge whose tree is not its branch's is not taken for its branch's" "$RUST" \
  EVENT=push BEFORE="$before" FAKE_TREE="$(git rev-parse 'HEAD^2^{tree}')" \
  FAKE_JSON="{\"artifacts\": [$OURS]}" FAKE_RUNS_JSON="{\"11\": $(run_json)}"

repo pushed-both
before=$(git rev-parse HEAD)
merge_change apps/web/src/app.ts=two crates/model/src/lib.rs=two
expect_plan "a merge of both whose tree passed skips the jobs of the build" "$BUILD" \
  EVENT=push BEFORE="$before" FAKE_TREE="$(git rev-parse 'HEAD^{tree}')" \
  FAKE_JSON="{\"artifacts\": [$OURS]}" FAKE_RUNS_JSON="{\"11\": $(run_json)}"

repo two-merges
before=$(git rev-parse HEAD)
merge_change apps/web/src/app.ts=two
merge_change crates/model/src/lib.rs=two
expect_plan "two merges pushed at once are planned on both" "" \
  EVENT=push BEFORE="$before" FAKE_TREE=none FAKE_JSON='{"artifacts": []}'

repo no-merge
before=$(git rev-parse HEAD)
change crates/model/src/lib.rs=two
expect_plan "a commit pushed without a merge skips by its files" "$WEB" \
  EVENT=push BEFORE="$before" FAKE_TREE=none FAKE_JSON='{"artifacts": []}'
expect_plan "a single-parent push fails closed even if its tree has a matching run" "$WEB" \
  EVENT=push BEFORE="$before" FAKE_TREE="$(git rev-parse 'HEAD^{tree}')" \
  FAKE_JSON="{\"artifacts\": [$OURS]}" \
  FAKE_RUNS_JSON="{\"*\": $(run_json "$(git rev-parse HEAD)")}"

repo octopus
git checkout --quiet -b branch-a
change apps/web/src/app.ts=two
git checkout --quiet main
git checkout --quiet -b branch-b
change apps/web/src/other.ts=two
git checkout --quiet main
before=$(git rev-parse HEAD)
git merge --quiet --no-ff --no-edit branch-a branch-b
expect_plan "an octopus merge fails closed even if a matching run exists" "$RUST" \
  EVENT=push BEFORE="$before" FAKE_TREE="$(git rev-parse 'HEAD^{tree}')" \
  FAKE_JSON="{\"artifacts\": [$OURS]}" \
  FAKE_RUNS_JSON="{\"*\": $(run_json "$(git rev-parse 'HEAD^2')")}"

repo new-branch
expect_plan "a push git cannot compare skips nothing" "" \
  EVENT=push BEFORE=0000000000000000000000000000000000000000 FAKE_JSON=fail

# The verdict, on the `needs` of ci-done as GitHub gives it.
expect_verdict() {
  local name=$1 want=$2 needs=$3 got=0
  NEEDS=$needs bash "$HERE/verdict.sh" >/dev/null || got=$?
  if [[ $got == "$want" ]]; then
    echo "ok    verdict: $name"
  else
    echo "FAIL  verdict: $name: exit $got, not $want"
    failed=1
  fi
}

# `needs` with the plan's result and list, and the given results of two jobs.
needs() {
  jq --null-input --compact-output --arg plan "$1" --arg skip "$2" --arg a "$3" --arg b "$4" \
    '{plan: {result: $plan, outputs: (if $skip == "-" then {} else {skip: $skip} end)},
      "rust-sqlite": {result: $a, outputs: {}}, "web-e2e": {result: $b, outputs: {}}}'
}

expect_verdict "all passed" 0 "$(needs success '[]' success success)"
expect_verdict "one failed" 1 "$(needs success '[]' success failure)"
expect_verdict "one was cancelled" 1 "$(needs success '[]' cancelled success)"
expect_verdict "one skipped by the plan" 0 "$(needs success '["web-e2e"]' success skipped)"
expect_verdict "one skipped that the plan ran" 1 "$(needs success '["rust-sqlite"]' success skipped)"
expect_verdict "one skipped by the plan ran anyway" 0 "$(needs success '["web-e2e"]' success success)"
expect_verdict "the plan failed" 1 "$(needs failure - skipped skipped)"
expect_verdict "the plan was cancelled" 1 "$(needs cancelled - skipped skipped)"
expect_verdict "the plan gave no list" 1 "$(needs success - success success)"
expect_verdict "the plan's list is not JSON" 1 "$(needs success 'web-e2e' success skipped)"
expect_verdict "the plan names a job not waited for" 1 "$(needs success '["web-e2e","nothing"]' success skipped)"
expect_verdict "the plan skips itself" 1 "$(needs success '["plan"]' success success)"

# Every job of ci.yml is waited for by ci-done, and the sharded Rust jobs keep the ids
# plan.sh and verdict.sh know them by.
WORKFLOW="$HERE/../workflows/ci.yml"
written=$(sed -n '/^jobs:/,$p' "$WORKFLOW" |
  sed -n 's/^  \([^ #].*\)$/\1/p' | sed 's/:[[:space:]]*\(#.*\)\{0,1\}$//' |
  grep -v '^ci-done$' | sort)
waited=$(sed -n '/^  ci-done:/,/^    runs-on:/p' "$WORKFLOW" |
  sed -n 's/^      - \([^ #].*\)$/\1/p' | sort)
if diff -u <(echo "$written") <(echo "$waited") >/dev/null; then
  echo "ok    workflow: ci-done waits for every job of ci.yml"
else
  echo "FAIL  workflow: ci-done's needs do not match the jobs of ci.yml"
  failed=1
fi

full_needs() {
  jq --null-input --compact-output --arg skip "$1" --arg sqlite "$2" --arg postgres "$3" \
    '{plan: {result: "success", outputs: {skip: $skip}},
      "rust-check": {result: "success", outputs: {}},
      "rust-sqlite": {result: $sqlite, outputs: {}},
      "rust-postgres": {result: $postgres, outputs: {}},
      "web-checks": {result: "success", outputs: {}},
      "web-build": {result: "success", outputs: {}},
      "web-e2e": {result: "success", outputs: {}},
      "web-e2e-apple": {result: "success", outputs: {}},
      "web-e2e-live": {result: "success", outputs: {}}}'
}

expect_verdict "both sharded Rust jobs skipped by a web-only plan" 0 \
  "$(full_needs "[$RUST]" skipped skipped)"
expect_verdict "a sharded Rust job whose matrix failed fails ci-done" 1 \
  "$(full_needs '[]' success failure)"

exit "$failed"

#!/usr/bin/env bash
# Says which jobs of ci.yml this run may skip, and writes them to the job's output
# `skip` as a JSON list of job ids. A job runs unless its id is on the list, so an
# empty or missing output runs everything; `ci-done` passes a skipped job only when
# its id is on the list (verdict.sh).
#
#   EVENT=<github.event_name> DEFAULT_BRANCH=main BEFORE=<github.event.before> plan.sh
#
# Run in a checkout of the run's commit with the history of the default branch (the
# trees are enough). A push also needs REPOSITORY and GH_TOKEN, a token that may read
# the repository's artifacts and runs.
#
# What a run changes:
# - a pull request: what its test merge changes since it parted from the default
#   branch. For a layer of a stack that is every layer up to it, so the top layer's
#   `ci-done` stands for the whole stack;
# - a push to main: what the push changes (`BEFORE..HEAD`);
# - any other event: everything.
# When git cannot say, or nothing changed, nothing is skipped.
#
# The rules are wide: a job is skipped only when no file it could test changed.
# - Every file under apps/web/: the Rust tests are skipped. No cargo test reads the
#   web app; `cargo xtask check` does (names), and it always runs.
# - Every file a `.rs` under crates/ or xtask/src/: the web app's check, unit tests,
#   stories, `e2e` and `e2e:apple` are skipped. Its build and `e2e:live`, which runs
#   the server, are not. The web app reads files of crates/ that are not Rust
#   (crates/collab/tests/data/, crates/server/assets/) and the root's (deny.toml):
#   those, like any other file, run everything.
# - A push whose HEAD is a two-parent merge commit and whose tree is one a pull request's
#   run of .github/workflows/ci.yml on this repository passed for HEAD^2 with all four
#   jobs of the build (ci-done keeps the artifact `tested-<tree>` for that), and that run
#   passed still at its last attempt: those four jobs are skipped. The others, which save
#   the caches, run by the rules above.
set -euo pipefail

skip=()

# Adds the skips of the files changed from $1 to $2.
skip_by_files() {
  local list web_only=true rust_only=true any=false path
  list=$(mktemp)
  if ! git diff --name-only -z --no-renames "$1" "$2" -- >"$list"; then
    rm -f "$list"
    echo "plan: git cannot say what changed from $1 to $2: nothing is skipped"
    return 0
  fi
  while IFS= read -r -d '' path; do
    any=true
    case $path in apps/web/*) ;; *) web_only=false ;; esac
    case $path in crates/*.rs | xtask/src/*.rs) ;; *) rust_only=false ;; esac
  done <"$list"
  echo "plan: $(tr -cd '\0' <"$list" | wc -c) files changed; all the web app's:" \
    "$web_only; all Rust: $rust_only"
  rm -f "$list"
  if ! $any; then return 0; fi
  if $web_only; then skip+=(rust-sqlite rust-postgres); fi
  if $rust_only; then skip+=(web-checks web-e2e web-e2e-apple); fi
}

# Whether a pull request's run of .github/workflows/ci.yml on a branch of this repository
# passed on the tree of HEAD for the merged commit (HEAD^2) with every job of the build,
# and passed still at its last attempt: a run whose first attempt left the record and
# whose later attempt failed does not count. Non-merge pushes (no HEAD^2) and
# multi-parent merges (HEAD^3) fail closed.
tested_tree() {
  local head_sha tree runs run verified
  if ! head_sha=$(git rev-parse --verify --quiet 'HEAD^2') ||
    git rev-parse --verify --quiet 'HEAD^3' >/dev/null; then
    echo "plan: HEAD is not a two-parent merge commit: not reusing a tested tree"
    return 1
  fi
  if ! git diff --quiet 'HEAD^1' 'HEAD' -- .github/; then
    echo "plan: .github/ changed in merge: not reusing a tested tree"
    return 1
  fi
  tree=$(git rev-parse 'HEAD^{tree}')
  if ! runs=$(gh api --jq '[.artifacts[]
      | select(
          .expired == false
          and (.workflow_run.repository_id | type == "number")
          and .workflow_run.head_repository_id == .workflow_run.repository_id
        )
      | .workflow_run.id] | unique[]' "repos/$REPOSITORY/actions/artifacts?name=tested-$tree&per_page=100"); then
    echo "plan: GitHub did not say whether the tree $tree passed"
    return 1
  fi
  while IFS= read -r run; do
    [[ $run =~ ^[1-9][0-9]*$ ]] || continue
    if verified=$(gh api --jq '
        select(
          .event == "pull_request"
          and .path == ".github/workflows/ci.yml"
          and .status == "completed"
          and .conclusion == "success"
        )
        | .head_sha' "repos/$REPOSITORY/actions/runs/$run") &&
      [[ -n $verified && $verified == "$head_sha" ]]; then
      echo "plan: run $run passed on the tree $tree with every job of the build"
      return 0
    fi
  done <<<"$runs"
  echo "plan: no pull request's run passed on the tree $tree with every job of the build"
  return 1
}

case ${EVENT:-} in
  pull_request)
    if base=$(git merge-base HEAD "origin/${DEFAULT_BRANCH:-}"); then
      skip_by_files "$base" HEAD
    else
      echo "plan: no commit in common with origin/${DEFAULT_BRANCH:-}: nothing is skipped"
    fi
    ;;
  push)
    skip_by_files "$BEFORE" HEAD
    if tested_tree; then skip+=(web-build web-e2e web-e2e-apple web-e2e-live); fi
    ;;
  *)
    echo "plan: a run of ${EVENT:-no event} runs everything"
    ;;
esac

json=$(jq --null-input --compact-output '$ARGS.positional | unique' --args "${skip[@]}")
echo "plan: skip $json"
echo "skip=$json" >>"${GITHUB_OUTPUT:-/dev/stdout}"

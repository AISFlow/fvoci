// Selected SQLite library cohort: the exact filters the fast job must execute and
// the per-test result rule. A zero-match Cargo run or an ignored test is not execution.
import { createHash } from "node:crypto";
import {
  type Mapping,
  type VerifyContext,
  PY_NON_WS,
  field,
  indexOfSame,
  rustJobs,
  same,
  steps,
} from "./rust-common.ts";

export const RUST_SELECTED_LIBRARY_STEP = "Selected SQLite library controls (54 exact tests)";
// Order is part of the contract: the slice pins below bind each accepted cohort.
export const RUST_SELECTED_LIBRARY_FILTERS: readonly string[] = [
  "db::stars::selected_star_read_finish_tests::star_read_cleanup_retains_refusal_and_driver_without_returning_rows",
  "db::groups::selected_group_read_finish_tests::group_read_cleanup_retains_refusal_and_driver_without_returning_rows",
  "streams::events::selected_access_read_tests::sqlite_access_role_change_targets_and_wrong_workspace",
  "streams::events::selected_access_read_tests::sqlite_access_counter_retention_rollback_and_inflight_writer",
  "streams::events::selected_access_read_tests::sqlite_access_current_credential_membership_workspace_and_cursor_denials",
  "streams::events::selected_access_read_tests::access_cleanup_uncertainty_withholds_observation_and_retains_driver_cause",
  "http::routes::stars::selected_list_access_http_tests::sqlite_http_stars_nonempty_dtos_order_current_acl_and_pat_kinds",
  "http::routes::stars::selected_list_access_http_tests::sqlite_http_groups_order_member_authority_pat_and_cross_tenant",
  "http::routes::stars::selected_list_access_http_tests::sqlite_http_lists_recheck_current_credential_and_propagate_sql_fault",
  "http::routes::stars::selected_list_access_http_tests::sqlite_http_access_role_change_bystander_and_read_error_end_real_body",
  "http::routes::stars::selected_list_access_http_tests::sqlite_http_access_stream_current_revocation_close_drop_and_guard",
  "db::project_documents::selected_create_finish_tests::project_create_rollback_retains_domain_and_driver_causes",
  "db::project_documents::selected_create_backend_tests::sqlite_project_create_current_authority_and_parent_denials",
  "db::project_documents::selected_create_backend_tests::sqlite_project_create_queued_writer_rechecks_credential_and_permission",
  "db::project_documents::selected_create_backend_tests::sqlite_project_create_fk_publication_and_commit_failures_then_healthy_create",
  "http::routes::project_documents::selected_create_http_tests::sqlite_http_project_create_literal_request_metadata_number_order_and_publication",
  "http::routes::project_documents::selected_create_http_tests::sqlite_http_project_create_input_origin_current_authority_and_pat_scope_denials",
  "http::routes::project_documents::selected_create_http_tests::sqlite_http_project_create_real_fk_refusal_is_500_without_partial_effects",
  "db::projects::selected_project_read_tests::sqlite_project_reads_literal_counts_workflow_and_binary_order",
  "db::projects::selected_project_read_tests::sqlite_project_reads_private_group_guest_and_current_authority",
  "db::projects::selected_project_read_tests::sqlite_project_reads_driver_failure_rolls_back_and_healthy_retry",
  "db::projects::selected_project_read_tests::project_read_cleanup_failure_withholds_rows_and_retains_typed_causes",
  "db::document_tags::selected_tag_pool_tests::sqlite_tag_pool_unicode_search_past_page_limit_and_assignment_counts",
  "db::document_tags::selected_tag_pool_tests::sqlite_tag_pool_current_authority_fault_and_healthy_retry",
  "db::document_tags::selected_tag_pool_tests::tag_pool_cleanup_failure_withholds_rows_and_retains_typed_causes",
  "http::routes::projects::selected_pending_get_http_tests::sqlite_http_pending_gets_literal_project_workflow_tag_and_members",
  "http::routes::projects::selected_pending_get_http_tests::sqlite_http_pending_gets_pat_scopes_tenant_and_document_count_disclosure",
  "http::routes::projects::selected_pending_get_http_tests::sqlite_http_pending_gets_current_denials_fault_and_healthy_retry",
  "db::project_documents::selected_metadata_backend_tests::sqlite_project_metadata_literal_status_and_archived_view",
  "db::project_documents::selected_metadata_backend_tests::sqlite_project_metadata_current_grants_tenant_and_credential_denials",
  "db::project_documents::selected_metadata_backend_tests::sqlite_project_metadata_driver_error_and_healthy_retry",
  "http::routes::project_documents::selected_create_http_tests::sqlite_http_project_metadata_cookie_pat_literal_and_current_denials",
  "http::routes::project_documents::selected_create_http_tests::sqlite_http_project_metadata_driver_error_and_healthy_retry",
  "db::labels::selected_project_read_tests::selected_labels_read_nonempty_order_current_grants_and_credential_refusals",
  "db::milestones::selected_project_read_tests::selected_milestones_read_nonempty_order_current_grants_and_credential_refusals",
  "db::workspace::selected_personal_workspace_tests::sqlite_personal_bootstrap_stable_concurrent_mapping_and_private_slug_collision",
  "db::workspace::selected_personal_workspace_tests::sqlite_personal_bootstrap_current_credentials_mapping_and_queued_writer_denials",
  "db::workspace::selected_personal_workspace_tests::sqlite_personal_bootstrap_event_audit_and_commit_fk_failures_then_healthy_retry",
  "db::workspace::selected_personal_workspace_tests::personal_bootstrap_rollback_cleanup_retains_domain_and_driver_causes",
  "db::quota::selected_seat_admission_tests::sqlite_seat_admission_exact_billable_predicate_limit_existing_and_unlimited",
  "db::quota::selected_seat_admission_tests::sqlite_seat_admission_writer_context_driver_failure_and_healthy_retry",
  "db::quota::selected_seat_admission_tests::quota_context_restore_failure_retains_original_refusal_or_driver",
  "http::routes::workspaces::selected_personal_bootstrap_http_tests::sqlite_http_personal_bootstrap_cookie_origin_session_only_and_stable_replay",
  "http::routes::workspaces::selected_personal_bootstrap_http_tests::sqlite_http_personal_bootstrap_seat_limit_and_publication_failure_then_healthy_retry",
  "db::workspace::selected_member_removal_tests::sqlite_member_removal_literal_effects_current_access_and_healthy_owner",
  "db::workspace::selected_member_removal_tests::sqlite_member_removal_domain_matrix_tenant_and_queued_actor_revocation",
  "db::workspace::selected_member_removal_tests::sqlite_member_removal_event_audit_deferred_fk_rollback_and_healthy_progress",
  "db::workspace::selected_member_removal_tests::sqlite_member_removal_concurrent_owners_private_leads_single_winner",
  "db::workspace::selected_member_removal_tests::member_removal_rollback_cleanup_retains_domain_and_driver_causes",
  "db::projects::selected_member_removal_lead_tests::sqlite_workspace_removal_private_archived_direct_group_and_writer_scope",
  "db::invitations::selected_member_removal_invitation_tests::sqlite_pending_inviter_roles_scope_accepted_and_empty_set",
  "db::collections::selected_member_removal_view_tests::sqlite_shared_view_transfer_scope_version_overflow_rollback_and_healthy_progress",
  "http::routes::workspaces::selected_personal_bootstrap_http_tests::member_removal::sqlite_http_member_delete_cookie_origin_pat_tenant_and_literal_success",
  "http::routes::workspaces::selected_personal_bootstrap_http_tests::member_removal::sqlite_http_member_delete_audit_rollback_private_lead_refusal_and_healthy_progress",
];

// [name, start, end) slices of the table and the sha256 of their "\n"-joined UTF-8 text.
export const RUST_SELECTED_LIBRARY_SLICE_PINS: readonly (readonly [
  string,
  number,
  number,
  string,
])[] = [
  ["original18", 0, 18, "f9221b4d6b32402a3e125643d3fc67dfb600df8ef27b8e76013bb8b46ade7c80"],
  ["get10", 18, 28, "157c4dc2ec55fbece3f7aeb85433c6e6552c8e2e91075f191dea15cab8c37e86"],
  ["metadata5", 28, 33, "8c8191b4b26800b5ed8980bc75e8c3ac6b1f6df55611dc9a3665c16db139ae46"],
  ["aux2", 33, 35, "7e7702addc4c016b2e885adb2447d0324da4c857dfa874fb8bc0e9dddc88a71d"],
  ["original35", 0, 35, "479c869c1734119e7ee091d490dbda12d1df5f59fcd73eea78478649f23b1c87"],
  ["bootstrap9", 35, 44, "1baee892b0bc67c4289d4e6ee57b801963d08793f9107f99f72d0e41bc7a18a0"],
  ["original44", 0, 44, "b1d7fff1c44346a300454eff3b0721e1a2078842308a1847b9e9411e23741cb1"],
  ["member10", 44, 54, "a77024442b3b63a39a70012a251fe5ed4948543b8a10156a09ace6d31d7ff09c"],
  ["all54", 0, 54, "aa10daa88b6f9860b43bfb628edb535f185cd4dacd7f1faad27f7d2178ab7498"],
];

// The maintained step body, byte-exact (the YAML block scalar adds the final newline).
export const RUST_SELECTED_LIBRARY_RUN = `set -euo pipefail
python3 - <<'PYLIB'
import subprocess
from scripts.ci_selection import RUST_SELECTED_LIBRARY_FILTERS, selected_library_result_error

for test_filter in RUST_SELECTED_LIBRARY_FILTERS:
    result = subprocess.run(
        ["cargo", "test", "--locked", "--offline", "--features", "db-tests", "--lib", test_filter, "--", "--exact"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    print(result.stdout, end="", flush=True)
    error = selected_library_result_error(test_filter, result.returncode, result.stdout)
    if error:
        raise SystemExit(error)
PYLIB`;
export const RUST_DEFAULT_LIBRARY_STEP: Mapping = {
  run: "cargo test --locked --offline --lib --bin fvoci-server",
  env: { CARGO_TARGET_DIR: "target/default" },
};

const sha256 = (text: string) => createHash("sha256").update(text, "utf8").digest("hex");

/** True when the filters keep all 54 unique entries and every slice pin. */
export function selectedLibraryRegistryIntact(
  filters: readonly string[] = RUST_SELECTED_LIBRARY_FILTERS,
): boolean {
  return (
    filters.length === 54 &&
    new Set(filters).size === 54 &&
    RUST_SELECTED_LIBRARY_SLICE_PINS.every(
      ([, start, end, pin]) => sha256(filters.slice(start, end).join("\n")) === pin,
    )
  );
}

// Python re.MULTILINE semantics: lines end at "\n" only and "." also spans "\r".
const runningLine = /^running ([0-9]+) tests?$/s;
const testLine = new RegExp(`^test (${PY_NON_WS}+) \\.\\.\\. (${PY_NON_WS}+).*$`, "s");
const summaryLine = /^test result: (.*)$/s;
const passedSummary =
  /^ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out; finished in [0-9]+(?:\.[0-9]+)?s$/;

/** Accept exactly the requested, nonignored library test, never compilation. */
export function selectedLibraryResultError(
  testFilter: string,
  returncode: number,
  output: string,
  filters: readonly string[] = RUST_SELECTED_LIBRARY_FILTERS,
): string | null {
  if (!filters.includes(testFilter)) return "rust: unregistered selected library filter";
  if (returncode !== 0) return "rust: selected library test command failed";
  const lines = output.split("\n");
  const counts = lines.flatMap((line) => runningLine.exec(line)?.[1] ?? []);
  if (counts.length !== 1 || counts[0] !== "1") {
    return "rust: selected library command must run exactly one test";
  }
  const executed = lines.flatMap((line) => {
    const match = testLine.exec(line);
    return match ? [[match[1], match[2]]] : [];
  });
  if (!same(executed, [[testFilter, "ok"]])) {
    return "rust: selected library result must be the exact requested test and ok";
  }
  const summaries = lines.flatMap((line) => summaryLine.exec(line)?.[1] ?? []);
  if (summaries.length !== 1 || !passedSummary.test(summaries[0] ?? "")) {
    return "rust: selected library result must pass one test without failure or ignore";
  }
  return null;
}

/** Bind the maintained step, its environment and the exact 54-filter cohort. */
export function verifySelectedLibraryExecution(
  jobs: Mapping,
  filters: readonly string[] = RUST_SELECTED_LIBRARY_FILTERS,
): string[] {
  const errors: string[] = [];
  if (!selectedLibraryRegistryIntact(filters)) {
    errors.push(
      "rust: selected library registry must retain all54 exact filters (original44 prefix and member10)",
    );
  }
  const fast = jobs.fast;
  const list = steps(fast);
  const matches = list.filter((step) => step.name === RUST_SELECTED_LIBRARY_STEP);
  if (matches.length !== 1) {
    errors.push("rust: selected library step must appear exactly once in fast");
    return errors;
  }
  const expected = {
    name: RUST_SELECTED_LIBRARY_STEP,
    env: { CARGO_TARGET_DIR: "target/db-lib" },
    run: RUST_SELECTED_LIBRARY_RUN + "\n",
  };
  if (!same(matches[0], expected)) {
    errors.push(
      "rust: selected library step must keep exact unconditional command without extra env or flags",
    );
  }
  const plain = list.flatMap((step, index) =>
    same(step, RUST_DEFAULT_LIBRARY_STEP) ? [index] : [],
  );
  if (plain.length !== 1 || (plain[0] ?? 0) >= indexOfSame(list, matches[0])) {
    errors.push(
      "rust: selected library step must preserve preceding default plain library command",
    );
  }
  if (field(fast, "env") !== undefined) {
    errors.push("rust: selected library fast job must not add an environment override");
  }
  return errors;
}

export function verifySelectedLibraryExecutionCtx(ctx: VerifyContext): string[] {
  const jobs = rustJobs(ctx);
  return jobs ? verifySelectedLibraryExecution(jobs) : [];
}

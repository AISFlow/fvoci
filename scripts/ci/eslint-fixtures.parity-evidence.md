# Parity evidence

One local run of each suite after the in-suite parity test was removed. No host name and no absolute paths.

| Command | Exit | Elapsed |
| --- | --- | --- |
| `python3 scripts/test_eslint.py` | 0 | 214s (runner reported 214.633s, 19 tests, OK) |
| `bun test scripts/ci/eslint-fixtures.test.ts` | 0 | 336s (runner reported 336.03s, 42 pass, 0 fail) |

The nineteen `test_*` names pass in both outputs. The TypeScript log also passes M1, the negative controls, and the signal-killed child proof. The runners print different words for a pass (`ok` versus `(pass)`).

```diff
--- python
+++ typescript
@@ -1,24 +1,50 @@
-test_actual_nuxt_autoimport_registration_boundary (__main__.VueToolchain.test_actual_nuxt_autoimport_registration_boundary) ... ok
-test_actual_web_and_editor_declaration_preparation_is_node_free (__main__.VueToolchain.test_actual_web_and_editor_declaration_preparation_is_node_free) ... ok
-test_browser_does_not_receive_node_or_bun_globals (__main__.VueToolchain.test_browser_does_not_receive_node_or_bun_globals) ... ok
-test_browser_worker_and_i18n_reject_runtime_node_bun (__main__.VueToolchain.test_browser_worker_and_i18n_reject_runtime_node_bun) ... ok
-test_bun_development_types_and_runtime (__main__.VueToolchain.test_bun_development_types_and_runtime) ... ok
-test_directives_unused_any_and_typed_promises_fail (__main__.VueToolchain.test_directives_unused_any_and_typed_promises_fail) ... ok
-test_dynamic_slots_have_no_unused_false_positive (__main__.VueToolchain.test_dynamic_slots_have_no_unused_false_positive) ... ok
-test_exact_development_export_buffer_contract (__main__.VueToolchain.test_exact_development_export_buffer_contract) ... ok
-test_formatter_keeps_import_order_text_and_tailwind (__main__.VueToolchain.test_formatter_keeps_import_order_text_and_tailwind) ... ok
-test_generated_sfc_types_and_failed_refresh_have_no_waiver (__main__.VueToolchain.test_generated_sfc_types_and_failed_refresh_have_no_waiver) ... ok
-test_indexed_access_preserves_missing_route_fallbacks (__main__.VueToolchain.test_indexed_access_preserves_missing_route_fallbacks) ... ok
-test_multiple_declaration_projects_fail_closed_together (__main__.VueToolchain.test_multiple_declaration_projects_fail_closed_together) ... ok
-test_node_test_runner_failure_propagation_and_no_waiver (__main__.VueToolchain.test_node_test_runner_failure_propagation_and_no_waiver) ... ok
-test_qualified_runtime_globals_and_local_names (__main__.VueToolchain.test_qualified_runtime_globals_and_local_names) ... ok
-test_runtime_and_positive_sfc (__main__.VueToolchain.test_runtime_and_positive_sfc) ... ok
-test_strict_script_template_props_emits_and_slots_types (__main__.VueToolchain.test_strict_script_template_props_emits_and_slots_types) ... ok
-test_unprepared_sfc_import_has_no_fake_fallback (__main__.VueToolchain.test_unprepared_sfc_import_has_no_fake_fallback) ... ok
-test_unused_disables_warnings_and_unmatched_paths_fail (__main__.VueToolchain.test_unused_disables_warnings_and_unmatched_paths_fail) ... ok
-test_y_text_declared_string_contract_without_rule_allowance (__main__.VueToolchain.test_y_text_declared_string_contract_without_rule_allowance) ... ok
+bun test v1.4.2 (744846f84)
 
-----------------------------------------------------------------------
-Ran 19 tests in 214.633s
+scripts/ci/eslint-fixtures.test.ts:
+(pass) test_actual_nuxt_autoimport_registration_boundary [914.84ms]
+(pass) test_actual_web_and_editor_declaration_preparation_is_node_free [11858.92ms]
+(pass) test_browser_does_not_receive_node_or_bun_globals [2032.28ms]
+(pass) test_browser_worker_and_i18n_reject_runtime_node_bun [31323.11ms]
+(pass) test_bun_development_types_and_runtime [9757.89ms]
+(pass) test_directives_unused_any_and_typed_promises_fail [33522.88ms]
+(pass) test_dynamic_slots_have_no_unused_false_positive [3726.50ms]
+(pass) test_exact_development_export_buffer_contract [29114.43ms]
+(pass) test_formatter_keeps_import_order_text_and_tailwind [199.10ms]
+(pass) test_generated_sfc_types_and_failed_refresh_have_no_waiver [6535.78ms]
+(pass) test_indexed_access_preserves_missing_route_fallbacks [9733.60ms]
+(pass) test_multiple_declaration_projects_fail_closed_together [6059.21ms]
+(pass) test_node_test_runner_failure_propagation_and_no_waiver [5883.84ms]
+(pass) test_qualified_runtime_globals_and_local_names [19737.83ms]
+(pass) test_runtime_and_positive_sfc [5690.67ms]
+(pass) test_strict_script_template_props_emits_and_slots_types [12921.40ms]
+(pass) test_unprepared_sfc_import_has_no_fake_fallback [6343.62ms]
+(pass) test_unused_disables_warnings_and_unmatched_paths_fail [5756.90ms]
+(pass) test_y_text_declared_string_contract_without_rule_allowance [8178.49ms]
+(pass) M1 missing path fails closed [46.63ms]
+(pass) negative control: @typescript-eslint/no-unused-vars [5638.89ms]
+(pass) negative control: vue/no-parsing-error [5565.49ms]
+(pass) negative control: vue/require-v-for-key [5764.00ms]
+(pass) negative control: vue/valid-v-for [5944.19ms]
+(pass) negative control: vue/no-use-v-if-with-v-for [5937.06ms]
+(pass) negative control: vue/valid-v-if [5910.81ms]
+(pass) negative control: vue/valid-v-model [5678.49ms]
+(pass) negative control: @typescript-eslint/no-explicit-any [5477.42ms]
+(pass) negative control: @typescript-eslint/no-unsafe-return [5992.27ms]
+(pass) negative control: @typescript-eslint/no-floating-promises [5511.65ms]
+(pass) negative control: @typescript-eslint/no-unnecessary-condition [5609.26ms]
+(pass) negative control: @typescript-eslint/no-base-to-string [5813.82ms]
+(pass) negative control: @typescript-eslint/no-unsafe-argument [5554.85ms]
+(pass) negative control: unused eslint-disable [5540.33ms]
+(pass) negative control: vue/no-v-html warnings fail the run [6090.78ms]
+(pass) negative control: no-restricted-globals browser [5955.48ms]
+(pass) negative control: no-restricted-globals worker [5696.50ms]
+(pass) negative control: no-restricted-globals library [8140.48ms]
+(pass) negative control: no-restricted-globals export [7490.20ms]
+(pass) negative control: no-restricted-globals buffer outside export [7582.33ms]
+(pass) negative control: no-restricted-globals qualified [5770.42ms]
+(pass) signal-killed child fails the proof [4.23ms]
 
-OK
+ 42 pass
+ 0 fail
+ 290 expect() calls
+Ran 42 tests across 1 file. [336.03s]
```

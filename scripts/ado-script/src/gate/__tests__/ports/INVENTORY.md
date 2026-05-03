# TypeScript gate test port inventory

Source: `tests/gate_eval_tests.py` (deleted; this file documents the 1:1
mapping done at port time, retained as a parity audit artifact).

- Python `def test_...` cases inventoried: 45
- `pytest.parametrize` decorators/cases found: 0
- Python cases ported: 45
- Python cases marked obsolete: 0
- Extra TS parity guards added: 6 (`glob` DOTALL, label case-insensitivity, file-glob empty/include/exclude combinations, and one `evaluatePredicates`/`PolicyTracker` integration guard)

## TestStripRefPrefix

| Python test | TS port location |
| --- | --- |
| `test_refs_heads` | `ref-prefix.test.ts` / `TestStripRefPrefix` / `test refs heads` |
| `test_refs_tags` | `ref-prefix.test.ts` / `TestStripRefPrefix` / `test refs tags` |
| `test_refs_pull` | `ref-prefix.test.ts` / `TestStripRefPrefix` / `test refs pull` |
| `test_no_prefix` | `ref-prefix.test.ts` / `TestStripRefPrefix` / `test no prefix` |
| `test_pattern_stripping_in_glob` | `ref-prefix.test.ts` / `TestStripRefPrefix` / `test pattern stripping in glob` |

## TestGlobMatch

| Python test | TS port location |
| --- | --- |
| `test_match` | `glob.test.ts` / `TestGlobMatch` / `test match` |
| `test_no_match` | `glob.test.ts` / `TestGlobMatch` / `test no match` |
| `test_wildcard` | `glob.test.ts` / `TestGlobMatch` / `test wildcard` |
| `test_exact` | `glob.test.ts` / `TestGlobMatch` / `test exact` |
| `test_exact_no_match` | `glob.test.ts` / `TestGlobMatch` / `test exact no match` |
| `test_empty_value` | `glob.test.ts` / `TestGlobMatch` / `test empty value` |

Additional guard: `glob.test.ts` / `TestGlobMatch` / `test dotall across newlines` verifies Python `_glob(..., flags=DOTALL)` parity.

## TestEquals

| Python test | TS port location |
| --- | --- |
| `test_match` | `equals.test.ts` / `TestEquals` / `test match` |
| `test_no_match` | `equals.test.ts` / `TestEquals` / `test no match` |
| `test_missing_fact` | `equals.test.ts` / `TestEquals` / `test missing fact` |

## TestValueInSet

| Python test | TS port location |
| --- | --- |
| `test_case_insensitive_match` | `value-set.test.ts` / `TestValueInSet` / `test case insensitive match` |
| `test_case_sensitive_no_match` | `value-set.test.ts` / `TestValueInSet` / `test case sensitive no match` |
| `test_not_in_set` | `value-set.test.ts` / `TestValueInSet` / `test not in set` |

## TestValueNotInSet

| Python test | TS port location |
| --- | --- |
| `test_not_in_set` | `value-set.test.ts` / `TestValueNotInSet` / `test not in set` |
| `test_in_set` | `value-set.test.ts` / `TestValueNotInSet` / `test in set` |

## TestNumericRange

| Python test | TS port location |
| --- | --- |
| `test_in_range` | `numeric-range.test.ts` / `TestNumericRange` / `test in range` |
| `test_below_min` | `numeric-range.test.ts` / `TestNumericRange` / `test below min` |
| `test_above_max` | `numeric-range.test.ts` / `TestNumericRange` / `test above max` |
| `test_min_only` | `numeric-range.test.ts` / `TestNumericRange` / `test min only` |
| `test_max_only` | `numeric-range.test.ts` / `TestNumericRange` / `test max only` |

## TestTimeWindow

| Python test | TS port location |
| --- | --- |
| `test_in_window` | `time-window.test.ts` / `TestTimeWindow` / `test in window` |
| `test_outside_window` | `time-window.test.ts` / `TestTimeWindow` / `test outside window` |
| `test_overnight_window_in` | `time-window.test.ts` / `TestTimeWindow` / `test overnight window in` |
| `test_overnight_window_out` | `time-window.test.ts` / `TestTimeWindow` / `test overnight window out` |

## TestLabelSetMatch

| Python test | TS port location |
| --- | --- |
| `test_any_of_match` | `label-set.test.ts` / `TestLabelSetMatch` / `test any of match` |
| `test_any_of_no_match` | `label-set.test.ts` / `TestLabelSetMatch` / `test any of no match` |
| `test_all_of_match` | `label-set.test.ts` / `TestLabelSetMatch` / `test all of match` |
| `test_all_of_missing` | `label-set.test.ts` / `TestLabelSetMatch` / `test all of missing` |
| `test_none_of_pass` | `label-set.test.ts` / `TestLabelSetMatch` / `test none of pass` |
| `test_none_of_fail` | `label-set.test.ts` / `TestLabelSetMatch` / `test none of fail` |
| `test_empty_labels` | `label-set.test.ts` / `TestLabelSetMatch` / `test empty labels` |

Additional guard: `label-set.test.ts` / `TestLabelSetMatch` / `test case insensitive labels` verifies the Python lower-case label comparison behavior.

## TestFileGlobMatch

| Python test | TS port location |
| --- | --- |
| `test_include_match` | `file-glob.test.ts` / `TestFileGlobMatch` / `test include match` |
| `test_include_no_match` | `file-glob.test.ts` / `TestFileGlobMatch` / `test include no match` |
| `test_exclude` | `file-glob.test.ts` / `TestFileGlobMatch` / `test exclude` |

Additional guards in `file-glob.test.ts` cover Python's empty-list include failure, exclude-only vacuous success, and include+exclude allowing a non-excluded match.

## TestLogicalCombinators

| Python test | TS port location |
| --- | --- |
| `test_and_all_pass` | `logical.test.ts` / `TestLogicalCombinators` / `test and all pass` |
| `test_and_one_fails` | `logical.test.ts` / `TestLogicalCombinators` / `test and one fails` |
| `test_or_one_passes` | `logical.test.ts` / `TestLogicalCombinators` / `test or one passes` |
| `test_not` | `logical.test.ts` / `TestLogicalCombinators` / `test not` |

## TestPredicateFacts

| Python test | TS port location |
| --- | --- |
| `test_simple` | `predicate-facts.test.ts` / `TestPredicateFacts` / `test simple` |
| `test_compound` | `predicate-facts.test.ts` / `TestPredicateFacts` / `test compound` |
| `test_not` | `predicate-facts.test.ts` / `TestPredicateFacts` / `test not` |

## Full-loop / policy cases

The deleted Python test suite had no full `main()`/`GateSpec` loop cases
and no policy state-machine cases. A small additional `integration.test.ts`
guard covers `evaluatePredicates(spec, facts, tracker)` with a real
`PolicyTracker`, but no Python case is mapped here.

## Divergences

No Python/TS behavioral divergences were found while porting the inventoried Python cases. No TS implementation changes were required.

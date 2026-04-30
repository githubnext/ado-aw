"""Unit tests for the ado-aw gate evaluator (scripts/gate-eval.py).

Run with: uv run pytest tests/gate_eval_tests.py -v
"""
import base64
import json
import os
import sys

# Add scripts/ to path so we can import the evaluator module
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "scripts"))

# Import evaluator functions directly
import importlib.util
spec = importlib.util.spec_from_file_location(
    "gate_eval",
    os.path.join(os.path.dirname(__file__), "..", "scripts", "gate-eval.py"),
)
gate_eval = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate_eval)

evaluate = gate_eval.evaluate
predicate_facts = gate_eval.predicate_facts


# ─── Predicate evaluation tests ─────────────────────────────────────────────


class TestRegexMatch:
    def test_match(self):
        pred = {"type": "regex_match", "fact": "pr_title", "pattern": r"\[review\]"}
        facts = {"pr_title": "feat: add feature [review]"}
        assert evaluate(pred, facts) is True

    def test_no_match(self):
        pred = {"type": "regex_match", "fact": "pr_title", "pattern": r"\[review\]"}
        facts = {"pr_title": "feat: add feature"}
        assert evaluate(pred, facts) is False

    def test_empty_value(self):
        pred = {"type": "regex_match", "fact": "pr_title", "pattern": ".*"}
        facts = {"pr_title": ""}
        assert evaluate(pred, facts) is True


class TestEquals:
    def test_match(self):
        pred = {"type": "equals", "fact": "pr_is_draft", "value": "false"}
        facts = {"pr_is_draft": "false"}
        assert evaluate(pred, facts) is True

    def test_no_match(self):
        pred = {"type": "equals", "fact": "pr_is_draft", "value": "false"}
        facts = {"pr_is_draft": "true"}
        assert evaluate(pred, facts) is False

    def test_missing_fact(self):
        pred = {"type": "equals", "fact": "missing", "value": "x"}
        facts = {}
        assert evaluate(pred, facts) is False


class TestValueInSet:
    def test_case_insensitive_match(self):
        pred = {
            "type": "value_in_set",
            "fact": "author_email",
            "values": ["Alice@Corp.com"],
            "case_insensitive": True,
        }
        facts = {"author_email": "alice@corp.com"}
        assert evaluate(pred, facts) is True

    def test_case_sensitive_no_match(self):
        pred = {
            "type": "value_in_set",
            "fact": "author_email",
            "values": ["Alice@Corp.com"],
            "case_insensitive": False,
        }
        facts = {"author_email": "alice@corp.com"}
        assert evaluate(pred, facts) is False

    def test_not_in_set(self):
        pred = {
            "type": "value_in_set",
            "fact": "build_reason",
            "values": ["PullRequest", "Manual"],
            "case_insensitive": True,
        }
        facts = {"build_reason": "Schedule"}
        assert evaluate(pred, facts) is False


class TestValueNotInSet:
    def test_not_in_set(self):
        pred = {
            "type": "value_not_in_set",
            "fact": "author_email",
            "values": ["bot@noreply.com"],
            "case_insensitive": True,
        }
        facts = {"author_email": "dev@corp.com"}
        assert evaluate(pred, facts) is True

    def test_in_set(self):
        pred = {
            "type": "value_not_in_set",
            "fact": "author_email",
            "values": ["bot@noreply.com"],
            "case_insensitive": True,
        }
        facts = {"author_email": "bot@noreply.com"}
        assert evaluate(pred, facts) is False


class TestNumericRange:
    def test_in_range(self):
        pred = {"type": "numeric_range", "fact": "changed_file_count", "min": 5, "max": 100}
        facts = {"changed_file_count": 50}
        assert evaluate(pred, facts) is True

    def test_below_min(self):
        pred = {"type": "numeric_range", "fact": "changed_file_count", "min": 5, "max": 100}
        facts = {"changed_file_count": 2}
        assert evaluate(pred, facts) is False

    def test_above_max(self):
        pred = {"type": "numeric_range", "fact": "changed_file_count", "min": 5, "max": 100}
        facts = {"changed_file_count": 200}
        assert evaluate(pred, facts) is False

    def test_min_only(self):
        pred = {"type": "numeric_range", "fact": "changed_file_count", "min": 3}
        facts = {"changed_file_count": 10}
        assert evaluate(pred, facts) is True

    def test_max_only(self):
        pred = {"type": "numeric_range", "fact": "changed_file_count", "max": 50}
        facts = {"changed_file_count": 100}
        assert evaluate(pred, facts) is False


class TestTimeWindow:
    def test_in_window(self):
        pred = {"type": "time_window", "start": "09:00", "end": "17:00"}
        facts = {"current_utc_minutes": 600}  # 10:00
        assert evaluate(pred, facts) is True

    def test_outside_window(self):
        pred = {"type": "time_window", "start": "09:00", "end": "17:00"}
        facts = {"current_utc_minutes": 1200}  # 20:00
        assert evaluate(pred, facts) is False

    def test_overnight_window_in(self):
        pred = {"type": "time_window", "start": "22:00", "end": "06:00"}
        facts = {"current_utc_minutes": 1380}  # 23:00
        assert evaluate(pred, facts) is True

    def test_overnight_window_out(self):
        pred = {"type": "time_window", "start": "22:00", "end": "06:00"}
        facts = {"current_utc_minutes": 720}  # 12:00
        assert evaluate(pred, facts) is False


class TestLabelSetMatch:
    def test_any_of_match(self):
        pred = {
            "type": "label_set_match",
            "fact": "pr_labels",
            "any_of": ["run-agent", "needs-review"],
        }
        facts = {"pr_labels": ["run-agent", "other"]}
        assert evaluate(pred, facts) is True

    def test_any_of_no_match(self):
        pred = {
            "type": "label_set_match",
            "fact": "pr_labels",
            "any_of": ["run-agent"],
        }
        facts = {"pr_labels": ["other"]}
        assert evaluate(pred, facts) is False

    def test_all_of_match(self):
        pred = {
            "type": "label_set_match",
            "fact": "pr_labels",
            "all_of": ["approved", "tested"],
        }
        facts = {"pr_labels": ["approved", "tested", "other"]}
        assert evaluate(pred, facts) is True

    def test_all_of_missing(self):
        pred = {
            "type": "label_set_match",
            "fact": "pr_labels",
            "all_of": ["approved", "tested"],
        }
        facts = {"pr_labels": ["approved"]}
        assert evaluate(pred, facts) is False

    def test_none_of_pass(self):
        pred = {
            "type": "label_set_match",
            "fact": "pr_labels",
            "none_of": ["do-not-run"],
        }
        facts = {"pr_labels": ["run-agent"]}
        assert evaluate(pred, facts) is True

    def test_none_of_fail(self):
        pred = {
            "type": "label_set_match",
            "fact": "pr_labels",
            "none_of": ["do-not-run"],
        }
        facts = {"pr_labels": ["do-not-run", "other"]}
        assert evaluate(pred, facts) is False

    def test_empty_labels(self):
        pred = {"type": "label_set_match", "fact": "pr_labels"}
        facts = {"pr_labels": []}
        assert evaluate(pred, facts) is True


class TestFileGlobMatch:
    def test_include_match(self):
        pred = {
            "type": "file_glob_match",
            "fact": "changed_files",
            "include": ["src/*.rs"],
        }
        facts = {"changed_files": ["src/main.rs", "src/lib.rs"]}
        assert evaluate(pred, facts) is True

    def test_include_no_match(self):
        pred = {
            "type": "file_glob_match",
            "fact": "changed_files",
            "include": ["src/**/*.rs"],
        }
        facts = {"changed_files": ["docs/readme.md"]}
        assert evaluate(pred, facts) is False

    def test_exclude(self):
        pred = {
            "type": "file_glob_match",
            "fact": "changed_files",
            "include": ["src/**/*.rs"],
            "exclude": ["src/test_*.rs"],
        }
        facts = {"changed_files": ["src/test_main.rs"]}
        assert evaluate(pred, facts) is False


class TestLogicalCombinators:
    def test_and_all_pass(self):
        pred = {
            "type": "and",
            "operands": [
                {"type": "equals", "fact": "a", "value": "1"},
                {"type": "equals", "fact": "b", "value": "2"},
            ],
        }
        facts = {"a": "1", "b": "2"}
        assert evaluate(pred, facts) is True

    def test_and_one_fails(self):
        pred = {
            "type": "and",
            "operands": [
                {"type": "equals", "fact": "a", "value": "1"},
                {"type": "equals", "fact": "b", "value": "3"},
            ],
        }
        facts = {"a": "1", "b": "2"}
        assert evaluate(pred, facts) is False

    def test_or_one_passes(self):
        pred = {
            "type": "or",
            "operands": [
                {"type": "equals", "fact": "a", "value": "wrong"},
                {"type": "equals", "fact": "b", "value": "2"},
            ],
        }
        facts = {"a": "1", "b": "2"}
        assert evaluate(pred, facts) is True

    def test_not(self):
        pred = {
            "type": "not",
            "operand": {"type": "equals", "fact": "a", "value": "1"},
        }
        facts = {"a": "2"}
        assert evaluate(pred, facts) is True


# ─── predicate_facts helper tests ────────────────────────────────────────────


class TestPredicateFacts:
    def test_simple(self):
        pred = {"type": "regex_match", "fact": "pr_title", "pattern": "test"}
        assert predicate_facts(pred) == {"pr_title"}

    def test_compound(self):
        pred = {
            "type": "and",
            "operands": [
                {"type": "equals", "fact": "a", "value": "1"},
                {"type": "regex_match", "fact": "b", "pattern": "x"},
            ],
        }
        assert predicate_facts(pred) == {"a", "b"}

    def test_not(self):
        pred = {"type": "not", "operand": {"type": "equals", "fact": "x", "value": "1"}}
        assert predicate_facts(pred) == {"x"}

#!/usr/bin/env python3
"""ado-aw gate evaluator — data-driven trigger filter evaluation.

Reads a base64-encoded JSON gate spec from the GATE_SPEC environment variable,
acquires runtime facts, evaluates filter predicates, and reports results via
ADO logging commands.

This script is embedded by the ado-aw compiler into pipeline gate steps.
It should not be modified directly — changes belong in src/compile/filter_ir.rs.
"""
import base64, fnmatch, json, os, sys
from datetime import datetime, timezone

# ─── Fact dependencies ───────────────────────────────────────────────────────

FACT_DEPS = {
    "pr_is_draft": ["pr_metadata"],
    "pr_labels": ["pr_metadata"],
    "changed_file_count": ["changed_files"],
}

# ─── Fact acquisition ────────────────────────────────────────────────────────

def acquire_fact(kind, acquired):
    """Acquire a fact value by kind. Returns the value or raises on failure."""
    # Pipeline variables (from ADO macro exports)
    env_facts = {
        "pr_title": "ADO_PR_TITLE",
        "author_email": "ADO_AUTHOR_EMAIL",
        "source_branch": "ADO_SOURCE_BRANCH",
        "target_branch": "ADO_TARGET_BRANCH",
        "commit_message": "ADO_COMMIT_MESSAGE",
        "build_reason": "ADO_BUILD_REASON",
        "triggered_by_pipeline": "ADO_TRIGGERED_BY_PIPELINE",
        "triggering_branch": "ADO_TRIGGERING_BRANCH",
    }
    if kind in env_facts:
        return os.environ.get(env_facts[kind], "")

    if kind == "pr_metadata":
        return _fetch_pr_metadata()

    if kind == "pr_is_draft":
        md = acquired.get("pr_metadata")
        if md is None:
            return "unknown"
        data = json.loads(md) if isinstance(md, str) else md
        return str(data.get("isDraft", False)).lower()

    if kind == "pr_labels":
        md = acquired.get("pr_metadata")
        if md is None:
            return []
        data = json.loads(md) if isinstance(md, str) else md
        return [l.get("name", "") for l in data.get("labels", [])]

    if kind == "changed_files":
        return _fetch_changed_files()

    if kind == "changed_file_count":
        files = acquired.get("changed_files", [])
        return len(files) if isinstance(files, list) else 0

    if kind == "current_utc_minutes":
        now = datetime.now(timezone.utc)
        return now.hour * 60 + now.minute

    raise ValueError(f"Unknown fact kind: {kind}")


def _fetch_pr_metadata():
    """Fetch PR metadata from ADO REST API."""
    from urllib.request import Request, urlopen
    token = os.environ.get("SYSTEM_ACCESSTOKEN", "")
    org_url = os.environ.get("ADO_COLLECTION_URI", "")
    project = os.environ.get("ADO_PROJECT", "")
    repo_id = os.environ.get("ADO_REPO_ID", "")
    pr_id = os.environ.get("ADO_PR_ID", "")
    if not all([token, org_url, project, repo_id, pr_id]):
        raise RuntimeError("Missing ADO environment variables for PR metadata")
    url = f"{org_url}{project}/_apis/git/repositories/{repo_id}/pullRequests/{pr_id}?api-version=7.1"
    req = Request(url, headers={"Authorization": f"Bearer {token}"})
    with urlopen(req, timeout=30) as resp:
        return json.loads(resp.read())


def _fetch_changed_files():
    """Fetch changed files via PR iterations API."""
    from urllib.request import Request, urlopen
    token = os.environ.get("SYSTEM_ACCESSTOKEN", "")
    org_url = os.environ.get("ADO_COLLECTION_URI", "")
    project = os.environ.get("ADO_PROJECT", "")
    repo_id = os.environ.get("ADO_REPO_ID", "")
    pr_id = os.environ.get("ADO_PR_ID", "")
    if not all([token, org_url, project, repo_id, pr_id]):
        raise RuntimeError("Missing ADO environment variables for changed files")
    base = f"{org_url}{project}/_apis/git/repositories/{repo_id}/pullRequests/{pr_id}"
    headers = {"Authorization": f"Bearer {token}"}
    # Get iterations
    req = Request(f"{base}/iterations?api-version=7.1", headers=headers)
    with urlopen(req, timeout=30) as resp:
        iters = json.loads(resp.read()).get("value", [])
    if not iters:
        return []
    last_iter = iters[-1]["id"]
    # Get changes for last iteration
    req = Request(f"{base}/iterations/{last_iter}/changes?api-version=7.1", headers=headers)
    with urlopen(req, timeout=30) as resp:
        changes = json.loads(resp.read())
    return [
        entry.get("item", {}).get("path", "").lstrip("/")
        for entry in changes.get("changeEntries", [])
        if entry.get("item", {}).get("path")
    ]


# ─── Predicate evaluation ───────────────────────────────────────────────────

def evaluate(pred, facts):
    """Evaluate a predicate against acquired facts. Returns True if passed."""
    t = pred["type"]

    if t == "glob_match":
        value = str(facts.get(pred["fact"], ""))
        # Simple glob: * matches anything, ? matches single char.
        # Brackets are NOT character classes (treated literally).
        import re as _re
        pattern = pred["pattern"]
        # Escape everything except * and ?, then convert * → .* and ? → .
        regex = _re.escape(pattern).replace(r"\*", ".*").replace(r"\?", ".")
        return bool(_re.fullmatch(regex, value))

    if t == "equals":
        value = str(facts.get(pred["fact"], ""))
        return value == pred["value"]

    if t == "value_in_set":
        value = str(facts.get(pred["fact"], ""))
        values = pred["values"]
        if pred.get("case_insensitive"):
            return value.lower() in [v.lower() for v in values]
        return value in values

    if t == "value_not_in_set":
        value = str(facts.get(pred["fact"], ""))
        values = pred["values"]
        if pred.get("case_insensitive"):
            return value.lower() not in [v.lower() for v in values]
        return value not in values

    if t == "numeric_range":
        value = int(facts.get(pred["fact"], 0))
        mn = pred.get("min")
        mx = pred.get("max")
        if mn is not None and value < mn:
            return False
        if mx is not None and value > mx:
            return False
        return True

    if t == "time_window":
        current = int(facts.get("current_utc_minutes", 0))
        sh, sm = pred["start"].split(":")
        eh, em = pred["end"].split(":")
        start = int(sh) * 60 + int(sm)
        end = int(eh) * 60 + int(em)
        if start <= end:
            return start <= current < end
        else:  # overnight window
            return current >= start or current < end

    if t == "label_set_match":
        labels = facts.get(pred["fact"]) or []
        if isinstance(labels, str):
            labels = [l.strip() for l in labels.split("\n") if l.strip()]
        labels_lower = [l.lower() for l in labels]
        any_of = pred.get("any_of", [])
        all_of = pred.get("all_of", [])
        none_of = pred.get("none_of", [])
        if any_of and not any(a.lower() in labels_lower for a in any_of):
            return False
        if all_of and not all(a.lower() in labels_lower for a in all_of):
            return False
        if none_of and any(n.lower() in labels_lower for n in none_of):
            return False
        return True

    if t == "file_glob_match":
        files = facts.get(pred["fact"]) or []
        if isinstance(files, str):
            files = [f.strip() for f in files.split("\n") if f.strip()]
        includes = pred.get("include", [])
        excludes = pred.get("exclude", [])
        # Empty file list: exclude-only filters pass (no excluded files present),
        # include filters fail (nothing to match against)
        if not files:
            if not includes:
                return True  # exclude-only: vacuously true (no bad files)
            log("  (changed-files: no files in PR — filter will not match)")
            return False
        for f in files:
            inc = not includes or any(fnmatch.fnmatch(f, p) for p in includes)
            exc = any(fnmatch.fnmatch(f, p) for p in excludes)
            if inc and not exc:
                return True
        return False

    if t == "and":
        return all(evaluate(p, facts) for p in pred["operands"])

    if t == "or":
        return any(evaluate(p, facts) for p in pred["operands"])

    if t == "not":
        return not evaluate(pred["operand"], facts)

    log(f"##[warning]Unknown predicate type: {t}")
    return True


def predicate_facts(pred):
    """Collect fact IDs referenced by a predicate (for skip checking)."""
    t = pred["type"]
    result = set()
    if "fact" in pred:
        result.add(pred["fact"])
    if t in ("and", "or"):
        for p in pred.get("operands", []):
            result.update(predicate_facts(p))
    if t == "not":
        result.update(predicate_facts(pred.get("operand", {})))
    return result


# ─── Helpers ─────────────────────────────────────────────────────────────────

def log(msg):
    print(msg, flush=True)

def vso_output(name, value):
    log(f"##vso[task.setvariable variable={name};isOutput=true]{value}")

def vso_tag(tag):
    log(f"##vso[build.addbuildtag]{tag}")

def self_cancel():
    from urllib.request import Request, urlopen
    token = os.environ.get("SYSTEM_ACCESSTOKEN", "")
    org_url = os.environ.get("ADO_COLLECTION_URI", "")
    project = os.environ.get("ADO_PROJECT", "")
    build_id = os.environ.get("ADO_BUILD_ID", "")
    if not all([token, org_url, project, build_id]):
        log("##[warning]Cannot self-cancel: missing ADO environment variables")
        return
    url = f"{org_url}{project}/_apis/build/builds/{build_id}?api-version=7.1"
    data = json.dumps({"status": "cancelling"}).encode()
    req = Request(url, data=data, method="PATCH", headers={
        "Authorization": f"Bearer {token}",
        "Content-Type": "application/json",
    })
    try:
        with urlopen(req, timeout=30) as resp:
            resp.read()
    except Exception as e:
        log(f"##[warning]Self-cancel failed: {e}")


# ─── Main ────────────────────────────────────────────────────────────────────

def main():
    spec = json.loads(base64.b64decode(os.environ["GATE_SPEC"]))
    ctx = spec["context"]

    # Bypass for non-matching trigger types
    build_reason = os.environ.get("ADO_BUILD_REASON", "")
    if build_reason != ctx["build_reason"]:
        log(f"Not a {ctx['bypass_label']} build -- gate passes automatically")
        vso_output("SHOULD_RUN", "true")
        vso_tag(f"{ctx['tag_prefix']}:passed")
        sys.exit(0)

    # Acquire facts (dependency-ordered)
    facts = {}
    skip_facts = set()
    fail_open_facts = set()
    should_run = True
    for fact_spec in spec["facts"]:
        kind = fact_spec["kind"]
        policy = fact_spec.get("failure_policy", "fail_closed")
        deps = FACT_DEPS.get(kind, [])
        if any(d in skip_facts for d in deps):
            skip_facts.add(kind)
            log(f"  Fact [{kind}]: skipped (dependency unavailable)")
            continue
        try:
            facts[kind] = acquire_fact(kind, facts)
            log(f"  Fact [{kind}]: acquired")
        except Exception as e:
            log(f"##[warning]Fact [{kind}]: acquisition failed ({e})")
            if policy == "skip_dependents":
                skip_facts.add(kind)
            elif policy == "fail_open":
                facts[kind] = None
                fail_open_facts.add(kind)
            else:
                # fail_closed: gate fails, skip dependent checks
                facts[kind] = None
                skip_facts.add(kind)
                should_run = False
                vso_tag(f"{ctx['tag_prefix']}:{kind}-unavailable")

    # Evaluate checks
    for check in spec["checks"]:
        name = check["name"]
        required = predicate_facts(check["predicate"])
        if any(f in skip_facts for f in required):
            log(f"  Filter: {name} | Result: SKIPPED (dependency unavailable)")
            continue
        if any(f in fail_open_facts for f in required):
            log(f"  Filter: {name} | Result: PASS (fail-open)")
            continue
        passed = evaluate(check["predicate"], facts)
        if passed:
            log(f"  Filter: {name} | Result: PASS")
        else:
            tag = f"{ctx['tag_prefix']}:{check['tag_suffix']}"
            log(f"##[warning]Filter {name} did not match")
            vso_tag(tag)
            should_run = False

    # Report result
    vso_output("SHOULD_RUN", str(should_run).lower())
    if should_run:
        log("All filters passed -- agent will run")
        vso_tag(f"{ctx['tag_prefix']}:passed")
    else:
        log("Filters not matched -- cancelling build")
        vso_tag(f"{ctx['tag_prefix']}:skipped")
        self_cancel()

if __name__ == "__main__":
    main()

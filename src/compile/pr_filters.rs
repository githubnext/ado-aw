//! PR trigger filter logic.
//!
//! This module handles the generation of:
//! - Native ADO PR trigger blocks (branches/paths)
//! - Pre-activation gate steps that evaluate runtime PR filters
//! - Self-cancellation via ADO REST API when filters don't match
//!
//! Gate steps are injected into the Setup job. Non-PR builds bypass the gate
//! entirely. Cancelled builds are invisible to `DownloadPipelineArtifact@2`,
//! naturally preserving the cache-memory artifact chain.

use super::types::{PrFilters, PrTriggerConfig};

// ─── Native ADO PR trigger ──────────────────────────────────────────────────

/// Generate native ADO PR trigger block from PrTriggerConfig.
pub(super) fn generate_native_pr_trigger(pr: &PrTriggerConfig) -> String {
    let has_branches = pr
        .branches
        .as_ref()
        .is_some_and(|b| !b.include.is_empty() || !b.exclude.is_empty());
    let has_paths = pr
        .paths
        .as_ref()
        .is_some_and(|p| !p.include.is_empty() || !p.exclude.is_empty());

    if !has_branches && !has_paths {
        return String::new();
    }

    let mut yaml = String::from("pr:\n");

    if let Some(branches) = &pr.branches {
        if !branches.include.is_empty() || !branches.exclude.is_empty() {
            yaml.push_str("  branches:\n");
            if !branches.include.is_empty() {
                yaml.push_str("    include:\n");
                for b in &branches.include {
                    yaml.push_str(&format!("      - '{}'\n", b.replace('\'', "''")));
                }
            }
            if !branches.exclude.is_empty() {
                yaml.push_str("    exclude:\n");
                for b in &branches.exclude {
                    yaml.push_str(&format!("      - '{}'\n", b.replace('\'', "''")));
                }
            }
        }
    }

    if let Some(paths) = &pr.paths {
        if !paths.include.is_empty() || !paths.exclude.is_empty() {
            yaml.push_str("  paths:\n");
            if !paths.include.is_empty() {
                yaml.push_str("    include:\n");
                for p in &paths.include {
                    yaml.push_str(&format!("      - '{}'\n", p.replace('\'', "''")));
                }
            }
            if !paths.exclude.is_empty() {
                yaml.push_str("    exclude:\n");
                for p in &paths.exclude {
                    yaml.push_str(&format!("      - '{}'\n", p.replace('\'', "''")));
                }
            }
        }
    }

    yaml.trim_end().to_string()
}

// ─── Gate step generation ───────────────────────────────────────────────────

/// Generate the bash gate step for PR filter evaluation.
///
/// The step evaluates all configured filters and sets a `SHOULD_RUN` output
/// variable. If any filter fails, the build is self-cancelled via the ADO
/// REST API. Non-PR builds pass the gate automatically.
pub(super) fn generate_pr_gate_step(filters: &PrFilters) -> String {
    let mut checks = Vec::new();

    // Tier 1 filters (pipeline variables)
    generate_title_check(filters, &mut checks);
    generate_author_check(filters, &mut checks);
    generate_source_branch_check(filters, &mut checks);
    generate_target_branch_check(filters, &mut checks);

    // Tier 2 filters (REST API)
    if has_tier2_filters(filters) {
        generate_api_preamble(&mut checks);
        generate_labels_check(filters, &mut checks);
        generate_draft_check(filters, &mut checks);
        // changed-files requires a separate API call (iteration changes)
        generate_changed_files_check(filters, &mut checks);
    }

    // Tier 3 filters (advanced)
    generate_time_window_check(filters, &mut checks);
    generate_change_count_check(filters, &mut checks);
    generate_build_reason_check(filters, &mut checks);

    let filter_checks = checks.join("\n\n");

    let mut step = String::new();
    step.push_str("- bash: |\n");
    step.push_str("    if [ \"$(Build.Reason)\" != \"PullRequest\" ]; then\n");
    step.push_str("      echo \"Not a PR build -- gate passes automatically\"\n");
    step.push_str("      echo \"##vso[task.setvariable variable=SHOULD_RUN;isOutput=true]true\"\n");
    step.push_str("      echo \"##vso[build.addbuildtag]pr-gate:passed\"\n");
    step.push_str("      exit 0\n");
    step.push_str("    fi\n");
    step.push_str("\n");
    step.push_str("    SHOULD_RUN=true\n");
    step.push_str("\n");
    step.push_str(&filter_checks);
    step.push_str("\n\n");
    step.push_str("    echo \"##vso[task.setvariable variable=SHOULD_RUN;isOutput=true]$SHOULD_RUN\"\n");
    step.push_str("    if [ \"$SHOULD_RUN\" = \"true\" ]; then\n");
    step.push_str("      echo \"All PR filters passed -- agent will run\"\n");
    step.push_str("      echo \"##vso[build.addbuildtag]pr-gate:passed\"\n");
    step.push_str("    else\n");
    step.push_str("      echo \"PR filters not matched -- cancelling build\"\n");
    step.push_str("      echo \"##vso[build.addbuildtag]pr-gate:skipped\"\n");
    step.push_str("      curl -s -X PATCH \\\n");
    step.push_str("        -H \"Authorization: Bearer $SYSTEM_ACCESSTOKEN\" \\\n");
    step.push_str("        -H \"Content-Type: application/json\" \\\n");
    step.push_str("        -d '{\"status\": \"cancelling\"}' \\\n");
    step.push_str("        \"$(System.CollectionUri)$(System.TeamProject)/_apis/build/builds/$(Build.BuildId)?api-version=7.1\"\n");
    step.push_str("    fi\n");
    step.push_str("  name: prGate\n");
    step.push_str("  displayName: \"Evaluate PR filters\"\n");
    step.push_str("  env:\n");
    step.push_str("    SYSTEM_ACCESSTOKEN: $(System.AccessToken)");

    step
}

/// Returns true if any Tier 2 filter (requiring REST API) is configured.
pub(super) fn has_tier2_filters(filters: &PrFilters) -> bool {
    filters.labels.is_some() || filters.draft.is_some() || filters.changed_files.is_some()
}

/// Add a `condition:` to each step in a list of serde_yaml::Value steps.
pub(super) fn add_condition_to_steps(
    steps: &[serde_yaml::Value],
    condition: &str,
) -> Vec<serde_yaml::Value> {
    steps
        .iter()
        .map(|step| {
            let mut step = step.clone();
            if let serde_yaml::Value::Mapping(ref mut map) = step {
                map.insert(
                    serde_yaml::Value::String("condition".into()),
                    serde_yaml::Value::String(condition.into()),
                );
            }
            step
        })
        .collect()
}

// ─── Tier 1 filter generators ───────────────────────────────────────────────

fn generate_title_check(filters: &PrFilters, checks: &mut Vec<String>) {
    if let Some(title) = &filters.title {
        let pattern = shell_escape(&title.pattern);
        checks.push(format!(
            concat!(
                "  # Title filter\n",
                "  TITLE=\"$(System.PullRequest.Title)\"\n",
                "  if echo \"$TITLE\" | grep -qE '{}'; then\n",
                "    echo \"Filter: title | Pattern: {} | Result: PASS\"\n",
                "  else\n",
                "    echo \"##[warning]PR filter title did not match (pattern: {})\"\n",
                "    echo \"##vso[build.addbuildtag]pr-gate:title-mismatch\"\n",
                "    SHOULD_RUN=false\n",
                "  fi",
            ),
            pattern, pattern, pattern,
        ));
    }
}

fn generate_author_check(filters: &PrFilters, checks: &mut Vec<String>) {
    let Some(author) = &filters.author else {
        return;
    };
    let mut author_check =
        String::from("  # Author filter\n  AUTHOR=\"$(Build.RequestedForEmail)\"\n");
    if !author.include.is_empty() {
        let emails: Vec<String> = author.include.iter().map(|e| shell_escape(e)).collect();
        let pattern = emails.join("|");
        author_check.push_str(&format!(
            concat!(
                "  if echo \"$AUTHOR\" | grep -qiE '^({})$'; then\n",
                "    echo \"Filter: author include | Result: PASS\"\n",
                "  else\n",
                "    echo \"##[warning]PR filter author did not match include list\"\n",
                "    echo \"##vso[build.addbuildtag]pr-gate:author-mismatch\"\n",
                "    SHOULD_RUN=false\n",
                "  fi",
            ),
            pattern,
        ));
    }
    if !author.exclude.is_empty() {
        let emails: Vec<String> = author.exclude.iter().map(|e| shell_escape(e)).collect();
        let pattern = emails.join("|");
        author_check.push_str(&format!(
            concat!(
                "\n  if echo \"$AUTHOR\" | grep -qiE '^({})$'; then\n",
                "    echo \"##[warning]PR filter author matched exclude list\"\n",
                "    echo \"##vso[build.addbuildtag]pr-gate:author-excluded\"\n",
                "    SHOULD_RUN=false\n",
                "  else\n",
                "    echo \"Filter: author exclude | Result: PASS (not in exclude list)\"\n",
                "  fi",
            ),
            pattern,
        ));
    }
    checks.push(author_check);
}

fn generate_source_branch_check(filters: &PrFilters, checks: &mut Vec<String>) {
    if let Some(source) = &filters.source_branch {
        let pattern = shell_escape(&source.pattern);
        checks.push(format!(
            concat!(
                "  # Source branch filter\n",
                "  SOURCE_BRANCH=\"$(System.PullRequest.SourceBranch)\"\n",
                "  if echo \"$SOURCE_BRANCH\" | grep -qE '{}'; then\n",
                "    echo \"Filter: source-branch | Pattern: {} | Result: PASS\"\n",
                "  else\n",
                "    echo \"##[warning]PR filter source-branch did not match (pattern: {})\"\n",
                "    echo \"##vso[build.addbuildtag]pr-gate:source-branch-mismatch\"\n",
                "    SHOULD_RUN=false\n",
                "  fi",
            ),
            pattern, pattern, pattern,
        ));
    }
}

fn generate_target_branch_check(filters: &PrFilters, checks: &mut Vec<String>) {
    if let Some(target) = &filters.target_branch {
        let pattern = shell_escape(&target.pattern);
        checks.push(format!(
            concat!(
                "  # Target branch filter\n",
                "  TARGET_BRANCH=\"$(System.PullRequest.TargetBranch)\"\n",
                "  if echo \"$TARGET_BRANCH\" | grep -qE '{}'; then\n",
                "    echo \"Filter: target-branch | Pattern: {} | Result: PASS\"\n",
                "  else\n",
                "    echo \"##[warning]PR filter target-branch did not match (pattern: {})\"\n",
                "    echo \"##vso[build.addbuildtag]pr-gate:target-branch-mismatch\"\n",
                "    SHOULD_RUN=false\n",
                "  fi",
            ),
            pattern, pattern, pattern,
        ));
    }
}

// ─── Tier 2 filter generators (REST API) ────────────────────────────────────

/// Generate the REST API preamble that fetches PR metadata.
/// Only emitted when Tier 2 filters are configured.
fn generate_api_preamble(checks: &mut Vec<String>) {
    checks.push(
        concat!(
            "  # Fetch PR metadata via REST API (Tier 2 filters)\n",
            "  PR_ID=\"$(System.PullRequest.PullRequestId)\"\n",
            "  ORG_URL=\"$(System.CollectionUri)\"\n",
            "  PROJECT=\"$(System.TeamProject)\"\n",
            "  REPO_ID=\"$(Build.Repository.ID)\"\n",
            "  PR_DATA=$(curl -s \\\n",
            "    -H \"Authorization: Bearer $SYSTEM_ACCESSTOKEN\" \\\n",
            "    \"${ORG_URL}${PROJECT}/_apis/git/repositories/${REPO_ID}/pullRequests/${PR_ID}?api-version=7.1\")\n",
            "  if [ -z \"$PR_DATA\" ] || echo \"$PR_DATA\" | python3 -c \"import sys,json; json.load(sys.stdin)\" 2>/dev/null; [ $? -ne 0 ] 2>/dev/null; then\n",
            "    echo \"##[warning]Failed to fetch PR data from API — skipping API-based filters\"\n",
            "  fi",
        )
        .to_string(),
    );
}

fn generate_labels_check(filters: &PrFilters, checks: &mut Vec<String>) {
    let Some(labels) = &filters.labels else {
        return;
    };

    // Extract labels from PR_DATA
    checks.push(
        "  # Extract PR labels\n  PR_LABELS=$(echo \"$PR_DATA\" | python3 -c \"import sys,json; data=json.load(sys.stdin); print(' '.join(l.get('name','') for l in data.get('labels',[])))\" 2>/dev/null || echo '')\n  echo \"PR labels: $PR_LABELS\""
            .to_string(),
    );

    if !labels.any_of.is_empty() {
        let label_list: Vec<String> = labels.any_of.iter().map(|l| shell_escape(l)).collect();
        let labels_str = label_list.join(" ");
        checks.push(format!(
            concat!(
                "  # Labels any-of filter\n",
                "  LABEL_MATCH=false\n",
                "  for REQUIRED_LABEL in {}; do\n",
                "    if echo \"$PR_LABELS\" | grep -qiw \"$REQUIRED_LABEL\"; then\n",
                "      LABEL_MATCH=true\n",
                "      break\n",
                "    fi\n",
                "  done\n",
                "  if [ \"$LABEL_MATCH\" = \"true\" ]; then\n",
                "    echo \"Filter: labels any-of | Result: PASS\"\n",
                "  else\n",
                "    echo \"##[warning]PR filter labels any-of did not match (required one of: {})\"\n",
                "    echo \"##vso[build.addbuildtag]pr-gate:labels-mismatch\"\n",
                "    SHOULD_RUN=false\n",
                "  fi",
            ),
            labels_str, labels_str,
        ));
    }

    if !labels.all_of.is_empty() {
        let label_list: Vec<String> = labels.all_of.iter().map(|l| shell_escape(l)).collect();
        let labels_str = label_list.join(" ");
        checks.push(format!(
            concat!(
                "  # Labels all-of filter\n",
                "  ALL_LABELS_MATCH=true\n",
                "  for REQUIRED_LABEL in {}; do\n",
                "    if ! echo \"$PR_LABELS\" | grep -qiw \"$REQUIRED_LABEL\"; then\n",
                "      ALL_LABELS_MATCH=false\n",
                "      break\n",
                "    fi\n",
                "  done\n",
                "  if [ \"$ALL_LABELS_MATCH\" = \"true\" ]; then\n",
                "    echo \"Filter: labels all-of | Result: PASS\"\n",
                "  else\n",
                "    echo \"##[warning]PR filter labels all-of did not match (required all of: {})\"\n",
                "    echo \"##vso[build.addbuildtag]pr-gate:labels-mismatch\"\n",
                "    SHOULD_RUN=false\n",
                "  fi",
            ),
            labels_str, labels_str,
        ));
    }

    if !labels.none_of.is_empty() {
        let label_list: Vec<String> = labels.none_of.iter().map(|l| shell_escape(l)).collect();
        let labels_str = label_list.join(" ");
        checks.push(format!(
            concat!(
                "  # Labels none-of filter\n",
                "  BLOCKED_LABEL_FOUND=false\n",
                "  for BLOCKED_LABEL in {}; do\n",
                "    if echo \"$PR_LABELS\" | grep -qiw \"$BLOCKED_LABEL\"; then\n",
                "      BLOCKED_LABEL_FOUND=true\n",
                "      break\n",
                "    fi\n",
                "  done\n",
                "  if [ \"$BLOCKED_LABEL_FOUND\" = \"false\" ]; then\n",
                "    echo \"Filter: labels none-of | Result: PASS\"\n",
                "  else\n",
                "    echo \"##[warning]PR filter labels none-of matched a blocked label (blocked: {})\"\n",
                "    echo \"##vso[build.addbuildtag]pr-gate:labels-mismatch\"\n",
                "    SHOULD_RUN=false\n",
                "  fi",
            ),
            labels_str, labels_str,
        ));
    }
}

fn generate_draft_check(filters: &PrFilters, checks: &mut Vec<String>) {
    let Some(draft_filter) = filters.draft else {
        return;
    };

    let expected = if draft_filter { "true" } else { "false" };
    checks.push(format!(
        concat!(
            "  # Draft filter\n",
            "  IS_DRAFT=$(echo \"$PR_DATA\" | python3 -c \"import sys,json; print(str(json.load(sys.stdin).get('isDraft',False)).lower())\" 2>/dev/null || echo 'unknown')\n",
            "  if [ \"$IS_DRAFT\" = \"{}\" ]; then\n",
            "    echo \"Filter: draft | Expected: {} | Actual: $IS_DRAFT | Result: PASS\"\n",
            "  else\n",
            "    echo \"##[warning]PR filter draft did not match (expected: {}, actual: $IS_DRAFT)\"\n",
            "    echo \"##vso[build.addbuildtag]pr-gate:draft-mismatch\"\n",
            "    SHOULD_RUN=false\n",
            "  fi",
        ),
        expected, expected, expected,
    ));
}

fn generate_changed_files_check(filters: &PrFilters, checks: &mut Vec<String>) {
    let Some(changed_files) = &filters.changed_files else {
        return;
    };

    // Fetch changed files via iterations API
    checks.push(
        concat!(
            "  # Fetch changed files via PR iterations API\n",
            "  ITERATIONS=$(curl -s \\\n",
            "    -H \"Authorization: Bearer $SYSTEM_ACCESSTOKEN\" \\\n",
            "    \"${ORG_URL}${PROJECT}/_apis/git/repositories/${REPO_ID}/pullRequests/${PR_ID}/iterations?api-version=7.1\")\n",
            "  LAST_ITER=$(echo \"$ITERATIONS\" | python3 -c \"import sys,json; iters=json.load(sys.stdin).get('value',[]); print(iters[-1]['id'] if iters else '')\" 2>/dev/null || echo '')\n",
            "  if [ -n \"$LAST_ITER\" ]; then\n",
            "    CHANGES=$(curl -s \\\n",
            "      -H \"Authorization: Bearer $SYSTEM_ACCESSTOKEN\" \\\n",
            "      \"${ORG_URL}${PROJECT}/_apis/git/repositories/${REPO_ID}/pullRequests/${PR_ID}/iterations/${LAST_ITER}/changes?api-version=7.1\")\n",
            "    CHANGED_FILES=$(echo \"$CHANGES\" | python3 -c \"\n",
            "import sys, json\n",
            "data = json.load(sys.stdin)\n",
            "for entry in data.get('changeEntries', []):\n",
            "    item = entry.get('item', {})\n",
            "    path = item.get('path', '')\n",
            "    if path:\n",
            "        print(path.lstrip('/'))\n",
            "\" 2>/dev/null || echo '')\n",
            "  else\n",
            "    CHANGED_FILES=''\n",
            "    echo \"##[warning]Could not determine PR iterations for changed-files filter\"\n",
            "  fi\n",
            "  echo \"Changed files: $(echo \"$CHANGED_FILES\" | head -20)\"",
        )
        .to_string(),
    );

    // Build the python3 fnmatch check
    let mut include_patterns = Vec::new();
    for p in &changed_files.include {
        include_patterns.push(format!("\"{}\"", shell_escape(p)));
    }
    let mut exclude_patterns = Vec::new();
    for p in &changed_files.exclude {
        exclude_patterns.push(format!("\"{}\"", shell_escape(p)));
    }

    let include_list = if include_patterns.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", include_patterns.join(", "))
    };
    let exclude_list = if exclude_patterns.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", exclude_patterns.join(", "))
    };

    checks.push(format!(
        concat!(
            "  # Changed files filter\n",
            "  FILES_MATCH=$(echo \"$CHANGED_FILES\" | python3 -c \"\n",
            "import sys, fnmatch\n",
            "includes = {}\n",
            "excludes = {}\n",
            "files = [l.strip() for l in sys.stdin if l.strip()]\n",
            "matched = []\n",
            "for f in files:\n",
            "    inc = not includes or any(fnmatch.fnmatch(f, p) for p in includes)\n",
            "    exc = any(fnmatch.fnmatch(f, p) for p in excludes)\n",
            "    if inc and not exc:\n",
            "        matched.append(f)\n",
            "print('true' if matched else 'false')\n",
            "\" 2>/dev/null || echo 'true')\n",
            "  if [ \"$FILES_MATCH\" = \"true\" ]; then\n",
            "    echo \"Filter: changed-files | Result: PASS\"\n",
            "  else\n",
            "    echo \"##[warning]PR filter changed-files did not match any relevant files\"\n",
            "    echo \"##vso[build.addbuildtag]pr-gate:changed-files-mismatch\"\n",
            "    SHOULD_RUN=false\n",
            "  fi",
        ),
        include_list, exclude_list,
    ));
}

// ─── Tier 3 filter generators (advanced) ────────────────────────────────────

fn generate_time_window_check(filters: &PrFilters, checks: &mut Vec<String>) {
    let Some(window) = &filters.time_window else {
        return;
    };

    let start = shell_escape(&window.start);
    let end = shell_escape(&window.end);

    checks.push(format!(
        concat!(
            "  # Time window filter\n",
            "  CURRENT_HOUR=$(date -u +%H)\n",
            "  CURRENT_MIN=$(date -u +%M)\n",
            "  CURRENT_MINUTES=$((CURRENT_HOUR * 60 + CURRENT_MIN))\n",
            "  START_H=${{{}%%:*}}\n",
            "  START_M=${{{}##*:}}\n",
            "  START_MINUTES=$((10#$START_H * 60 + 10#$START_M))\n",
            "  END_H=${{{}%%:*}}\n",
            "  END_M=${{{}##*:}}\n",
            "  END_MINUTES=$((10#$END_H * 60 + 10#$END_M))\n",
            "  if [ $START_MINUTES -le $END_MINUTES ]; then\n",
            "    # Same-day window\n",
            "    if [ $CURRENT_MINUTES -ge $START_MINUTES ] && [ $CURRENT_MINUTES -lt $END_MINUTES ]; then\n",
            "      IN_WINDOW=true\n",
            "    else\n",
            "      IN_WINDOW=false\n",
            "    fi\n",
            "  else\n",
            "    # Overnight window (e.g., 22:00-06:00)\n",
            "    if [ $CURRENT_MINUTES -ge $START_MINUTES ] || [ $CURRENT_MINUTES -lt $END_MINUTES ]; then\n",
            "      IN_WINDOW=true\n",
            "    else\n",
            "      IN_WINDOW=false\n",
            "    fi\n",
            "  fi\n",
            "  if [ \"$IN_WINDOW\" = \"true\" ]; then\n",
            "    echo \"Filter: time-window | Window: {}-{} UTC | Result: PASS\"\n",
            "  else\n",
            "    echo \"##[warning]PR filter time-window: current time is outside {}-{} UTC\"\n",
            "    echo \"##vso[build.addbuildtag]pr-gate:time-window-mismatch\"\n",
            "    SHOULD_RUN=false\n",
            "  fi",
        ),
        // Shell parameter expansion for start/end parsing
        start, start, end, end,
        // Diagnostic messages
        start, end, start, end,
    ));
}

fn generate_change_count_check(filters: &PrFilters, checks: &mut Vec<String>) {
    let has_min = filters.min_changes.is_some();
    let has_max = filters.max_changes.is_some();
    if !has_min && !has_max {
        return;
    }

    // Ensure we have CHANGED_FILES available (from changed-files filter or fresh fetch)
    if filters.changed_files.is_none() {
        // Need to fetch changed files count if not already fetched by changed-files filter
        if !has_tier2_filters(filters) {
            checks.push(
                concat!(
                    "  # Fetch PR change count (for min/max-changes)\n",
                    "  PR_ID=\"$(System.PullRequest.PullRequestId)\"\n",
                    "  ORG_URL=\"$(System.CollectionUri)\"\n",
                    "  PROJECT=\"$(System.TeamProject)\"\n",
                    "  REPO_ID=\"$(Build.Repository.ID)\"",
                )
                .to_string(),
            );
        }
        checks.push(
            concat!(
                "  # Count changed files via iterations API\n",
                "  if [ -z \"${LAST_ITER:-}\" ]; then\n",
                "    ITERATIONS=$(curl -s \\\n",
                "      -H \"Authorization: Bearer $SYSTEM_ACCESSTOKEN\" \\\n",
                "      \"${ORG_URL}${PROJECT}/_apis/git/repositories/${REPO_ID}/pullRequests/${PR_ID}/iterations?api-version=7.1\")\n",
                "    LAST_ITER=$(echo \"$ITERATIONS\" | python3 -c \"import sys,json; iters=json.load(sys.stdin).get('value',[]); print(iters[-1]['id'] if iters else '')\" 2>/dev/null || echo '')\n",
                "  fi\n",
                "  if [ -n \"$LAST_ITER\" ]; then\n",
                "    CHANGES_RESP=$(curl -s \\\n",
                "      -H \"Authorization: Bearer $SYSTEM_ACCESSTOKEN\" \\\n",
                "      \"${ORG_URL}${PROJECT}/_apis/git/repositories/${REPO_ID}/pullRequests/${PR_ID}/iterations/${LAST_ITER}/changes?api-version=7.1\")\n",
                "    FILE_COUNT=$(echo \"$CHANGES_RESP\" | python3 -c \"import sys,json; print(len(json.load(sys.stdin).get('changeEntries',[])))\" 2>/dev/null || echo '0')\n",
                "  else\n",
                "    FILE_COUNT=0\n",
                "  fi\n",
                "  echo \"Changed file count: $FILE_COUNT\"",
            )
            .to_string(),
        );
    } else {
        // CHANGED_FILES already available from changed-files filter
        checks.push(
            "  # Count changed files (from changed-files data)\n  FILE_COUNT=$(echo \"$CHANGED_FILES\" | grep -c . || echo '0')\n  echo \"Changed file count: $FILE_COUNT\""
                .to_string(),
        );
    }

    if let Some(min) = filters.min_changes {
        checks.push(format!(
            concat!(
                "  # Min changes filter\n",
                "  if [ \"$FILE_COUNT\" -ge {} ]; then\n",
                "    echo \"Filter: min-changes | Min: {} | Actual: $FILE_COUNT | Result: PASS\"\n",
                "  else\n",
                "    echo \"##[warning]PR filter min-changes: $FILE_COUNT files changed, minimum {} required\"\n",
                "    echo \"##vso[build.addbuildtag]pr-gate:min-changes-mismatch\"\n",
                "    SHOULD_RUN=false\n",
                "  fi",
            ),
            min, min, min,
        ));
    }

    if let Some(max) = filters.max_changes {
        checks.push(format!(
            concat!(
                "  # Max changes filter\n",
                "  if [ \"$FILE_COUNT\" -le {} ]; then\n",
                "    echo \"Filter: max-changes | Max: {} | Actual: $FILE_COUNT | Result: PASS\"\n",
                "  else\n",
                "    echo \"##[warning]PR filter max-changes: $FILE_COUNT files changed, maximum {} allowed\"\n",
                "    echo \"##vso[build.addbuildtag]pr-gate:max-changes-mismatch\"\n",
                "    SHOULD_RUN=false\n",
                "  fi",
            ),
            max, max, max,
        ));
    }
}

fn generate_build_reason_check(filters: &PrFilters, checks: &mut Vec<String>) {
    let Some(build_reason) = &filters.build_reason else {
        return;
    };

    let mut reason_check = String::from("  # Build reason filter\n  REASON=\"$(Build.Reason)\"\n");

    if !build_reason.include.is_empty() {
        let reasons: Vec<String> = build_reason.include.iter().map(|r| shell_escape(r)).collect();
        let pattern = reasons.join("|");
        reason_check.push_str(&format!(
            concat!(
                "  if echo \"$REASON\" | grep -qiE '^({})$'; then\n",
                "    echo \"Filter: build-reason include | Result: PASS\"\n",
                "  else\n",
                "    echo \"##[warning]PR filter build-reason: $REASON not in include list\"\n",
                "    echo \"##vso[build.addbuildtag]pr-gate:build-reason-mismatch\"\n",
                "    SHOULD_RUN=false\n",
                "  fi",
            ),
            pattern,
        ));
    }

    if !build_reason.exclude.is_empty() {
        let reasons: Vec<String> = build_reason.exclude.iter().map(|r| shell_escape(r)).collect();
        let pattern = reasons.join("|");
        reason_check.push_str(&format!(
            concat!(
                "\n  if echo \"$REASON\" | grep -qiE '^({})$'; then\n",
                "    echo \"##[warning]PR filter build-reason: $REASON in exclude list\"\n",
                "    echo \"##vso[build.addbuildtag]pr-gate:build-reason-excluded\"\n",
                "    SHOULD_RUN=false\n",
                "  else\n",
                "    echo \"Filter: build-reason exclude | Result: PASS\"\n",
                "  fi",
            ),
            pattern,
        ));
    }

    checks.push(reason_check);
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Shell-escape a string for use in a bash script.
/// Prevents shell injection from filter pattern values.
pub(super) fn shell_escape(s: &str) -> String {
    s.chars()
        .filter(|c| {
            c.is_alphanumeric()
                || matches!(
                    c,
                    '.' | '*'
                        | '+'
                        | '?'
                        | '^'
                        | '$'
                        | '|'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | '\\'
                        | '-'
                        | '_'
                        | '/'
                        | '@'
                        | ' '
                        | ':'
                )
        })
        .collect()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::common::{generate_agentic_depends_on, generate_pr_trigger, generate_setup_job};
    use crate::compile::types::*;

    #[test]
    fn test_generate_pr_trigger_with_explicit_pr_trigger_overrides_schedule() {
        let triggers = Some(TriggerConfig {
            pipeline: None,
            pr: Some(PrTriggerConfig::default()),
        });
        let result = generate_pr_trigger(&triggers, true);
        assert!(!result.contains("pr: none"), "triggers.pr should override schedule suppression");
    }

    #[test]
    fn test_generate_pr_trigger_with_pr_trigger_and_pipeline_trigger() {
        let triggers = Some(TriggerConfig {
            pipeline: Some(PipelineTrigger {
                name: "Build".into(),
                project: None,
                branches: vec![],
            }),
            pr: Some(PrTriggerConfig::default()),
        });
        let result = generate_pr_trigger(&triggers, false);
        assert!(!result.contains("pr: none"), "triggers.pr should override pipeline trigger suppression");
    }

    #[test]
    fn test_generate_pr_trigger_with_branches() {
        let triggers = Some(TriggerConfig {
            pipeline: None,
            pr: Some(PrTriggerConfig {
                branches: Some(BranchFilter {
                    include: vec!["main".into(), "release/*".into()],
                    exclude: vec!["test/*".into()],
                }),
                paths: None,
                filters: None,
            }),
        });
        let result = generate_pr_trigger(&triggers, false);
        assert!(result.contains("pr:"), "should emit pr: block");
        assert!(result.contains("branches:"), "should include branches");
        assert!(result.contains("main"), "should include main branch");
        assert!(result.contains("release/*"), "should include release/* branch");
        assert!(result.contains("exclude:"), "should include exclude");
        assert!(result.contains("test/*"), "should include test/* exclusion");
    }

    #[test]
    fn test_generate_pr_trigger_with_paths() {
        let triggers = Some(TriggerConfig {
            pipeline: None,
            pr: Some(PrTriggerConfig {
                branches: None,
                paths: Some(PathFilter {
                    include: vec!["src/*".into()],
                    exclude: vec!["docs/*".into()],
                }),
                filters: None,
            }),
        });
        let result = generate_pr_trigger(&triggers, false);
        assert!(result.contains("pr:"), "should emit pr: block");
        assert!(result.contains("paths:"), "should include paths");
        assert!(result.contains("src/*"), "should include src/* path");
        assert!(result.contains("docs/*"), "should include docs/* exclusion");
    }

    #[test]
    fn test_generate_pr_trigger_with_filters_only_no_pr_block() {
        let triggers = Some(TriggerConfig {
            pipeline: None,
            pr: Some(PrTriggerConfig {
                branches: None,
                paths: None,
                filters: Some(PrFilters {
                    title: Some(PatternFilter { pattern: "\\[agent\\]".into() }),
                    ..Default::default()
                }),
            }),
        });
        let result = generate_pr_trigger(&triggers, false);
        assert!(result.is_empty(), "filters-only should not emit a pr: block (use default trigger)");
    }

    #[test]
    fn test_generate_setup_job_with_pr_filters_creates_gate() {
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "\\[review\\]".into() }),
            ..Default::default()
        };
        let result = generate_setup_job(&[], "MyPool", Some(&filters));
        assert!(result.contains("- job: Setup"), "should create Setup job");
        assert!(result.contains("name: prGate"), "should include gate step");
        assert!(result.contains("Evaluate PR filters"), "should have gate displayName");
        assert!(result.contains("SHOULD_RUN"), "should set SHOULD_RUN variable");
        assert!(result.contains("\\[review\\]"), "should include title pattern");
        assert!(result.contains("SYSTEM_ACCESSTOKEN"), "should pass System.AccessToken");
        assert!(result.contains("cancelling"), "should include self-cancel API call");
    }

    #[test]
    fn test_generate_setup_job_with_filters_and_user_steps() {
        let step: serde_yaml::Value = serde_yaml::from_str("bash: echo hello\ndisplayName: User step").unwrap();
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "test".into() }),
            ..Default::default()
        };
        let result = generate_setup_job(&[step], "MyPool", Some(&filters));
        assert!(result.contains("name: prGate"), "should include gate step");
        assert!(result.contains("User step"), "should include user step");
        assert!(result.contains("prGate.SHOULD_RUN"), "user steps should reference gate output");
    }

    #[test]
    fn test_generate_setup_job_without_filters_unchanged() {
        let result = generate_setup_job(&[], "MyPool", None);
        assert!(result.is_empty(), "no setup steps and no filters should produce empty string");
    }

    #[test]
    fn test_generate_agentic_depends_on_with_pr_filters() {
        let result = generate_agentic_depends_on(&[], true, None);
        assert!(result.contains("dependsOn: Setup"), "should depend on Setup");
        assert!(result.contains("condition:"), "should have condition");
        assert!(result.contains("Build.Reason"), "should check Build.Reason");
        assert!(result.contains("prGate.SHOULD_RUN"), "should check gate output");
    }

    #[test]
    fn test_generate_agentic_depends_on_setup_only_no_condition() {
        let step: serde_yaml::Value = serde_yaml::from_str("bash: echo hello").unwrap();
        let result = generate_agentic_depends_on(&[step], false, None);
        assert_eq!(result, "dependsOn: Setup");
        assert!(!result.contains("condition:"), "no condition without PR filters");
    }

    #[test]
    fn test_generate_agentic_depends_on_nothing() {
        let result = generate_agentic_depends_on(&[], false, None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_generate_setup_job_gate_author_filter() {
        let filters = PrFilters {
            author: Some(IncludeExcludeFilter {
                include: vec!["alice@corp.com".into()],
                exclude: vec!["bot@noreply.com".into()],
            }),
            ..Default::default()
        };
        let result = generate_setup_job(&[], "MyPool", Some(&filters));
        assert!(result.contains("alice@corp.com"), "should include author email");
        assert!(result.contains("bot@noreply.com"), "should include excluded email");
        assert!(result.contains("Build.RequestedForEmail"), "should check author variable");
    }

    #[test]
    fn test_generate_setup_job_gate_branch_filters() {
        let filters = PrFilters {
            source_branch: Some(PatternFilter { pattern: "^feature/.*".into() }),
            target_branch: Some(PatternFilter { pattern: "^main$".into() }),
            ..Default::default()
        };
        let result = generate_setup_job(&[], "MyPool", Some(&filters));
        assert!(result.contains("SourceBranch"), "should check source branch");
        assert!(result.contains("TargetBranch"), "should check target branch");
        assert!(result.contains("^feature/.*"), "should include source pattern");
        assert!(result.contains("^main$"), "should include target pattern");
    }

    #[test]
    fn test_generate_setup_job_gate_non_pr_passthrough() {
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "test".into() }),
            ..Default::default()
        };
        let result = generate_setup_job(&[], "MyPool", Some(&filters));
        assert!(result.contains("PullRequest"), "should check for PR build reason");
        assert!(result.contains("Not a PR build"), "should pass non-PR builds automatically");
    }

    #[test]
    fn test_generate_setup_job_gate_build_tags() {
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "test".into() }),
            ..Default::default()
        };
        let result = generate_setup_job(&[], "MyPool", Some(&filters));
        assert!(result.contains("pr-gate:passed"), "should tag passed builds");
        assert!(result.contains("pr-gate:skipped"), "should tag skipped builds");
        assert!(result.contains("pr-gate:title-mismatch"), "should tag specific filter failures");
    }

    #[test]
    fn test_shell_escape_removes_dangerous_chars() {
        assert_eq!(shell_escape("safe-pattern_123"), "safe-pattern_123");
        assert_eq!(shell_escape("test;echo pwned"), "testecho pwned");
        assert_eq!(shell_escape("test`echo`"), "testecho");
        assert_eq!(shell_escape("^feature/.*$"), "^feature/.*$");
        assert_eq!(shell_escape("\\[agent\\]"), "\\[agent\\]");
        assert_eq!(shell_escape("(a|b)"), "(a|b)");
    }

    // ─── Tier 2 filter tests ────────────────────────────────────────────────

    #[test]
    fn test_has_tier2_filters_none() {
        let filters = PrFilters::default();
        assert!(!has_tier2_filters(&filters));
    }

    #[test]
    fn test_has_tier2_filters_labels() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(has_tier2_filters(&filters));
    }

    #[test]
    fn test_has_tier2_filters_draft() {
        let filters = PrFilters {
            draft: Some(false),
            ..Default::default()
        };
        assert!(has_tier2_filters(&filters));
    }

    #[test]
    fn test_has_tier2_filters_changed_files() {
        let filters = PrFilters {
            changed_files: Some(IncludeExcludeFilter {
                include: vec!["src/**".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(has_tier2_filters(&filters));
    }

    #[test]
    fn test_gate_step_includes_api_call_for_tier2() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("pullRequests"), "should include API call for labels filter");
        assert!(result.contains("PR_DATA"), "should store API response");
    }

    #[test]
    fn test_gate_step_no_api_call_for_tier1_only() {
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "test".into() }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(!result.contains("PR_DATA"), "should not make API call for title-only filter");
    }

    #[test]
    fn test_gate_step_labels_any_of() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into(), "needs-review".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("run-agent"), "should check for run-agent label");
        assert!(result.contains("needs-review"), "should check for needs-review label");
        assert!(result.contains("LABEL_MATCH"), "should use any-of matching");
    }

    #[test]
    fn test_gate_step_labels_none_of() {
        let filters = PrFilters {
            labels: Some(LabelFilter {
                none_of: vec!["do-not-run".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("do-not-run"), "should check for blocked label");
        assert!(result.contains("BLOCKED_LABEL"), "should use none-of matching");
    }

    #[test]
    fn test_gate_step_draft_false() {
        let filters = PrFilters {
            draft: Some(false),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("isDraft"), "should check isDraft field");
        assert!(result.contains("false"), "should expect draft=false");
    }

    #[test]
    fn test_gate_step_changed_files() {
        let filters = PrFilters {
            changed_files: Some(IncludeExcludeFilter {
                include: vec!["src/**/*.rs".into()],
                exclude: vec!["docs/**".into()],
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("iterations"), "should fetch iteration changes");
        assert!(result.contains("fnmatch"), "should use fnmatch for glob matching");
        assert!(result.contains("src/**/*.rs"), "should include the include pattern");
        assert!(result.contains("docs/**"), "should include the exclude pattern");
    }

    #[test]
    fn test_gate_step_combined_tier1_and_tier2() {
        let filters = PrFilters {
            title: Some(PatternFilter { pattern: "\\[review\\]".into() }),
            draft: Some(false),
            labels: Some(LabelFilter {
                any_of: vec!["run-agent".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        // Tier 1
        assert!(result.contains("System.PullRequest.Title"), "should check title");
        // Tier 2
        assert!(result.contains("PR_DATA"), "should make API call");
        assert!(result.contains("isDraft"), "should check draft");
        assert!(result.contains("run-agent"), "should check labels");
    }

    // ─── Tier 3 filter tests ────────────────────────────────────────────────

    #[test]
    fn test_gate_step_time_window() {
        let filters = PrFilters {
            time_window: Some(super::super::types::TimeWindowFilter {
                start: "09:00".into(),
                end: "17:00".into(),
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("CURRENT_HOUR"), "should get current UTC hour");
        assert!(result.contains("09:00"), "should include start time");
        assert!(result.contains("17:00"), "should include end time");
        assert!(result.contains("IN_WINDOW"), "should evaluate time window");
        assert!(result.contains("pr-gate:time-window-mismatch"), "should tag time-window failures");
    }

    #[test]
    fn test_gate_step_min_changes() {
        let filters = PrFilters {
            min_changes: Some(5),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("FILE_COUNT"), "should count changed files");
        assert!(result.contains("-ge 5"), "should check minimum 5 files");
        assert!(result.contains("pr-gate:min-changes-mismatch"), "should tag min-changes failures");
    }

    #[test]
    fn test_gate_step_max_changes() {
        let filters = PrFilters {
            max_changes: Some(50),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("FILE_COUNT"), "should count changed files");
        assert!(result.contains("-le 50"), "should check maximum 50 files");
        assert!(result.contains("pr-gate:max-changes-mismatch"), "should tag max-changes failures");
    }

    #[test]
    fn test_gate_step_min_and_max_changes() {
        let filters = PrFilters {
            min_changes: Some(2),
            max_changes: Some(100),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("-ge 2"), "should check min");
        assert!(result.contains("-le 100"), "should check max");
    }

    #[test]
    fn test_gate_step_build_reason_include() {
        let filters = PrFilters {
            build_reason: Some(IncludeExcludeFilter {
                include: vec!["PullRequest".into(), "Manual".into()],
                exclude: vec![],
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("Build.Reason"), "should check build reason");
        assert!(result.contains("PullRequest"), "should include PullRequest");
        assert!(result.contains("Manual"), "should include Manual");
        assert!(result.contains("pr-gate:build-reason-mismatch"), "should tag build-reason failures");
    }

    #[test]
    fn test_gate_step_build_reason_exclude() {
        let filters = PrFilters {
            build_reason: Some(IncludeExcludeFilter {
                include: vec![],
                exclude: vec!["Schedule".into()],
            }),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        assert!(result.contains("Schedule"), "should check excluded reason");
        assert!(result.contains("pr-gate:build-reason-excluded"), "should tag excluded builds");
    }

    #[test]
    fn test_agentic_depends_on_with_expression() {
        let result = generate_agentic_depends_on(
            &[],
            false,
            Some("eq(variables['Custom.ShouldRun'], 'true')"),
        );
        assert!(result.contains("condition:"), "should have condition");
        assert!(result.contains("Custom.ShouldRun"), "should include expression");
        assert!(result.contains("succeeded()"), "should still require succeeded");
    }

    #[test]
    fn test_agentic_depends_on_with_pr_filters_and_expression() {
        let result = generate_agentic_depends_on(
            &[],
            true,
            Some("eq(variables['Custom.Flag'], 'yes')"),
        );
        assert!(result.contains("prGate.SHOULD_RUN"), "should check gate output");
        assert!(result.contains("Custom.Flag"), "should include expression");
        assert!(result.contains("Build.Reason"), "should check build reason");
    }

    #[test]
    fn test_agentic_depends_on_expression_only_no_depends() {
        let result = generate_agentic_depends_on(
            &[],
            false,
            Some("eq(variables['Run'], 'true')"),
        );
        // No setup steps, no PR filters — no dependsOn, but still a condition
        assert!(!result.contains("dependsOn"), "no dependsOn without setup/filters");
        assert!(result.contains("condition:"), "should have condition from expression");
    }

    #[test]
    fn test_gate_step_change_count_reuses_changed_files_data() {
        let filters = PrFilters {
            changed_files: Some(IncludeExcludeFilter {
                include: vec!["src/**".into()],
                ..Default::default()
            }),
            min_changes: Some(3),
            ..Default::default()
        };
        let result = generate_pr_gate_step(&filters);
        // Should use CHANGED_FILES from the changed-files filter, not make a new API call
        assert!(result.contains("grep -c ."), "should count from existing CHANGED_FILES");
    }

    #[test]
    fn test_pr_trigger_type_deserialization_tier3() {
        let yaml = r#"
triggers:
  pr:
    filters:
      time-window:
        start: "09:00"
        end: "17:00"
      min-changes: 5
      max-changes: 100
      build-reason:
        include: [PullRequest, Manual]
      expression: "eq(variables['Custom.Flag'], 'true')"
"#;
        let val: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let tc: TriggerConfig = serde_yaml::from_value(val["triggers"].clone()).unwrap();
        let filters = tc.pr.unwrap().filters.unwrap();
        assert_eq!(filters.time_window.as_ref().unwrap().start, "09:00");
        assert_eq!(filters.time_window.as_ref().unwrap().end, "17:00");
        assert_eq!(filters.min_changes, Some(5));
        assert_eq!(filters.max_changes, Some(100));
        assert_eq!(filters.build_reason.as_ref().unwrap().include, vec!["PullRequest", "Manual"]);
        assert_eq!(filters.expression.as_ref().unwrap(), "eq(variables['Custom.Flag'], 'true')");
    }
}

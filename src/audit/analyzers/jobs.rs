//! ADO build-timeline analyzer for `ado-aw audit`.

use anyhow::Context;
use chrono::DateTime;
use log::debug;
use serde_json::Value;
use std::cmp::Ordering;

use crate::ado::{AdoAuth, AdoContext, PATH_SEGMENT};
use crate::audit::model::JobData;

/// Fetch the build timeline JSON from ADO.
pub async fn fetch_timeline(
    client: &reqwest::Client,
    ctx: &AdoContext,
    auth: &AdoAuth,
    build_id: u64,
) -> anyhow::Result<Value> {
    let url = format!(
        "{}/{}/_apis/build/builds/{}/timeline?api-version=7.1",
        ctx.org_url.trim_end_matches('/'),
        percent_encoding::utf8_percent_encode(&ctx.project, PATH_SEGMENT),
        build_id
    );

    debug!("GET build {} timeline: {}", build_id, url);

    let resp = auth
        .apply(client.get(&url))
        .send()
        .await
        .with_context(|| format!("Failed to fetch build {} timeline", build_id))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "ADO API returned {} when fetching build {} timeline: {}",
            status,
            build_id,
            body
        );
    }

    resp.json()
        .await
        .with_context(|| format!("Failed to parse build {} timeline response", build_id))
}

/// Map a timeline JSON `value` into a sorted `Vec<JobData>` for the
/// audit report. Filters to records of `type: "Job"` (skips stages
/// and tasks). Sorts by `startTime` ascending; records with no start
/// time go last.
pub fn timeline_to_jobs(timeline: &Value) -> Vec<JobData> {
    let Some(records) = timeline.get("records").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut jobs: Vec<JobData> = records.iter().filter_map(record_to_job).collect();
    jobs.sort_by(compare_jobs_by_start_time);
    jobs
}

fn record_to_job(record: &Value) -> Option<JobData> {
    let record_type = string_field(record, "type")?;
    if !record_type.eq_ignore_ascii_case("job") {
        return None;
    }

    let status = string_field(record, "state").unwrap_or_default();
    let started_at = string_field(record, "startTime");
    let finished_at = string_field(record, "finishTime");

    Some(JobData {
        name: string_field(record, "name").unwrap_or_default(),
        result: if status.eq_ignore_ascii_case("completed") {
            string_field(record, "result")
        } else {
            None
        },
        duration: format_duration(started_at.as_deref(), finished_at.as_deref()),
        started_at,
        finished_at,
        status,
    })
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn format_duration(started_at: Option<&str>, finished_at: Option<&str>) -> Option<String> {
    let start = DateTime::parse_from_rfc3339(started_at?).ok()?;
    let finish = DateTime::parse_from_rfc3339(finished_at?).ok()?;
    let delta = finish.signed_duration_since(start);
    if delta.num_seconds() < 0 {
        return None;
    }

    let total_seconds = delta.num_seconds();
    Some(format!("{}m {}s", total_seconds / 60, total_seconds % 60))
}

fn compare_jobs_by_start_time(left: &JobData, right: &JobData) -> Ordering {
    match (left.started_at.as_deref(), right.started_at.as_deref()) {
        (Some(left_start), Some(right_start)) => compare_timestamp_strings(left_start, right_start)
            .then_with(|| left.name.cmp(&right.name)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => left.name.cmp(&right.name),
    }
}

fn compare_timestamp_strings(left: &str, right: &str) -> Ordering {
    match (
        DateTime::parse_from_rfc3339(left),
        DateTime::parse_from_rfc3339(right),
    ) {
        (Ok(left_dt), Ok(right_dt)) => left_dt.cmp(&right_dt),
        _ => left.cmp(right),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn timeline_to_jobs_empty_input_returns_empty_vec() {
        assert!(timeline_to_jobs(&json!({})).is_empty());
        assert!(timeline_to_jobs(&json!({ "records": [] })).is_empty());
    }

    #[test]
    fn timeline_to_jobs_filters_non_job_records() {
        let timeline = json!({
            "records": [
                {
                    "name": "Build Stage",
                    "type": "Stage",
                    "state": "completed",
                    "result": "succeeded"
                },
                {
                    "name": "Agent",
                    "type": "Job",
                    "state": "completed",
                    "result": "succeeded",
                    "startTime": "2026-01-01T00:00:00Z",
                    "finishTime": "2026-01-01T00:01:00Z"
                },
                {
                    "name": "Checkout",
                    "type": "Task",
                    "state": "completed",
                    "result": "succeeded"
                }
            ]
        });

        let jobs = timeline_to_jobs(&timeline);

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "Agent");
    }

    #[test]
    fn timeline_to_jobs_computes_duration() {
        let timeline = json!({
            "records": [{
                "name": "Agent",
                "type": "Job",
                "state": "completed",
                "result": "succeeded",
                "startTime": "2026-01-01T00:00:00Z",
                "finishTime": "2026-01-01T00:01:30Z"
            }]
        });

        let jobs = timeline_to_jobs(&timeline);

        assert_eq!(jobs[0].duration.as_deref(), Some("1m 30s"));
    }

    #[test]
    fn timeline_to_jobs_omits_result_and_duration_for_unfinished_job() {
        let timeline = json!({
            "records": [{
                "name": "Detection",
                "type": "Job",
                "state": "inProgress",
                "result": "succeeded",
                "startTime": "2026-01-01T00:00:00Z"
            }]
        });

        let jobs = timeline_to_jobs(&timeline);

        assert_eq!(jobs[0].result, None);
        assert_eq!(jobs[0].duration, None);
    }

    #[test]
    fn timeline_to_jobs_sorts_by_start_time() {
        let timeline = json!({
            "records": [
                {
                    "name": "A",
                    "type": "Job",
                    "state": "completed",
                    "result": "succeeded",
                    "startTime": "2026-01-01T00:02:00Z"
                },
                {
                    "name": "B",
                    "type": "Job",
                    "state": "completed",
                    "result": "succeeded",
                    "startTime": "2026-01-01T00:01:00Z"
                }
            ]
        });

        let jobs = timeline_to_jobs(&timeline);

        assert_eq!(
            jobs.iter().map(|job| job.name.as_str()).collect::<Vec<_>>(),
            vec!["B", "A"]
        );
    }

    #[test]
    fn timeline_to_jobs_parses_real_ado_shape() {
        // Simulates a real ADO timeline response with extra fields (id, parentId)
        // that the parser should silently ignore.
        let timeline = json!({
            "records": [
                {
                    "id": "1",
                    "name": "Agent",
                    "type": "Job",
                    "state": "completed",
                    "result": "succeeded",
                    "startTime": "2026-01-01T00:00:00Z",
                    "finishTime": "2026-01-01T00:01:00Z",
                    "parentId": "stage-1"
                },
                {
                    "id": "2",
                    "name": "Detection",
                    "type": "Job",
                    "state": "completed",
                    "result": "succeeded",
                    "startTime": "2026-01-01T00:01:01Z",
                    "finishTime": "2026-01-01T00:02:00Z",
                    "parentId": "stage-2"
                },
                {
                    "id": "3",
                    "name": "SafeOutputs",
                    "type": "Job",
                    "state": "completed",
                    "result": "succeeded",
                    "startTime": "2026-01-01T00:02:01Z",
                    "finishTime": "2026-01-01T00:03:00Z",
                    "parentId": "stage-3"
                }
            ]
        });

        let jobs = timeline_to_jobs(&timeline);

        assert_eq!(jobs.len(), 3);
        assert_eq!(
            jobs.iter().map(|job| job.name.as_str()).collect::<Vec<_>>(),
            vec!["Agent", "Detection", "SafeOutputs"]
        );

        // Verify all fields are correctly parsed for the first job; extra ADO
        // fields (id, parentId) must be silently ignored.
        assert_eq!(jobs[0].status, "completed");
        assert_eq!(jobs[0].result.as_deref(), Some("succeeded"));
        assert_eq!(jobs[0].duration.as_deref(), Some("1m 0s"));
        assert_eq!(jobs[0].started_at.as_deref(), Some("2026-01-01T00:00:00Z"));
        assert_eq!(jobs[0].finished_at.as_deref(), Some("2026-01-01T00:01:00Z"));
    }
}

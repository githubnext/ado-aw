#![allow(dead_code)]

use anyhow::{Result, bail};
use percent_encoding::percent_decode_str;
use url::Url;

const ACCEPTED_FORMATS: &str = "Accepted formats:\n- Bare numeric ID: 1234567890\n- dev.azure.com URL: https://dev.azure.com/{org}/{project}/_build/results?buildId=N\n- Legacy visualstudio.com URL: https://{org}.visualstudio.com/{project}/_build/results?buildId=N\n- On-prem URL: https://{server}/{collection}/{project}/_build/results?buildId=N";

/// Parsed form of an Azure DevOps build identifier (bare ID or URL).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedBuildRef {
    /// The numeric build ID.
    pub build_id: u64,
    /// Organization name (e.g. "my-org") or full collection URL host segment.
    /// `None` if the input was a bare ID and the caller must resolve it from
    /// git remote / `--org`.
    pub org: Option<String>,
    /// Project name (URL-decoded). `None` if the input was a bare ID.
    pub project: Option<String>,
    /// Host of the ADO instance (e.g. "dev.azure.com" or an on-prem hostname).
    /// `None` if the input was a bare ID.
    pub host: Option<String>,
    /// Job timeline ID if the URL pinned a specific job (`&j=<guid>`).
    /// MVP normalizes to the parent build but the value is preserved for
    /// future job-mode audit.
    pub job_id: Option<String>,
    /// Task/step timeline ID if the URL pinned a specific step (`&t=<guid>`
    /// or `&s=<guid>`).
    pub step_id: Option<String>,
}

/// Parse any accepted form of an Azure DevOps build identifier into a
/// `ParsedBuildRef`.
///
/// Accepted shapes:
/// - Bare numeric ID: `1234567890`
/// - dev.azure.com URL: `https://dev.azure.com/{org}/{project}/_build/results?buildId=N`
///   with optional `&view=logs&j=<guid>&t=<guid>` anchors
/// - On-prem URL: `https://{server}/{collection}/{project}/_build/results?buildId=N`
///   with the same optional anchors
/// - URL-encoded project segments (e.g. `My%20Project`) are decoded
/// - Trailing slashes and case-insensitive `_build/results` match
///
/// Returns a structured error when the input is malformed (with a hint
/// listing the accepted formats).
pub fn parse_build_ref(input: &str) -> Result<ParsedBuildRef> {
    let input = input.trim();
    if input.is_empty() {
        return invalid_build_ref(input);
    }

    if input.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(ParsedBuildRef {
            build_id: input
                .parse()
                .map_err(|_| anyhow::anyhow!(invalid_build_ref_message(input)))?,
            org: None,
            project: None,
            host: None,
            job_id: None,
            step_id: None,
        });
    }

    let url = match Url::parse(input) {
        Ok(url) => url,
        Err(_) => return invalid_build_ref(input),
    };

    let host = match url.host_str() {
        Some(host) => host.to_string(),
        None => return invalid_build_ref(input),
    };
    let path_segments: Vec<&str> = match url.path_segments() {
        Some(segments) => segments.filter(|segment| !segment.is_empty()).collect(),
        None => return invalid_build_ref(input),
    };

    let (org, project) = parse_location(&host, &path_segments, input)?;
    let (build_id, job_id, step_id) = parse_query(&url, input)?;

    Ok(ParsedBuildRef {
        build_id,
        org: Some(org),
        project: Some(project),
        host: Some(host),
        job_id,
        step_id,
    })
}

fn parse_location(host: &str, path_segments: &[&str], input: &str) -> Result<(String, String)> {
    if host.eq_ignore_ascii_case("dev.azure.com") {
        if matches_build_results(path_segments, 2) {
            return Ok((
                path_segments[0].to_string(),
                decode_path_segment(path_segments[1]),
            ));
        }
        return invalid_build_ref(input);
    }

    if host.to_ascii_lowercase().ends_with(".visualstudio.com") {
        if matches_build_results(path_segments, 1) {
            let org = host
                .split('.')
                .next()
                .filter(|segment| !segment.is_empty())
                .map(str::to_string);
            if let Some(org) = org {
                return Ok((org, decode_path_segment(path_segments[0])));
            }
        }
        return invalid_build_ref(input);
    }

    if matches_build_results(path_segments, 2) {
        return Ok((
            path_segments[0].to_string(),
            decode_path_segment(path_segments[1]),
        ));
    }

    invalid_build_ref(input)
}

fn parse_query(url: &Url, input: &str) -> Result<(u64, Option<String>, Option<String>)> {
    let mut build_id = None;
    let mut job_id = None;
    let mut step_id = None;

    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "buildId" if build_id.is_none() => {
                build_id = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| anyhow::anyhow!(invalid_build_ref_message(input)))?,
                );
            }
            "j" if job_id.is_none() => job_id = Some(value.into_owned()),
            "t" | "s" if step_id.is_none() => step_id = Some(value.into_owned()),
            _ => {}
        }
    }

    match build_id {
        Some(build_id) => Ok((build_id, job_id, step_id)),
        None => invalid_build_ref(input),
    }
}

fn matches_build_results(path_segments: &[&str], prefix_len: usize) -> bool {
    path_segments.len() == prefix_len + 2
        && path_segments[prefix_len].eq_ignore_ascii_case("_build")
        && path_segments[prefix_len + 1].eq_ignore_ascii_case("results")
}

fn decode_path_segment(segment: &str) -> String {
    percent_decode_str(segment).decode_utf8_lossy().into_owned()
}

fn invalid_build_ref<T>(input: &str) -> Result<T> {
    bail!("{}", invalid_build_ref_message(input));
}

fn invalid_build_ref_message(input: &str) -> String {
    format!(
        "Malformed Azure DevOps build reference: {:?}\n{}",
        input, ACCEPTED_FORMATS
    )
}

#[cfg(test)]
mod tests {
    use super::{ParsedBuildRef, parse_build_ref};

    struct SuccessCase {
        input: &'static str,
        expected: ParsedBuildRef,
    }

    struct ErrorCase {
        input: &'static str,
    }

    #[test]
    fn parses_supported_build_references() {
        let cases = vec![
            SuccessCase {
                input: "1234567890",
                expected: ParsedBuildRef {
                    build_id: 1_234_567_890,
                    org: None,
                    project: None,
                    host: None,
                    job_id: None,
                    step_id: None,
                },
            },
            SuccessCase {
                input: "https://dev.azure.com/my-org/My%20Project/_build/results?buildId=42",
                expected: ParsedBuildRef {
                    build_id: 42,
                    org: Some("my-org".to_string()),
                    project: Some("My Project".to_string()),
                    host: Some("dev.azure.com".to_string()),
                    job_id: None,
                    step_id: None,
                },
            },
            SuccessCase {
                input: "https://dev.azure.com/org/proj/_build/results?buildId=99&view=logs&j=abc-123&t=def-456",
                expected: ParsedBuildRef {
                    build_id: 99,
                    org: Some("org".to_string()),
                    project: Some("proj".to_string()),
                    host: Some("dev.azure.com".to_string()),
                    job_id: Some("abc-123".to_string()),
                    step_id: Some("def-456".to_string()),
                },
            },
            SuccessCase {
                input: "https://dev.azure.com/org/proj/_build/results?buildId=7&s=step-guid",
                expected: ParsedBuildRef {
                    build_id: 7,
                    org: Some("org".to_string()),
                    project: Some("proj".to_string()),
                    host: Some("dev.azure.com".to_string()),
                    job_id: None,
                    step_id: Some("step-guid".to_string()),
                },
            },
            SuccessCase {
                input: "https://my-org.visualstudio.com/proj/_build/results?buildId=5",
                expected: ParsedBuildRef {
                    build_id: 5,
                    org: Some("my-org".to_string()),
                    project: Some("proj".to_string()),
                    host: Some("my-org.visualstudio.com".to_string()),
                    job_id: None,
                    step_id: None,
                },
            },
            SuccessCase {
                input: "https://onprem.example.com/DefaultCollection/MyProject/_build/results?buildId=11",
                expected: ParsedBuildRef {
                    build_id: 11,
                    org: Some("DefaultCollection".to_string()),
                    project: Some("MyProject".to_string()),
                    host: Some("onprem.example.com".to_string()),
                    job_id: None,
                    step_id: None,
                },
            },
            SuccessCase {
                input: "https://dev.azure.com/org/proj/_BUILD/RESULTS/?buildId=1",
                expected: ParsedBuildRef {
                    build_id: 1,
                    org: Some("org".to_string()),
                    project: Some("proj".to_string()),
                    host: Some("dev.azure.com".to_string()),
                    job_id: None,
                    step_id: None,
                },
            },
        ];

        for case in cases {
            match parse_build_ref(case.input) {
                Ok(actual) => assert_eq!(actual, case.expected, "input: {:?}", case.input),
                Err(err) => panic!("expected success for {:?}: {err}", case.input),
            }
        }
    }

    #[test]
    fn rejects_malformed_build_references() {
        let cases = vec![
            ErrorCase { input: "" },
            ErrorCase { input: "abc" },
            ErrorCase {
                input: "https://dev.azure.com/org/proj/_build/results",
            },
            ErrorCase {
                input: "https://dev.azure.com/org/proj/_build/results?buildId=notanum",
            },
            ErrorCase {
                input: "https://dev.azure.com/org/proj/_other/results?buildId=1",
            },
        ];

        for case in cases {
            match parse_build_ref(case.input) {
                Ok(parsed) => panic!("expected error for {:?}, got {:?}", case.input, parsed),
                Err(err) => {
                    let message = err.to_string();
                    assert!(
                        message.contains("Accepted formats:"),
                        "missing accepted formats hint for {:?}: {}",
                        case.input,
                        message
                    );
                }
            }
        }
    }
}

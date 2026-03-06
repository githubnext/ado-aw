//! Fuzzy Schedule Parser
//!
//! Implements the Fuzzy Schedule Time Syntax specification for human-friendly
//! scheduling that automatically distributes execution times to prevent load spikes.
//!
//! Supported schedule types:
//! - `daily` - Scattered across full day
//! - `daily around HH:MM` - Within ±60 minute window
//! - `daily between HH:MM and HH:MM` - Within specified time range
//! - `weekly` - Scattered across full week
//! - `weekly on <weekday>` - On specific day, scattered time
//! - `weekly on <weekday> around HH:MM` - On specific day, within ±60 minute window
//! - `weekly on <weekday> between HH:MM and HH:MM` - On specific day, within range
//! - `hourly` - Every hour at scattered minute
//! - `every Nh` / `every N hours` - Every N hours at scattered minute
//! - `every Nm` / `every N minutes` - Every N minutes (fixed, not scattered)
//! - `bi-weekly` - Every 14 days at scattered time
//! - `tri-weekly` - Every 21 days at scattered time
//!
//! All times support optional UTC offset: `daily around 14:00 utc+9`

use anyhow::{Context, Result, bail};
use log::debug;

/// FNV-1a 32-bit hash constants
const FNV_OFFSET_BASIS: u32 = 2166136261;
const FNV_PRIME: u32 = 16777619;

/// Compute FNV-1a 32-bit hash for deterministic scattering
fn fnv1a_hash(data: &str) -> u32 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in data.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Parsed time specification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSpec {
    pub hour: u8,
    pub minute: u8,
}

impl TimeSpec {
    pub fn new(hour: u8, minute: u8) -> Result<Self> {
        if hour > 23 {
            bail!("Hour out of range (0-23): {}", hour);
        }
        if minute > 59 {
            bail!("Minute out of range (0-59): {}", minute);
        }
        Ok(Self { hour, minute })
    }

    pub fn to_minutes(self) -> u32 {
        self.hour as u32 * 60 + self.minute as u32
    }

    pub fn from_minutes(minutes: u32) -> Self {
        let minutes = minutes % 1440; // Wrap to 24 hours
        Self {
            hour: (minutes / 60) as u8,
            minute: (minutes % 60) as u8,
        }
    }
}

/// Day of week (0 = Sunday, 6 = Saturday)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weekday {
    Sunday = 0,
    Monday = 1,
    Tuesday = 2,
    Wednesday = 3,
    Thursday = 4,
    Friday = 5,
    Saturday = 6,
}

impl Weekday {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "sunday" | "sun" => Ok(Weekday::Sunday),
            "monday" | "mon" => Ok(Weekday::Monday),
            "tuesday" | "tue" | "tues" => Ok(Weekday::Tuesday),
            "wednesday" | "wed" => Ok(Weekday::Wednesday),
            "thursday" | "thu" | "thurs" => Ok(Weekday::Thursday),
            "friday" | "fri" => Ok(Weekday::Friday),
            "saturday" | "sat" => Ok(Weekday::Saturday),
            _ => bail!(
                "Unknown weekday '{}'. Valid weekdays: sunday, monday, tuesday, wednesday, thursday, friday, saturday",
                s
            ),
        }
    }

    pub fn to_cron(self) -> u8 {
        self as u8
    }
}

/// Time constraint for schedules
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeConstraint {
    /// No constraint - scatter across full range
    None,
    /// Around a specific time (±60 minutes)
    Around(TimeSpec),
    /// Between two times (inclusive)
    Between(TimeSpec, TimeSpec),
}

/// Parsed schedule expression
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FuzzySchedule {
    /// Daily schedule with optional time constraint
    Daily(TimeConstraint),
    /// Weekly schedule with optional day and time constraint
    Weekly {
        day: Option<Weekday>,
        constraint: TimeConstraint,
    },
    /// Hourly schedule (scattered minute)
    Hourly,
    /// Every N hours (scattered minute)
    EveryHours(u8),
    /// Every N minutes (fixed, not scattered)
    EveryMinutes(u8),
    /// Every N days (scattered time)
    EveryDays(u8),
    /// Bi-weekly (every 14 days)
    BiWeekly,
    /// Tri-weekly (every 21 days)
    TriWeekly,
}

/// Parse a time specification like "14:00", "3pm", "midnight", "noon"
fn parse_time_spec(s: &str) -> Result<TimeSpec> {
    let s = s.trim().to_lowercase();

    // Handle keywords
    match s.as_str() {
        "midnight" => return TimeSpec::new(0, 0),
        "noon" => return TimeSpec::new(12, 0),
        _ => {}
    }

    // Handle 12-hour format: "3pm", "11am", "3:30pm"
    if s.ends_with("am") || s.ends_with("pm") {
        let is_pm = s.ends_with("pm");
        let time_part = &s[..s.len() - 2];

        let (hour, minute) = if let Some((h, m)) = time_part.split_once(':') {
            let hour: u8 = h.parse().context("Invalid hour in 12-hour format")?;
            let minute: u8 = m.parse().context("Invalid minute in 12-hour format")?;
            (hour, minute)
        } else {
            let hour: u8 = time_part
                .parse()
                .context("Invalid hour in 12-hour format")?;
            (hour, 0)
        };

        if hour < 1 || hour > 12 {
            bail!("Hour must be 1-12 in 12-hour format, got {}", hour);
        }
        if minute > 59 {
            bail!("Minute must be 0-59, got {}", minute);
        }

        // Convert to 24-hour format
        let hour_24 = match (hour, is_pm) {
            (12, false) => 0,    // 12am = midnight
            (12, true) => 12,    // 12pm = noon
            (h, false) => h,     // 1am-11am = 1-11
            (h, true) => h + 12, // 1pm-11pm = 13-23
        };

        return TimeSpec::new(hour_24, minute);
    }

    // Handle 24-hour format: "14:00", "9:30"
    if let Some((h, m)) = s.split_once(':') {
        let hour: u8 = h.parse().context("Invalid hour in 24-hour format")?;
        let minute: u8 = m.parse().context("Invalid minute in 24-hour format")?;
        return TimeSpec::new(hour, minute);
    }

    bail!(
        "Invalid time format '{}'. Use 24-hour (14:00), 12-hour (3pm), or keywords (midnight, noon)",
        s
    );
}

/// Parse UTC offset like "utc+9", "utc-5", "utc+05:30"
fn parse_utc_offset(s: &str) -> Result<i32> {
    let s = s.trim().to_lowercase();

    if !s.starts_with("utc") {
        bail!("UTC offset must start with 'utc', got '{}'", s);
    }

    let offset_part = &s[3..];
    if offset_part.is_empty() {
        return Ok(0); // "utc" alone means UTC+0
    }

    let (sign, value) = if let Some(v) = offset_part.strip_prefix('+') {
        (1, v)
    } else if let Some(v) = offset_part.strip_prefix('-') {
        (-1, v)
    } else {
        bail!("UTC offset must have + or - sign after 'utc', got '{}'", s);
    };

    // Parse hours and optional minutes
    let offset_minutes = if let Some((h, m)) = value.split_once(':') {
        let hours: i32 = h.parse().context("Invalid hours in UTC offset")?;
        let minutes: i32 = m.parse().context("Invalid minutes in UTC offset")?;
        hours * 60 + minutes
    } else {
        let hours: i32 = value.parse().context("Invalid hours in UTC offset")?;
        hours * 60
    };

    let total_offset = sign * offset_minutes;

    // Validate range: UTC-12:00 to UTC+14:00
    if total_offset < -12 * 60 || total_offset > 14 * 60 {
        bail!("UTC offset out of range (UTC-12:00 to UTC+14:00): {}", s);
    }

    Ok(total_offset)
}

/// Convert local time to UTC given an offset in minutes
fn to_utc(time: TimeSpec, offset_minutes: i32) -> TimeSpec {
    let local_minutes = time.to_minutes() as i32;
    let utc_minutes = local_minutes - offset_minutes;

    // Handle day wrapping
    let utc_minutes = if utc_minutes < 0 {
        (utc_minutes + 1440) as u32
    } else if utc_minutes >= 1440 {
        (utc_minutes - 1440) as u32
    } else {
        utc_minutes as u32
    };

    TimeSpec::from_minutes(utc_minutes)
}

/// Parse a fuzzy schedule expression
pub fn parse_fuzzy_schedule(input: &str) -> Result<FuzzySchedule> {
    debug!("Parsing fuzzy schedule: '{}'", input);
    let input = input.trim().to_lowercase();

    // Split into tokens
    let tokens: Vec<&str> = input.split_whitespace().collect();

    if tokens.is_empty() {
        bail!("Empty schedule expression");
    }

    match tokens[0] {
        "daily" => parse_daily_schedule(&tokens[1..]),
        "weekly" => parse_weekly_schedule(&tokens[1..]),
        "hourly" => {
            if tokens.len() > 1 {
                bail!(
                    "'hourly' does not accept additional parameters. Use 'every Nh' for interval schedules."
                );
            }
            Ok(FuzzySchedule::Hourly)
        }
        "every" => parse_interval_schedule(&tokens[1..]),
        "bi-weekly" | "biweekly" => {
            if tokens.len() > 1 {
                bail!("'bi-weekly' does not accept additional parameters");
            }
            Ok(FuzzySchedule::BiWeekly)
        }
        "tri-weekly" | "triweekly" => {
            if tokens.len() > 1 {
                bail!("'tri-weekly' does not accept additional parameters");
            }
            Ok(FuzzySchedule::TriWeekly)
        }
        other => bail!(
            "Unknown schedule type '{}'. Valid types: daily, weekly, hourly, every, bi-weekly, tri-weekly",
            other
        ),
    }
}

fn parse_daily_schedule(tokens: &[&str]) -> Result<FuzzySchedule> {
    if tokens.is_empty() {
        return Ok(FuzzySchedule::Daily(TimeConstraint::None));
    }

    match tokens[0] {
        "around" => {
            if tokens.len() < 2 {
                bail!("'around' requires a time specification. Example: daily around 14:00");
            }
            let (time, _offset) = parse_time_with_offset(&tokens[1..])?;
            Ok(FuzzySchedule::Daily(TimeConstraint::Around(time)))
        }
        "between" => {
            // Format: between <start> and <end>
            let and_pos = tokens.iter().position(|&t| t == "and");
            let Some(and_pos) = and_pos else {
                bail!("'between' requires format: between <start> and <end>");
            };

            if and_pos < 2 || and_pos + 1 >= tokens.len() {
                bail!("'between' requires format: between <start> and <end>");
            }

            let (start_time, _) = parse_time_with_offset(&tokens[1..and_pos])?;
            let (end_time, _) = parse_time_with_offset(&tokens[and_pos + 1..])?;

            Ok(FuzzySchedule::Daily(TimeConstraint::Between(
                start_time, end_time,
            )))
        }
        "at" => bail!(
            "'daily at <time>' syntax is not supported. Use 'daily around <time>' for fuzzy scheduling within ±1 hour window"
        ),
        _ => bail!(
            "Unknown daily schedule modifier '{}'. Use 'around' or 'between'",
            tokens[0]
        ),
    }
}

fn parse_weekly_schedule(tokens: &[&str]) -> Result<FuzzySchedule> {
    if tokens.is_empty() {
        return Ok(FuzzySchedule::Weekly {
            day: None,
            constraint: TimeConstraint::None,
        });
    }

    // Check for "on <weekday>"
    if tokens[0] != "on" {
        bail!("Weekly schedule expects 'on <weekday>'. Example: weekly on monday");
    }

    if tokens.len() < 2 {
        bail!("'weekly on' requires a weekday. Example: weekly on monday");
    }

    let day = Weekday::parse(tokens[1])?;
    let remaining = &tokens[2..];

    if remaining.is_empty() {
        return Ok(FuzzySchedule::Weekly {
            day: Some(day),
            constraint: TimeConstraint::None,
        });
    }

    match remaining[0] {
        "around" => {
            if remaining.len() < 2 {
                bail!(
                    "'around' requires a time specification. Example: weekly on friday around 17:00"
                );
            }
            let (time, _) = parse_time_with_offset(&remaining[1..])?;
            Ok(FuzzySchedule::Weekly {
                day: Some(day),
                constraint: TimeConstraint::Around(time),
            })
        }
        "between" => {
            let and_pos = remaining.iter().position(|&t| t == "and");
            let Some(and_pos) = and_pos else {
                bail!("'between' requires format: between <start> and <end>");
            };

            if and_pos < 2 || and_pos + 1 >= remaining.len() {
                bail!("'between' requires format: between <start> and <end>");
            }

            let (start_time, _) = parse_time_with_offset(&remaining[1..and_pos])?;
            let (end_time, _) = parse_time_with_offset(&remaining[and_pos + 1..])?;

            Ok(FuzzySchedule::Weekly {
                day: Some(day),
                constraint: TimeConstraint::Between(start_time, end_time),
            })
        }
        _ => bail!(
            "Unknown weekly schedule modifier '{}'. Use 'around' or 'between'",
            remaining[0]
        ),
    }
}

fn parse_interval_schedule(tokens: &[&str]) -> Result<FuzzySchedule> {
    if tokens.is_empty() {
        bail!("'every' requires an interval specification. Example: every 2h, every 5 minutes");
    }

    // Try to parse combined format: "2h", "5m", "3d", "2w"
    let interval_str = tokens[0];

    // Check for suffix patterns
    for (suffix, unit) in &[
        ("hours", "h"),
        ("hour", "h"),
        ("h", "h"),
        ("minutes", "m"),
        ("minute", "m"),
        ("mins", "m"),
        ("min", "m"),
        ("m", "m"),
        ("days", "d"),
        ("day", "d"),
        ("d", "d"),
        ("weeks", "w"),
        ("week", "w"),
        ("w", "w"),
    ] {
        if let Some(num_str) = interval_str.strip_suffix(suffix) {
            if let Ok(n) = num_str.parse::<u8>() {
                return create_interval_schedule(n, unit);
            }
        }
    }

    // Try format: "<N> <unit>" (e.g., "2 hours")
    if tokens.len() >= 2 {
        if let Ok(n) = tokens[0].parse::<u8>() {
            let unit = tokens[1];
            let unit_char = match unit {
                "hours" | "hour" | "h" => "h",
                "minutes" | "minute" | "mins" | "min" | "m" => "m",
                "days" | "day" | "d" => "d",
                "weeks" | "week" | "w" => "w",
                _ => bail!(
                    "Unknown interval unit '{}'. Valid units: hours, minutes, days, weeks",
                    unit
                ),
            };
            return create_interval_schedule(n, unit_char);
        }
    }

    bail!(
        "Invalid interval format '{}'. Examples: every 2h, every 5 minutes, every 3 days",
        tokens.join(" ")
    );
}

fn create_interval_schedule(n: u8, unit: &str) -> Result<FuzzySchedule> {
    if n == 0 {
        bail!("Interval must be greater than 0");
    }

    match unit {
        "h" => {
            // Validate hour intervals (should be factors of 24 for even distribution)
            let valid_hours = [1, 2, 3, 4, 6, 8, 12];
            if !valid_hours.contains(&n) {
                bail!(
                    "Hour interval {} is not recommended. Valid intervals: {} (factors of 24 for even distribution)",
                    n,
                    valid_hours
                        .iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            Ok(FuzzySchedule::EveryHours(n))
        }
        "m" => {
            // Minimum 5 minutes per GitHub Actions constraint
            if n < 5 {
                bail!(
                    "Minute interval must be at least 5 minutes (GitHub Actions constraint), got {}",
                    n
                );
            }
            Ok(FuzzySchedule::EveryMinutes(n))
        }
        "d" => Ok(FuzzySchedule::EveryDays(n)),
        "w" => {
            // Convert weeks to days
            let days = n
                .checked_mul(7)
                .ok_or_else(|| anyhow::anyhow!("Week interval too large"))?;
            Ok(FuzzySchedule::EveryDays(days))
        }
        _ => bail!("Unknown unit '{}'", unit),
    }
}

/// Parse time with optional UTC offset
fn parse_time_with_offset(tokens: &[&str]) -> Result<(TimeSpec, i32)> {
    if tokens.is_empty() {
        bail!("Expected time specification");
    }

    // Check if last token is a UTC offset
    let (time_tokens, offset) =
        if tokens.len() >= 2 && tokens.last().map_or(false, |t| t.starts_with("utc")) {
            let offset = parse_utc_offset(tokens.last().unwrap())?;
            (&tokens[..tokens.len() - 1], offset)
        } else if tokens.len() == 1 && tokens[0].contains("utc") {
            // Handle cases like "14:00utc+9" (no space)
            if let Some(utc_pos) = tokens[0].find("utc") {
                let time_part = &tokens[0][..utc_pos];
                let offset_part = &tokens[0][utc_pos..];
                let time = parse_time_spec(time_part)?;
                let offset = parse_utc_offset(offset_part)?;
                return Ok((to_utc(time, offset), offset));
            }
            (tokens, 0)
        } else {
            (tokens, 0)
        };

    // Join time tokens and parse
    let time_str = time_tokens.join("");
    let time = parse_time_spec(&time_str)?;

    // Convert to UTC if offset specified
    let utc_time = if offset != 0 {
        to_utc(time, offset)
    } else {
        time
    };

    Ok((utc_time, offset))
}

/// Generate a cron expression from a fuzzy schedule
pub fn generate_cron(schedule: &FuzzySchedule, workflow_id: &str) -> String {
    let hash = fnv1a_hash(workflow_id);

    match schedule {
        FuzzySchedule::Daily(constraint) => generate_daily_cron(hash, constraint),
        FuzzySchedule::Weekly { day, constraint } => generate_weekly_cron(hash, *day, constraint),
        FuzzySchedule::Hourly => {
            let minute = hash % 60;
            format!("{} * * * *", minute)
        }
        FuzzySchedule::EveryHours(n) => {
            let minute = hash % 60;
            format!("{} */{} * * *", minute, n)
        }
        FuzzySchedule::EveryMinutes(n) => {
            // Fixed intervals, not scattered
            format!("*/{} * * * *", n)
        }
        FuzzySchedule::EveryDays(n) => {
            let minute = hash % 60;
            let hour = (hash / 60) % 24;
            format!("{} {} */{} * *", minute, hour, n)
        }
        FuzzySchedule::BiWeekly => {
            let minute = hash % 60;
            let hour = (hash / 60) % 24;
            format!("{} {} */14 * *", minute, hour)
        }
        FuzzySchedule::TriWeekly => {
            let minute = hash % 60;
            let hour = (hash / 60) % 24;
            format!("{} {} */21 * *", minute, hour)
        }
    }
}

fn generate_daily_cron(hash: u32, constraint: &TimeConstraint) -> String {
    match constraint {
        TimeConstraint::None => {
            // Scatter across full 24 hours
            let total_minutes = hash % 1440;
            let hour = total_minutes / 60;
            let minute = total_minutes % 60;
            format!("{} {} * * *", minute, hour)
        }
        TimeConstraint::Around(time) => {
            // Scatter within ±60 minute window
            let target_minutes = time.to_minutes();
            let offset = (hash % 120) as i32 - 60; // Range: -60 to +59
            let scattered = (target_minutes as i32 + offset).rem_euclid(1440) as u32;
            let hour = scattered / 60;
            let minute = scattered % 60;
            format!("{} {} * * *", minute, hour)
        }
        TimeConstraint::Between(start, end) => {
            let start_minutes = start.to_minutes();
            let end_minutes = end.to_minutes();

            // Calculate range size (handling midnight crossing)
            let range_size = if end_minutes > start_minutes {
                end_minutes - start_minutes
            } else if start_minutes > end_minutes {
                // Midnight crossing: e.g., 22:00 to 02:00
                (1440 - start_minutes) + end_minutes
            } else {
                // Same time means full 24-hour range
                1440
            };

            let offset = hash % range_size;
            let scattered = (start_minutes + offset) % 1440;
            let hour = scattered / 60;
            let minute = scattered % 60;
            format!("{} {} * * *", minute, hour)
        }
    }
}

fn generate_weekly_cron(hash: u32, day: Option<Weekday>, constraint: &TimeConstraint) -> String {
    let day_of_week = match day {
        Some(d) => d.to_cron().to_string(),
        None => {
            // Scatter across all days
            let dow = (hash / 1440) % 7;
            dow.to_string()
        }
    };

    let time_cron = match constraint {
        TimeConstraint::None => {
            // Scatter across full day
            let total_minutes = hash % 1440;
            let hour = total_minutes / 60;
            let minute = total_minutes % 60;
            format!("{} {}", minute, hour)
        }
        TimeConstraint::Around(time) => {
            let target_minutes = time.to_minutes();
            let offset = (hash % 120) as i32 - 60;
            let scattered = (target_minutes as i32 + offset).rem_euclid(1440) as u32;
            let hour = scattered / 60;
            let minute = scattered % 60;
            format!("{} {}", minute, hour)
        }
        TimeConstraint::Between(start, end) => {
            let start_minutes = start.to_minutes();
            let end_minutes = end.to_minutes();

            let range_size = if end_minutes > start_minutes {
                end_minutes - start_minutes
            } else if start_minutes > end_minutes {
                (1440 - start_minutes) + end_minutes
            } else {
                // Same time means full 24-hour range
                1440
            };

            let offset = hash % range_size;
            let scattered = (start_minutes + offset) % 1440;
            let hour = scattered / 60;
            let minute = scattered % 60;
            format!("{} {}", minute, hour)
        }
    };

    format!("{} * * {}", time_cron, day_of_week)
}

/// Generate full schedule YAML block for Azure DevOps pipelines.
///
/// When `branches` is empty, no branch filter is emitted — the schedule fires on
/// any branch where the YAML exists. When `branches` is non-empty, a
/// `branches.include` block is generated to restrict which branches trigger the schedule.
pub fn generate_schedule_yaml(
    schedule_str: &str,
    workflow_id: &str,
    branches: &[String],
) -> Result<String> {
    debug!(
        "Generating schedule YAML for '{}' (workflow: {})",
        schedule_str, workflow_id
    );
    let schedule = parse_fuzzy_schedule(schedule_str)?;
    let cron = generate_cron(&schedule, workflow_id);
    debug!("Generated cron expression: '{}'", cron);

    let branches_block = if branches.is_empty() {
        String::new()
    } else {
        let entries: Vec<String> = branches.iter().map(|b| format!("        - {}", b)).collect();
        format!(
            "\n    branches:\n      include:\n{}",
            entries.join("\n")
        )
    };

    Ok(format!(
        r#"schedules:
  - cron: "{}"
    displayName: "Scheduled run"{}
    always: true
"#,
        cron, branches_block
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_time_24h() {
        assert_eq!(
            parse_time_spec("14:00").unwrap(),
            TimeSpec {
                hour: 14,
                minute: 0
            }
        );
        assert_eq!(
            parse_time_spec("9:30").unwrap(),
            TimeSpec {
                hour: 9,
                minute: 30
            }
        );
        assert_eq!(
            parse_time_spec("00:00").unwrap(),
            TimeSpec { hour: 0, minute: 0 }
        );
        assert_eq!(
            parse_time_spec("23:59").unwrap(),
            TimeSpec {
                hour: 23,
                minute: 59
            }
        );
    }

    #[test]
    fn test_parse_time_12h() {
        assert_eq!(
            parse_time_spec("3pm").unwrap(),
            TimeSpec {
                hour: 15,
                minute: 0
            }
        );
        assert_eq!(
            parse_time_spec("11am").unwrap(),
            TimeSpec {
                hour: 11,
                minute: 0
            }
        );
        assert_eq!(
            parse_time_spec("12am").unwrap(),
            TimeSpec { hour: 0, minute: 0 }
        ); // midnight
        assert_eq!(
            parse_time_spec("12pm").unwrap(),
            TimeSpec {
                hour: 12,
                minute: 0
            }
        ); // noon
        assert_eq!(
            parse_time_spec("3:30pm").unwrap(),
            TimeSpec {
                hour: 15,
                minute: 30
            }
        );
    }

    #[test]
    fn test_parse_time_keywords() {
        assert_eq!(
            parse_time_spec("midnight").unwrap(),
            TimeSpec { hour: 0, minute: 0 }
        );
        assert_eq!(
            parse_time_spec("noon").unwrap(),
            TimeSpec {
                hour: 12,
                minute: 0
            }
        );
    }

    #[test]
    fn test_parse_time_invalid() {
        assert!(parse_time_spec("25:00").is_err());
        assert!(parse_time_spec("14:60").is_err());
        assert!(parse_time_spec("13pm").is_err());
    }

    #[test]
    fn test_parse_utc_offset() {
        assert_eq!(parse_utc_offset("utc+9").unwrap(), 9 * 60);
        assert_eq!(parse_utc_offset("utc-5").unwrap(), -5 * 60);
        assert_eq!(parse_utc_offset("utc+05:30").unwrap(), 5 * 60 + 30);
        assert_eq!(parse_utc_offset("utc-08:00").unwrap(), -8 * 60);
    }

    #[test]
    fn test_to_utc() {
        // 14:00 JST (UTC+9) -> 05:00 UTC
        let time = TimeSpec {
            hour: 14,
            minute: 0,
        };
        let utc = to_utc(time, 9 * 60);
        assert_eq!(utc, TimeSpec { hour: 5, minute: 0 });

        // 02:00 JST (UTC+9) -> 17:00 UTC (previous day)
        let time = TimeSpec { hour: 2, minute: 0 };
        let utc = to_utc(time, 9 * 60);
        assert_eq!(
            utc,
            TimeSpec {
                hour: 17,
                minute: 0
            }
        );
    }

    #[test]
    fn test_parse_daily() {
        assert!(matches!(
            parse_fuzzy_schedule("daily").unwrap(),
            FuzzySchedule::Daily(TimeConstraint::None)
        ));

        let schedule = parse_fuzzy_schedule("daily around 14:00").unwrap();
        assert!(matches!(
            schedule,
            FuzzySchedule::Daily(TimeConstraint::Around(_))
        ));

        let schedule = parse_fuzzy_schedule("daily between 9:00 and 17:00").unwrap();
        assert!(matches!(
            schedule,
            FuzzySchedule::Daily(TimeConstraint::Between(_, _))
        ));
    }

    #[test]
    fn test_parse_weekly() {
        assert!(matches!(
            parse_fuzzy_schedule("weekly").unwrap(),
            FuzzySchedule::Weekly {
                day: None,
                constraint: TimeConstraint::None
            }
        ));

        let schedule = parse_fuzzy_schedule("weekly on monday").unwrap();
        assert!(matches!(
            schedule,
            FuzzySchedule::Weekly {
                day: Some(Weekday::Monday),
                constraint: TimeConstraint::None
            }
        ));

        let schedule = parse_fuzzy_schedule("weekly on friday around 17:00").unwrap();
        assert!(matches!(
            schedule,
            FuzzySchedule::Weekly {
                day: Some(Weekday::Friday),
                constraint: TimeConstraint::Around(_)
            }
        ));
    }

    #[test]
    fn test_parse_hourly() {
        assert!(matches!(
            parse_fuzzy_schedule("hourly").unwrap(),
            FuzzySchedule::Hourly
        ));
    }

    #[test]
    fn test_parse_intervals() {
        assert!(matches!(
            parse_fuzzy_schedule("every 2h").unwrap(),
            FuzzySchedule::EveryHours(2)
        ));
        assert!(matches!(
            parse_fuzzy_schedule("every 6 hours").unwrap(),
            FuzzySchedule::EveryHours(6)
        ));
        assert!(matches!(
            parse_fuzzy_schedule("every 5 minutes").unwrap(),
            FuzzySchedule::EveryMinutes(5)
        ));
        assert!(matches!(
            parse_fuzzy_schedule("every 2 days").unwrap(),
            FuzzySchedule::EveryDays(2)
        ));
    }

    #[test]
    fn test_parse_special_periods() {
        assert!(matches!(
            parse_fuzzy_schedule("bi-weekly").unwrap(),
            FuzzySchedule::BiWeekly
        ));
        assert!(matches!(
            parse_fuzzy_schedule("tri-weekly").unwrap(),
            FuzzySchedule::TriWeekly
        ));
    }

    #[test]
    fn test_cron_generation_deterministic() {
        let schedule = FuzzySchedule::Daily(TimeConstraint::None);
        let cron1 = generate_cron(&schedule, "test/workflow");
        let cron2 = generate_cron(&schedule, "test/workflow");
        assert_eq!(cron1, cron2, "Same workflow should produce same cron");

        let _cron3 = generate_cron(&schedule, "other/workflow");
        // Different workflows should (usually) produce different crons
        // Note: There's a small chance of collision, but it's unlikely
    }

    #[test]
    fn test_cron_format() {
        let schedule = FuzzySchedule::Daily(TimeConstraint::None);
        let cron = generate_cron(&schedule, "test");
        let parts: Vec<&str> = cron.split_whitespace().collect();
        assert_eq!(parts.len(), 5, "Cron should have 5 fields");

        // Validate minute field
        let minute: u32 = parts[0].parse().expect("Minute should be a number");
        assert!(minute < 60, "Minute should be 0-59");

        // Validate hour field
        let hour: u32 = parts[1].parse().expect("Hour should be a number");
        assert!(hour < 24, "Hour should be 0-23");
    }

    #[test]
    fn test_between_equal_times_daily() {
        // Test edge case: daily between 14:00 and 14:00 (same time)
        // Should not panic and should generate valid cron
        let schedule = parse_fuzzy_schedule("daily between 14:00 and 14:00").unwrap();
        let cron = generate_cron(&schedule, "test/agent");

        // Verify it's a valid cron format
        let parts: Vec<&str> = cron.split_whitespace().collect();
        assert_eq!(parts.len(), 5, "Cron should have 5 fields");

        let minute: u32 = parts[0].parse().expect("Minute should be a number");
        assert!(minute < 60, "Minute should be 0-59");

        let hour: u32 = parts[1].parse().expect("Hour should be a number");
        assert!(hour < 24, "Hour should be 0-23");
    }

    #[test]
    fn test_between_equal_times_weekly() {
        // Test edge case: weekly on monday between 09:00 and 09:00 (same time)
        // Should not panic and should generate valid cron
        let schedule = parse_fuzzy_schedule("weekly on monday between 09:00 and 09:00").unwrap();
        let cron = generate_cron(&schedule, "test/agent");

        // Verify it's a valid cron format
        let parts: Vec<&str> = cron.split_whitespace().collect();
        assert_eq!(parts.len(), 5, "Cron should have 5 fields");

        let minute: u32 = parts[0].parse().expect("Minute should be a number");
        assert!(minute < 60, "Minute should be 0-59");

        let hour: u32 = parts[1].parse().expect("Hour should be a number");
        assert!(hour < 24, "Hour should be 0-23");

        // Verify day of week is Monday (1)
        assert_eq!(parts[4], "1", "Day of week should be Monday (1)");
    }

    #[test]
    fn test_between_equal_times_midnight() {
        // Test edge case: between midnight and midnight
        let schedule = parse_fuzzy_schedule("daily between midnight and midnight").unwrap();
        let cron = generate_cron(&schedule, "test/agent");

        let parts: Vec<&str> = cron.split_whitespace().collect();
        assert_eq!(parts.len(), 5, "Cron should have 5 fields");

        let minute: u32 = parts[0].parse().expect("Minute should be a number");
        assert!(minute < 60, "Minute should be 0-59");

        let hour: u32 = parts[1].parse().expect("Hour should be a number");
        assert!(hour < 24, "Hour should be 0-23");
    }

    #[test]
    fn test_generate_schedule_yaml() {
        let yaml = generate_schedule_yaml("daily", "test/agent", &[]).unwrap();
        assert!(yaml.contains("schedules:"));
        assert!(yaml.contains("cron:"));
        // No branches filter by default
        assert!(!yaml.contains("branches:"));
    }

    #[test]
    fn test_generate_schedule_yaml_with_branches() {
        let branches = vec!["main".to_string(), "release/*".to_string()];
        let yaml = generate_schedule_yaml("daily", "test/agent", &branches).unwrap();
        assert!(yaml.contains("schedules:"));
        assert!(yaml.contains("cron:"));
        assert!(yaml.contains("branches:"));
        assert!(yaml.contains("include:"));
        assert!(yaml.contains("- main"));
        assert!(yaml.contains("- release/*"));
    }

    #[test]
    fn test_error_messages() {
        let err = parse_fuzzy_schedule("monthly").unwrap_err();
        assert!(err.to_string().contains("Unknown schedule type"));

        let err = parse_fuzzy_schedule("daily at 14:00").unwrap_err();
        assert!(err.to_string().contains("not supported"));

        let err = parse_fuzzy_schedule("daily around").unwrap_err();
        assert!(err.to_string().contains("requires a time"));

        let err = parse_fuzzy_schedule("every 3 minutes").unwrap_err();
        assert!(err.to_string().contains("at least 5 minutes"));
    }

    #[test]
    fn test_backward_compatibility() {
        // Test that simple "hourly" and "daily" still work
        let yaml = generate_schedule_yaml("hourly", "test", &[]).unwrap();
        assert!(yaml.contains("cron:"));

        let yaml = generate_schedule_yaml("daily", "test", &[]).unwrap();
        assert!(yaml.contains("cron:"));
    }
}

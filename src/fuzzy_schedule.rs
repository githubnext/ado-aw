//! Fuzzy Schedule Parser
//!
//! Implements the Fuzzy Schedule Time Syntax specification for human-friendly
//! scheduling that automatically distributes execution times to prevent load spikes.
//!
//! Supported schedule types:
//! - `daily` - Scattered across full day
//! - `daily on weekdays` - Scattered across Monday-Friday
//! - `daily around HH:MM` - Within ±60 minute window
//! - `daily between HH:MM and HH:MM` - Within specified time range
//! - `weekly` - Scattered across full week
//! - `weekly on <weekday>` - On specific day, scattered time
//! - `weekly on <weekday> around HH:MM` - On specific day, within ±60 minute window
//! - `weekly on <weekday> between HH:MM and HH:MM` - On specific day, within range
//! - `hourly` - Every hour at scattered minute
//! - `hourly on weekdays` - Every hour Monday-Friday at scattered minute
//! - `every Nh` / `every N hours` - Every N hours at scattered minute
//! - `every Nh on weekdays` - Every N hours Monday-Friday at scattered minute
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

/// Day filter for schedule forms that can run daily.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayFilter {
    EveryDay,
    Weekdays { utc_offset_minutes: i32 },
}

impl DayFilter {
    fn cron_field(self) -> &'static str {
        match self {
            DayFilter::EveryDay => "*",
            DayFilter::Weekdays {
                utc_offset_minutes: 0,
            } => "1-5",
            DayFilter::Weekdays { .. } => {
                unreachable!("timezone-aware weekday filters require a generated UTC time")
            }
        }
    }

    fn with_utc_offset(self, utc_offset_minutes: i32) -> Self {
        match self {
            DayFilter::EveryDay => DayFilter::EveryDay,
            DayFilter::Weekdays { .. } => DayFilter::Weekdays {
                utc_offset_minutes,
            },
        }
    }

    fn cron_field_for_utc_time(self, utc_minutes: u32) -> &'static str {
        match self {
            DayFilter::EveryDay => "*",
            DayFilter::Weekdays { utc_offset_minutes } => {
                let local_minutes = utc_minutes as i32 + utc_offset_minutes;
                match local_minutes.div_euclid(1440) {
                    -1 => "2-6",
                    0 => "1-5",
                    1 => "0-4",
                    shift => unreachable!("UTC offset produced unsupported day shift {shift}"),
                }
            }
        }
    }
}

/// Parsed schedule expression
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FuzzySchedule {
    /// Daily schedule with optional time constraint
    Daily {
        constraint: TimeConstraint,
        days: DayFilter,
    },
    /// Weekly schedule with optional day and time constraint
    Weekly {
        day: Option<Weekday>,
        constraint: TimeConstraint,
    },
    /// Hourly schedule (scattered minute)
    Hourly { days: DayFilter },
    /// Every N hours (scattered minute)
    EveryHours { interval: u8, days: DayFilter },
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

        if !(1..=12).contains(&hour) {
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
    if !(-12 * 60..=14 * 60).contains(&total_offset) {
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
            let days = parse_hourly_day_filter(&tokens[1..])?;
            Ok(FuzzySchedule::Hourly { days })
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

/// Parse either a fuzzy schedule expression or a validated five-field ADO cron.
pub fn schedule_expression_to_cron(input: &str, workflow_id: &str) -> Result<String> {
    let input = input.trim();
    if looks_like_raw_cron(input) {
        validate_raw_cron(input)?;
        return Ok(input.to_string());
    }

    let schedule = parse_fuzzy_schedule(input)?;
    Ok(generate_cron(&schedule, workflow_id))
}

fn looks_like_raw_cron(input: &str) -> bool {
    let fields = input.split_whitespace().collect::<Vec<_>>();
    let Some(first) = fields.first() else {
        return false;
    };
    if matches!(
        *first,
        "daily" | "weekly" | "hourly" | "every" | "bi-weekly" | "biweekly"
            | "tri-weekly" | "triweekly"
    ) {
        return false;
    }
    fields.len() == 5
        || first.starts_with('*')
        || first.starts_with('$')
        || first.chars().any(|ch| ch.is_ascii_digit())
}

fn validate_raw_cron(input: &str) -> Result<()> {
    let fields = input.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 5 {
        bail!(
            "ADO cron expressions require exactly 5 fields (minute hour day-of-month month day-of-week), got {} in '{}'",
            fields.len(),
            input
        );
    }

    let definitions = [
        ("minute", 0, 59, CronValueKind::Numeric),
        ("hour", 0, 23, CronValueKind::Numeric),
        ("day-of-month", 1, 31, CronValueKind::Numeric),
        ("month", 1, 12, CronValueKind::Month),
        ("day-of-week", 0, 6, CronValueKind::Weekday),
    ];
    for (field, (name, min, max, kind)) in fields.iter().zip(definitions) {
        validate_raw_cron_field(field, name, min, max, kind)?;
    }

    Ok(())
}

#[derive(Clone, Copy)]
enum CronValueKind {
    Numeric,
    Month,
    Weekday,
}

fn validate_raw_cron_field(
    field: &str,
    name: &str,
    min: u8,
    max: u8,
    kind: CronValueKind,
) -> Result<()> {
    if field.is_empty() {
        bail!("ADO cron {name} field cannot be empty");
    }
    if !field
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '*' | ',' | '-' | '/'))
    {
        bail!(
            "ADO cron {name} field '{}' contains unsupported characters",
            field
        );
    }

    for item in field.split(',') {
        if item.is_empty() {
            bail!("ADO cron {name} field '{}' contains an empty list item", field);
        }

        let mut step_parts = item.split('/');
        let base = step_parts.next().unwrap_or_default();
        let step = step_parts.next();
        if step_parts.next().is_some() {
            bail!(
                "ADO cron {name} field '{}' contains more than one step separator",
                field
            );
        }
        if let Some(step) = step {
            let step = parse_cron_number(step, name, field)?;
            if step == 0 {
                bail!("ADO cron {name} field '{}' has a zero step", field);
            }
        }

        if base == "*" {
            continue;
        }
        if base.is_empty() {
            bail!("ADO cron {name} field '{}' is missing a value", field);
        }

        if let Some((start, end)) = base.split_once('-') {
            if end.contains('-') {
                bail!("ADO cron {name} field '{}' contains an invalid range", field);
            }
            let start = parse_cron_value(start, name, field, kind)?;
            let end = parse_cron_value(end, name, field, kind)?;
            validate_cron_value(start, name, field, min, max)?;
            validate_cron_value(end, name, field, min, max)?;
            if start > end {
                bail!(
                    "ADO cron {name} field '{}' has a reversed range {}-{}",
                    field,
                    start,
                    end
                );
            }
        } else {
            let value = parse_cron_value(base, name, field, kind)?;
            validate_cron_value(value, name, field, min, max)?;
        }
    }

    Ok(())
}

fn parse_cron_number(value: &str, name: &str, field: &str) -> Result<u8> {
    if value.is_empty() {
        bail!("ADO cron {name} field '{}' contains a missing number", field);
    }
    value.parse::<u8>().with_context(|| {
        format!(
            "ADO cron {name} field '{}' contains invalid number '{}'",
            field, value
        )
    })
}

fn parse_cron_value(value: &str, name: &str, field: &str, kind: CronValueKind) -> Result<u8> {
    if let Ok(value) = value.parse::<u8>() {
        return Ok(value);
    }

    let normalized = value.to_ascii_lowercase();
    let named_value = match kind {
        CronValueKind::Numeric => None,
        CronValueKind::Month => match normalized.as_str() {
            "jan" | "january" => Some(1),
            "feb" | "february" => Some(2),
            "mar" | "march" => Some(3),
            "apr" | "april" => Some(4),
            "may" => Some(5),
            "jun" | "june" => Some(6),
            "jul" | "july" => Some(7),
            "aug" | "august" => Some(8),
            "sep" | "september" => Some(9),
            "oct" | "october" => Some(10),
            "nov" | "november" => Some(11),
            "dec" | "december" => Some(12),
            _ => None,
        },
        CronValueKind::Weekday => match normalized.as_str() {
            "sun" | "sunday" => Some(0),
            "mon" | "monday" => Some(1),
            "tue" | "tuesday" => Some(2),
            "wed" | "wednesday" => Some(3),
            "thu" | "thursday" => Some(4),
            "fri" | "friday" => Some(5),
            "sat" | "saturday" => Some(6),
            _ => None,
        },
    };

    named_value.ok_or_else(|| {
        anyhow::anyhow!(
            "ADO cron {name} field '{}' contains unsupported value '{}'",
            field,
            value
        )
    })
}

fn validate_cron_value(value: u8, name: &str, field: &str, min: u8, max: u8) -> Result<()> {
    if !(min..=max).contains(&value) {
        bail!(
            "ADO cron {name} field '{}' contains value {}; expected {}-{}",
            field,
            value,
            min,
            max
        );
    }
    Ok(())
}

fn parse_daily_schedule(tokens: &[&str]) -> Result<FuzzySchedule> {
    let (tokens, days) = extract_daily_day_filter(tokens)?;

    if tokens.is_empty() {
        return Ok(FuzzySchedule::Daily {
            constraint: TimeConstraint::None,
            days,
        });
    }

    match tokens[0] {
        "around" => {
            if tokens.len() < 2 {
                bail!("'around' requires a time specification. Example: daily around 14:00");
            }
            let (time, offset) = parse_time_with_offset(&tokens[1..])?;
            Ok(FuzzySchedule::Daily {
                constraint: TimeConstraint::Around(time),
                days: days.with_utc_offset(offset),
            })
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

            let (start_time, start_offset) =
                parse_time_with_offset(&tokens[1..and_pos])?;
            let (end_time, end_offset) =
                parse_time_with_offset(&tokens[and_pos + 1..])?;
            if matches!(days, DayFilter::Weekdays { .. }) && start_offset != end_offset {
                bail!("weekday 'between' schedules require the same UTC offset on both times");
            }

            Ok(FuzzySchedule::Daily {
                constraint: TimeConstraint::Between(start_time, end_time),
                days: days.with_utc_offset(start_offset),
            })
        }
        "at" => bail!(
            "'daily at <time>' syntax is not supported. Use 'daily around <time>' for fuzzy scheduling within ±1 hour window"
        ),
        _ => bail!(
            "Unknown daily schedule modifier '{}'. Use 'around', 'between', or 'on weekdays'",
            tokens[0]
        ),
    }
}

fn extract_daily_day_filter<'a>(tokens: &'a [&'a str]) -> Result<(Vec<&'a str>, DayFilter)> {
    let weekday_pair = tokens
        .windows(2)
        .enumerate()
        .filter(|(_, window)| window[0] == "on" && window[1] == "weekdays")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    if weekday_pair.is_empty() {
        if tokens
            .iter()
            .any(|token| *token == "on" || *token == "weekdays")
        {
            bail!(
                "Invalid weekday modifier. Use the complete suffix 'on weekdays', for example: daily around 09:00 on weekdays"
            );
        }
        return Ok((tokens.to_vec(), DayFilter::EveryDay));
    }

    if weekday_pair.len() > 1 {
        bail!("The 'on weekdays' modifier may only be specified once");
    }

    let index = weekday_pair[0];
    let canonical_suffix = index + 2 == tokens.len();
    let issue_compatibility_order = tokens.first() == Some(&"around")
        && index + 3 == tokens.len()
        && tokens[index + 2].starts_with("utc");

    if !canonical_suffix && !issue_compatibility_order {
        bail!(
            "The 'on weekdays' modifier must be last. Put any UTC offset with the time, for example: daily around 09:00 utc-7 on weekdays"
        );
    }

    let mut remaining = tokens[..index].to_vec();
    if issue_compatibility_order {
        remaining.push(tokens[index + 2]);
    }
    if remaining
        .iter()
        .any(|token| *token == "on" || *token == "weekdays")
    {
        bail!("The 'on weekdays' modifier may only be specified once");
    }

    Ok((
        remaining,
        DayFilter::Weekdays {
            utc_offset_minutes: 0,
        },
    ))
}

fn parse_hourly_day_filter(tokens: &[&str]) -> Result<DayFilter> {
    match tokens {
        [] => Ok(DayFilter::EveryDay),
        ["on", "weekdays"] => Ok(DayFilter::Weekdays {
            utc_offset_minutes: 0,
        }),
        _ => bail!(
            "'hourly' accepts only the optional suffix 'on weekdays'. Use 'every Nh' for interval schedules."
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

    let (tokens, days) = extract_interval_day_filter(tokens)?;
    if tokens.is_empty() {
        bail!(
            "'every on weekdays' requires an hour interval. Example: every 2h on weekdays"
        );
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
        if let Some(num_str) = interval_str.strip_suffix(suffix)
            && let Ok(n) = num_str.parse::<u8>()
        {
            if tokens.len() != 1 {
                bail!(
                    "Unexpected interval schedule tokens '{}'. Use 'every {}' or append 'on weekdays' to an hour interval",
                    tokens[1..].join(" "),
                    interval_str
                );
            }
            return create_interval_schedule(n, unit, days);
        }
    }

    // Try format: "<N> <unit>" (e.g., "2 hours")
    if tokens.len() >= 2
        && let Ok(n) = tokens[0].parse::<u8>()
    {
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
        if tokens.len() != 2 {
            bail!(
                "Unexpected interval schedule tokens '{}'. Append only the optional suffix 'on weekdays' to an hour interval",
                tokens[2..].join(" ")
            );
        }
        return create_interval_schedule(n, unit_char, days);
    }

    bail!(
        "Invalid interval format '{}'. Examples: every 2h, every 5 minutes, every 3 days",
        tokens.join(" ")
    );
}

fn extract_interval_day_filter<'a>(
    tokens: &'a [&'a str],
) -> Result<(Vec<&'a str>, DayFilter)> {
    if tokens.ends_with(&["on", "weekdays"]) {
        let remaining = tokens[..tokens.len() - 2].to_vec();
        if remaining
            .iter()
            .any(|token| *token == "on" || *token == "weekdays")
        {
            bail!("The 'on weekdays' modifier may only be specified once");
        }
        return Ok((
            remaining,
            DayFilter::Weekdays {
                utc_offset_minutes: 0,
            },
        ));
    }

    if tokens
        .iter()
        .any(|token| *token == "on" || *token == "weekdays")
    {
        bail!(
            "The 'on weekdays' modifier must be the final suffix, for example: every 2h on weekdays"
        );
    }

    Ok((tokens.to_vec(), DayFilter::EveryDay))
}

fn create_interval_schedule(n: u8, unit: &str, days: DayFilter) -> Result<FuzzySchedule> {
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
            Ok(FuzzySchedule::EveryHours { interval: n, days })
        }
        "m" => {
            if matches!(days, DayFilter::Weekdays { .. }) {
                bail!("Minute intervals do not support the 'on weekdays' modifier");
            }
            // Minimum 5 minutes per GitHub Actions constraint
            if n < 5 {
                bail!(
                    "Minute interval must be at least 5 minutes (GitHub Actions constraint), got {}",
                    n
                );
            }
            Ok(FuzzySchedule::EveryMinutes(n))
        }
        "d" => {
            if matches!(days, DayFilter::Weekdays { .. }) {
                bail!("Day intervals do not support the 'on weekdays' modifier");
            }
            Ok(FuzzySchedule::EveryDays(n))
        }
        "w" => {
            if matches!(days, DayFilter::Weekdays { .. }) {
                bail!("Week intervals do not support the 'on weekdays' modifier");
            }
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
        if tokens.len() >= 2 && tokens.last().is_some_and(|t| t.starts_with("utc")) {
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
        FuzzySchedule::Daily { constraint, days } => {
            generate_daily_cron(hash, constraint, *days)
        }
        FuzzySchedule::Weekly { day, constraint } => generate_weekly_cron(hash, *day, constraint),
        FuzzySchedule::Hourly { days } => {
            let minute = hash % 60;
            format!("{} * * * {}", minute, days.cron_field())
        }
        FuzzySchedule::EveryHours { interval, days } => {
            let minute = hash % 60;
            format!("{} */{} * * {}", minute, interval, days.cron_field())
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

fn generate_daily_cron(hash: u32, constraint: &TimeConstraint, days: DayFilter) -> String {
    let total_minutes = match constraint {
        TimeConstraint::None => {
            // Scatter across full 24 hours
            hash % 1440
        }
        TimeConstraint::Around(time) => {
            // Scatter within ±60 minute window
            let target_minutes = time.to_minutes();
            let offset = (hash % 120) as i32 - 60; // Range: -60 to +59
            (target_minutes as i32 + offset).rem_euclid(1440) as u32
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
            (start_minutes + offset) % 1440
        }
    };
    let hour = total_minutes / 60;
    let minute = total_minutes % 60;
    format!(
        "{} {} * * {}",
        minute,
        hour,
        days.cron_field_for_utc_time(total_minutes)
    )
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
            FuzzySchedule::Daily {
                constraint: TimeConstraint::None,
                days: DayFilter::EveryDay
            }
        ));

        let schedule = parse_fuzzy_schedule("daily around 14:00").unwrap();
        assert_eq!(
            schedule,
            FuzzySchedule::Daily {
                constraint: TimeConstraint::Around(TimeSpec {
                    hour: 14,
                    minute: 0
                }),
                days: DayFilter::EveryDay
            },
            "daily around 14:00 should capture 14:00 in the Around variant"
        );

        let schedule = parse_fuzzy_schedule("daily between 9:00 and 17:00").unwrap();
        assert_eq!(
            schedule,
            FuzzySchedule::Daily {
                constraint: TimeConstraint::Between(
                    TimeSpec { hour: 9, minute: 0 },
                    TimeSpec {
                        hour: 17,
                        minute: 0
                    }
                ),
                days: DayFilter::EveryDay
            },
            "daily between should capture both boundary times"
        );
    }

    #[test]
    fn test_parse_daily_weekdays() {
        assert_eq!(
            parse_fuzzy_schedule("daily on weekdays").unwrap(),
            FuzzySchedule::Daily {
                constraint: TimeConstraint::None,
                days: DayFilter::Weekdays {
                    utc_offset_minutes: 0
                }
            }
        );

        assert_eq!(
            parse_fuzzy_schedule("daily around 08:00 utc-7 on weekdays").unwrap(),
            FuzzySchedule::Daily {
                constraint: TimeConstraint::Around(TimeSpec {
                    hour: 15,
                    minute: 0
                }),
                days: DayFilter::Weekdays {
                    utc_offset_minutes: -7 * 60
                }
            }
        );

        assert_eq!(
            parse_fuzzy_schedule("daily around 08:00 on weekdays utc-7").unwrap(),
            FuzzySchedule::Daily {
                constraint: TimeConstraint::Around(TimeSpec {
                    hour: 15,
                    minute: 0
                }),
                days: DayFilter::Weekdays {
                    utc_offset_minutes: -7 * 60
                }
            }
        );

        assert_eq!(
            parse_fuzzy_schedule(
                "daily between 9:00 utc-5 and 17:00 utc-5 on weekdays"
            )
            .unwrap(),
            FuzzySchedule::Daily {
                constraint: TimeConstraint::Between(
                    TimeSpec {
                        hour: 14,
                        minute: 0
                    },
                    TimeSpec {
                        hour: 22,
                        minute: 0
                    }
                ),
                days: DayFilter::Weekdays {
                    utc_offset_minutes: -5 * 60
                }
            }
        );

        let cron = schedule_expression_to_cron(
            "daily around 08:00 on weekdays utc-7",
            "test/workflow",
        )
        .unwrap();
        assert_eq!(cron, "47 14 * * 1-5");

        assert_eq!(
            schedule_expression_to_cron(
                "daily around 02:00 utc+9 on weekdays",
                "test/workflow"
            )
            .unwrap(),
            "47 16 * * 0-4"
        );
        assert_eq!(
            schedule_expression_to_cron(
                "daily around 23:00 utc-7 on weekdays",
                "test/workflow"
            )
            .unwrap(),
            "47 5 * * 2-6"
        );
    }

    #[test]
    fn test_invalid_daily_weekday_modifiers() {
        for input in [
            "daily on",
            "daily weekdays",
            "daily on weekends",
            "daily on weekdays around 09:00",
            "daily around 09:00 on weekdays on weekdays",
            "daily between 09:00 on weekdays and 17:00",
            "daily between 09:00 utc-5 and 17:00 utc-4 on weekdays",
        ] {
            let error = parse_fuzzy_schedule(input).unwrap_err();
            assert!(
                error.to_string().contains("weekday")
                    || error.to_string().contains("on weekdays"),
                "unexpected error for {input}: {error}"
            );
        }
    }

    #[test]
    fn test_parse_daily_between_weekdays_preserves_boundaries() {
        let schedule =
            parse_fuzzy_schedule("daily between 9:00 and 17:00 on weekdays").unwrap();
        assert_eq!(
            schedule,
            FuzzySchedule::Daily {
                constraint: TimeConstraint::Between(
                    TimeSpec { hour: 9, minute: 0 },
                    TimeSpec {
                        hour: 17,
                        minute: 0
                    }
                ),
                days: DayFilter::Weekdays {
                    utc_offset_minutes: 0
                }
            }
        );
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
        assert_eq!(
            schedule,
            FuzzySchedule::Weekly {
                day: Some(Weekday::Friday),
                constraint: TimeConstraint::Around(TimeSpec {
                    hour: 17,
                    minute: 0
                })
            },
            "weekly on friday around 17:00 should capture the time spec in Around variant"
        );
    }

    #[test]
    fn test_parse_hourly() {
        let schedule = parse_fuzzy_schedule("hourly").unwrap();
        assert!(matches!(
            schedule,
            FuzzySchedule::Hourly {
                days: DayFilter::EveryDay
            }
        ));
        // Cron must be "M * * * *" — every hour at a hash-scattered minute.
        // A regression that emits "0 * * * *" (fixed minute) or changes the
        // field count would silently break the scattering contract.
        let cron = generate_cron(&schedule, "test/workflow");
        let parts: Vec<&str> = cron.split_whitespace().collect();
        assert_eq!(parts.len(), 5, "Hourly cron must have 5 fields");
        let minute: u32 = parts[0].parse().expect("minute field must be a number");
        assert!(minute < 60, "Scattered minute must be in [0, 59], got {minute}");
        assert_eq!(parts[1], "*", "Hour field must be * for hourly schedule");
        assert_eq!(parts[2], "*", "Day-of-month must be * for hourly schedule");
        assert_eq!(parts[3], "*", "Month must be * for hourly schedule");
        assert_eq!(parts[4], "*", "Day-of-week must be * for hourly schedule");

        let weekday_schedule = parse_fuzzy_schedule("hourly on weekdays").unwrap();
        assert_eq!(
            generate_cron(&weekday_schedule, "test/workflow"),
            "47 * * * 1-5"
        );
    }

    #[test]
    fn test_parse_intervals() {
        assert_eq!(
            parse_fuzzy_schedule("every 2h").unwrap(),
            FuzzySchedule::EveryHours {
                interval: 2,
                days: DayFilter::EveryDay
            }
        );
        assert_eq!(
            parse_fuzzy_schedule("every 6 hours").unwrap(),
            FuzzySchedule::EveryHours {
                interval: 6,
                days: DayFilter::EveryDay
            }
        );
        assert_eq!(
            parse_fuzzy_schedule("every 5 minutes").unwrap(),
            FuzzySchedule::EveryMinutes(5)
        );
        assert_eq!(
            parse_fuzzy_schedule("every 2 days").unwrap(),
            FuzzySchedule::EveryDays(2)
        );

        let weekday_schedule = parse_fuzzy_schedule("every 2 hours on weekdays").unwrap();
        assert_eq!(
            weekday_schedule,
            FuzzySchedule::EveryHours {
                interval: 2,
                days: DayFilter::Weekdays {
                    utc_offset_minutes: 0
                }
            }
        );
        assert_eq!(
            generate_cron(&weekday_schedule, "test/workflow"),
            "47 */2 * * 1-5"
        );
    }

    #[test]
    fn test_rejects_weekdays_for_unsupported_intervals() {
        for input in [
            "every 5 minutes on weekdays",
            "every 2 days on weekdays",
            "every 1w on weekdays",
        ] {
            let error = parse_fuzzy_schedule(input).unwrap_err();
            assert!(
                error.to_string().contains("do not support"),
                "unexpected error for {input}: {error}"
            );
        }
    }

    #[test]
    fn test_parse_special_periods() {
        // Verify parse and cron structure for bi-weekly and tri-weekly schedules.
        // The key assertion is the day-of-month step (*/14 / */21) — a regression
        // that swaps these or omits the step would silently change schedule frequency.
        let bi = parse_fuzzy_schedule("bi-weekly").unwrap();
        assert!(matches!(bi, FuzzySchedule::BiWeekly));
        let bi_cron = generate_cron(&bi, "test/workflow");
        let bi_parts: Vec<&str> = bi_cron.split_whitespace().collect();
        assert_eq!(bi_parts.len(), 5, "Bi-weekly cron must have 5 fields");
        let bi_min: u32 = bi_parts[0].parse().expect("minute field must be a number");
        assert!(bi_min < 60, "Scattered minute must be in [0, 59], got {bi_min}");
        let bi_hr: u32 = bi_parts[1].parse().expect("hour field must be a number");
        assert!(bi_hr < 24, "Scattered hour must be in [0, 23], got {bi_hr}");
        assert_eq!(bi_parts[2], "*/14", "Bi-weekly step must be */14");
        assert_eq!(bi_parts[3], "*");
        assert_eq!(bi_parts[4], "*");

        let tri = parse_fuzzy_schedule("tri-weekly").unwrap();
        assert!(matches!(tri, FuzzySchedule::TriWeekly));
        let tri_cron = generate_cron(&tri, "test/workflow");
        let tri_parts: Vec<&str> = tri_cron.split_whitespace().collect();
        assert_eq!(tri_parts.len(), 5, "Tri-weekly cron must have 5 fields");
        let tri_min: u32 = tri_parts[0].parse().expect("minute field must be a number");
        assert!(tri_min < 60, "Scattered minute must be in [0, 59], got {tri_min}");
        let tri_hr: u32 = tri_parts[1].parse().expect("hour field must be a number");
        assert!(tri_hr < 24, "Scattered hour must be in [0, 23], got {tri_hr}");
        assert_eq!(tri_parts[2], "*/21", "Tri-weekly step must be */21");
        assert_eq!(tri_parts[3], "*");
        assert_eq!(tri_parts[4], "*");
    }

    #[test]
    fn test_cron_generation_deterministic() {
        let schedule = FuzzySchedule::Daily {
            constraint: TimeConstraint::None,
            days: DayFilter::EveryDay,
        };
        // FNV-1a("test/workflow") = 718355327; total_minutes = 718355327 % 1440 = 1247
        // → hour = 20, minute = 47
        let cron1 = generate_cron(&schedule, "test/workflow");
        assert_eq!(
            cron1, "47 20 * * *",
            "Cron for test/workflow should be deterministically pinned"
        );

        let cron3 = generate_cron(&schedule, "other/workflow");
        assert_ne!(
            cron1, cron3,
            "Different workflow IDs should produce different crons"
        );
    }

    #[test]
    fn test_cron_format() {
        let schedule = FuzzySchedule::Daily {
            constraint: TimeConstraint::None,
            days: DayFilter::EveryDay,
        };
        // FNV-1a("test") = 2949673445; total_minutes = 2949673445 % 1440 = 485
        // → minute = 5, hour = 8
        let cron = generate_cron(&schedule, "test");
        let parts: Vec<&str> = cron.split_whitespace().collect();
        assert_eq!(parts.len(), 5, "Cron should have 5 fields");

        let minute: u32 = parts[0].parse().expect("Minute should be a number");
        assert_eq!(
            minute, 5,
            "Minute should be 5 for \"test\" workflow (FNV-1a 2949673445 → 485 total_minutes)"
        );

        let hour: u32 = parts[1].parse().expect("Hour should be a number");
        assert_eq!(
            hour, 8,
            "Hour should be 8 for \"test\" workflow (FNV-1a 2949673445 → 485 total_minutes)"
        );

        // Daily schedule must not restrict day-of-month, month, or day-of-week.
        // A regression that adds e.g. a day-of-week constraint would silently
        // turn a daily schedule into a weekly one.
        assert_eq!(
            parts[2], "*",
            "Day-of-month should be * for a daily schedule"
        );
        assert_eq!(parts[3], "*", "Month should be * for a daily schedule");
        assert_eq!(
            parts[4], "*",
            "Day-of-week should be * for a daily schedule"
        );
    }

    #[test]
    fn test_between_equal_times_daily() {
        // When start == end the range expands to the full 24-hour day, so the
        // scattered time must NOT be pinned to the specified hour (14).
        // The cron expression must be deterministic for a given workflow key.
        let schedule = parse_fuzzy_schedule("daily between 14:00 and 14:00").unwrap();
        let cron = generate_cron(&schedule, "test/agent");
        // FNV-1a("test/agent")=196813323; offset=196813323%1440=1323;
        // scattered=(840+1323)%1440=723 → hour=12, min=3
        assert_eq!(
            cron, "3 12 * * *",
            "Same start/end time should scatter across full 24-hour day deterministically"
        );
    }

    #[test]
    fn test_between_equal_times_weekly() {
        // When start == end the range expands to the full 24-hour day, so the
        // scattered time must NOT be pinned to the specified hour (09).
        // The cron expression must be deterministic for a given workflow key.
        let schedule = parse_fuzzy_schedule("weekly on monday between 09:00 and 09:00").unwrap();
        let cron = generate_cron(&schedule, "test/agent");
        // FNV-1a("test/agent")=196813323; offset=196813323%1440=1323;
        // scattered=(540+1323)%1440=423 → hour=7, min=3; day-of-week=1 (Monday)
        assert_eq!(
            cron, "3 7 * * 1",
            "Same start/end time on Monday should scatter across full day deterministically"
        );
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

        let err = parse_fuzzy_schedule("every on weekdays").unwrap_err();
        assert!(err.to_string().contains("requires an hour interval"));
    }

    #[test]
    fn test_valid_raw_cron_is_preserved() {
        for cron in [
            "0 9 * * 1-5",
            "*/15 * * * *",
            "5,20,35,50 8-17/3 1,15 * 1-5",
            "0 18 * * Mon,Wed,Fri",
            "0 0 1 Jan,July *",
        ] {
            assert_eq!(
                schedule_expression_to_cron(cron, "ignored").unwrap(),
                cron
            );
        }
    }

    #[test]
    fn test_invalid_raw_cron_is_rejected() {
        let cases = [
            ("0 9 * *", "exactly 5 fields"),
            ("60 9 * * *", "expected 0-59"),
            ("0 24 * * *", "expected 0-23"),
            ("0 9 0 * *", "expected 1-31"),
            ("0 9 * 13 *", "expected 1-12"),
            ("0 9 * * 7", "expected 0-6"),
            ("*/0 9 * * *", "zero step"),
            ("0 9 * * 5-1", "reversed range"),
            ("0 9 * * Funday", "unsupported value"),
            ("Noon 9 * * *", "unsupported value"),
            ("$(MINUTE) 9 * * *", "unsupported characters"),
        ];

        for (cron, expected) in cases {
            let error = schedule_expression_to_cron(cron, "ignored").unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "error for {cron:?} should contain {expected:?}: {error}"
            );
        }
    }

    // ─── invalid hour interval error path ────────────────────────────────────

    #[test]
    fn test_parse_invalid_hour_interval() {
        for input in &["every 5h", "every 7h"] {
            let err = parse_fuzzy_schedule(input).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("not recommended"),
                "Error for {input} should say 'not recommended': {msg}"
            );
            assert!(
                msg.contains("Valid intervals"),
                "Error for {input} should list valid intervals: {msg}"
            );
        }
    }

    #[test]
    fn test_parse_zero_hour_interval() {
        let err = parse_fuzzy_schedule("every 0h").unwrap_err();
        assert!(
            err.to_string().contains("greater than 0"),
            "Error for 0h should mention interval must be greater than 0: {}",
            err
        );
    }
}

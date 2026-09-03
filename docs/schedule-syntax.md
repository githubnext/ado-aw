# Schedule Syntax (Fuzzy Schedule Time Syntax)

_Part of the [ado-aw documentation](../AGENTS.md)._

## Schedule Syntax (Fuzzy Schedule Time Syntax)

The `on.schedule` field supports a human-friendly fuzzy schedule syntax that automatically distributes execution times to prevent server load spikes. The syntax follows the [gh-aw schedule reference](https://github.github.com/gh-aw/reference/schedule-syntax/) and its fuzzy-schedule parser where the concepts map to Azure Pipelines. The [formal fuzzy schedule specification](https://github.com/githubnext/gh-aw/blob/main/docs/src/content/docs/specs/fuzzy-schedule-specification.md) describes the core grammar but currently trails the reference for weekday modifiers.

Schedule is configured under the `on:` key:

```yaml
on:
  schedule: daily around 14:00
```

### Daily Schedules

```yaml
schedule: daily                          # Scattered across full 24-hour day
schedule: daily around 14:00             # Within ±60 minutes of 2 PM
schedule: daily around 3pm               # 12-hour format supported
schedule: daily around midnight          # Keywords: midnight, noon
schedule: daily between 9:00 and 17:00   # Business hours (9 AM - 5 PM)
schedule: daily between 22:00 and 02:00  # Overnight (handles midnight crossing)
schedule: daily on weekdays              # Monday-Friday at a scattered time
schedule: daily around 9am on weekdays   # Monday-Friday within ±60 minutes
schedule: daily between 9:00 and 17:00 on weekdays
```

`on weekdays` is supported on daily schedules, including `around` and
`between`. The generated Azure Pipelines cron uses the day-of-week range
`1-5`.

### Weekly Schedules

```yaml
schedule: weekly                              # Any day, scattered time
schedule: weekly on monday                    # Monday, scattered time
schedule: weekly on friday around 17:00       # Friday, within ±60 min of 5 PM
schedule: weekly on wednesday between 9:00 and 12:00  # Wednesday morning
```

Valid weekdays: `sunday`, `monday`, `tuesday`, `wednesday`, `thursday`, `friday`, `saturday`.
Short aliases are also accepted: `sun`, `mon`, `tue`/`tues`, `wed`, `thu`/`thurs`, `fri`, `sat`.

### Hourly Schedules

```yaml
schedule: hourly       # Every hour at a scattered minute
schedule: every 1h     # Equivalent to hourly
schedule: every 2h     # Every 2 hours at scattered minute
schedule: every 2 hours # Long form also supported
schedule: every 6h     # Every 6 hours at scattered minute
schedule: hourly on weekdays
schedule: every 2h on weekdays
```

Valid hour intervals: 1, 2, 3, 4, 6, 8, 12 (factors of 24 for even distribution)

Weekday filtering is not supported for minute, day, week, bi-weekly, or
tri-weekly intervals.

### Minute Intervals (Fixed, Not Scattered)

```yaml
schedule: every 5 minutes     # Every 5 minutes (minimum interval)
schedule: every 5 min         # Singular/short forms also supported
schedule: every 15 minutes    # Every 15 minutes
schedule: every 30m           # Short form supported
```

Note: Minimum interval is 5 minutes (GitHub Actions/Azure DevOps constraint).
Accepted minute units: `minutes`, `minute`, `mins`, `min`, `m`.

### Special Periods

```yaml
schedule: bi-weekly       # Every 14 days at scattered time
schedule: biweekly        # No-hyphen alias
schedule: tri-weekly      # Every 21 days at scattered time
schedule: triweekly       # No-hyphen alias
schedule: every 2 days    # Every N days at scattered time
schedule: every 2d        # Short day form
schedule: every 2 weeks   # Every N weeks (converted to N×7 days) at scattered time
schedule: every 2w        # Short week form
```

Accepted day/week units: `days`, `day`, `d`, `weeks`, `week`, `w`.

### Timezone Support

All time specifications support UTC offsets for timezone conversion:

```yaml
schedule: daily around 14:00 utc+9      # 2 PM JST → 5 AM UTC
schedule: daily around 3pm utc-5        # 3 PM EST → 8 PM UTC
schedule: daily around 09:00 utc        # Bare UTC means UTC+0
schedule: daily between 9am utc+05:30 and 5pm utc+05:30  # IST business hours
schedule: daily around 08:00 utc-7 on weekdays
```

Supported offset formats: `utc`, `utc+9`, `utc-5`, `utc+05:30`, `utc-08:00`.
Keep the offset with the time and put `on weekdays` last. For compatibility
with the syntax requested in #1965, `daily around 08:00 on weekdays utc-7` is
also accepted.

Azure Pipelines YAML schedules are always evaluated in UTC. IANA timezone names
such as `America/New_York` are not supported because Azure Pipelines has no
schedule timezone field and a single UTC cron cannot preserve local wall-clock
time across daylight-saving transitions. When a fixed UTC offset crosses
midnight, the compiler rotates the cron weekday range so the schedule still
runs Monday-Friday in the requested local offset.

### How Scattering Works

The compiler uses a deterministic hash of the agent name to scatter execution times:
- Same agent always gets the same execution time (stable across recompilations)
- Different agents get different times (distributes load)
- Times stay within the specified constraints (around, between, etc.)

This prevents load spikes that occur when many workflows use convenient times like midnight or on-the-hour.

### Schedule Branch Filtering

By default, when no branches are explicitly configured, the schedule fires only on the `main` branch. To specify different branches, use the object form:

```yaml
# Default: fires only on main branch (string form)
schedule: daily around 14:00

# Custom branches: fires on listed branches (object form)
schedule:
  run: daily around 14:00
  branches:
    - main
    - release/*
```

### Raw Cron and Multiple Schedules

Use a validated five-field Azure Pipelines cron expression when fuzzy syntax
cannot express the required schedule:

```yaml
on:
  schedule: "0 9 * * 1-5"
```

The fields are `minute hour day-of-month month day-of-week`. Numeric values,
wildcards, lists, ranges, and steps are supported and validated against the ADO
field ranges. Months and weekdays also accept full English names or their first
three letters, such as `Jan` and `Mon-Fri`.

Use list form to configure multiple fuzzy and/or raw cron schedules. Each item
may specify its own branch include list; omitted branches default to `main`.

```yaml
on:
  schedule:
    - cron: daily on weekdays
    - cron: "0 9 * * 1-5"
      branches:
        - main
        - release/*
```

List items accept only `cron` and `branches`. IANA `timezone`, display-name,
batching, and branch-exclusion options are not supported.

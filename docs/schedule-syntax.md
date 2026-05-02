# Schedule Syntax (Fuzzy Schedule Time Syntax)

_Part of the [ado-aw documentation](../AGENTS.md)._

## Schedule Syntax (Fuzzy Schedule Time Syntax)

The `on.schedule` field supports a human-friendly fuzzy schedule syntax that automatically distributes execution times to prevent server load spikes. The syntax is based on the [Fuzzy Schedule Time Syntax Specification](https://github.com/githubnext/gh-aw/blob/main/docs/src/content/docs/reference/fuzzy-schedule-specification.md).

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
```

### Weekly Schedules

```yaml
schedule: weekly                              # Any day, scattered time
schedule: weekly on monday                    # Monday, scattered time
schedule: weekly on friday around 17:00       # Friday, within ±60 min of 5 PM
schedule: weekly on wednesday between 9:00 and 12:00  # Wednesday morning
```

Valid weekdays: `sunday`, `monday`, `tuesday`, `wednesday`, `thursday`, `friday`, `saturday`

### Hourly Schedules

```yaml
schedule: hourly       # Every hour at a scattered minute
schedule: every 2h     # Every 2 hours at scattered minute
schedule: every 6h     # Every 6 hours at scattered minute
```

Valid hour intervals: 1, 2, 3, 4, 6, 8, 12 (factors of 24 for even distribution)

### Minute Intervals (Fixed, Not Scattered)

```yaml
schedule: every 5 minutes     # Every 5 minutes (minimum interval)
schedule: every 15 minutes    # Every 15 minutes
schedule: every 30m           # Short form supported
```

Note: Minimum interval is 5 minutes (GitHub Actions/Azure DevOps constraint).

### Special Periods

```yaml
schedule: bi-weekly    # Every 14 days at scattered time
schedule: tri-weekly   # Every 21 days at scattered time
schedule: every 2 days # Every 2 days at scattered time
```

### Timezone Support

All time specifications support UTC offsets for timezone conversion:

```yaml
schedule: daily around 14:00 utc+9      # 2 PM JST → 5 AM UTC
schedule: daily around 3pm utc-5        # 3 PM EST → 8 PM UTC
schedule: daily between 9am utc+05:30 and 5pm utc+05:30  # IST business hours
```

Supported offset formats: `utc+9`, `utc-5`, `utc+05:30`, `utc-08:00`

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

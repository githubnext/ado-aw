Create an ado-aw workflow named `stale-critical-work-items`.

Run it daily around 09:00. Read open Severity 1 and Severity 2 work items in
`SyntheticProject\Platform` in the `synthetic-org` Azure DevOps organization.
When an item has had no update for 7 days, propose one concise comment asking
for status. Comment on at most 3 work items per run. Use `synthetic-read-sc` for
reads and `synthetic-write-sc` for the safe-output write path. If no items
qualify, explicitly use noop.

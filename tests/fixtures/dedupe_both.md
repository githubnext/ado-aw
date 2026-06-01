---
name: "Dedupe Both"
description: "Gate AND runtime-import resolver active — download must land in BOTH jobs"
on:
  pr:
    branches:
      include: [main]
    filters:
      title: "*[agent]*"
---

## Dedupe Both

Used by `test_both_features_active_downloads_bundle_in_both_jobs`.

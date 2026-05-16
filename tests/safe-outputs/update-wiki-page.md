---
name: "Daily safe-output smoke: update-wiki-page"
description: "Exercises the update-wiki-page safe output once a day"
on:
  schedule: daily around 03:00
target: standalone
engine:
  id: copilot
  model: gpt-5-mini
  timeout-minutes: 15
permissions:
  read: agent-playground-read
  write: agent-playground-write
safe-outputs:
  update-wiki-page:
    wiki-name: $(permaWikiName)
    max: 1
    include-stats: false
---

## Daily smoke for update-wiki-page

You are a smoke test. The variable group `ado-aw-daily-smoke` provides
a perma wiki page at `$(permaWikiPagePath)`. Call exactly one safe-output
tool: `update-wiki-page`. Use these literal values (no improvisation):

- path: "$(permaWikiPagePath)"
- content: "ado-aw daily smoke exercising the update-wiki-page safe output. Last updated by build $(Build.BuildId)."
- comment: "ado-aw daily smoke build $(Build.BuildId)"

Do not call any other tool. After the safe output is emitted, stop.

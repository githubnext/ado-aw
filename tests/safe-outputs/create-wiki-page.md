---
name: "Daily safe-output smoke: create-wiki-page"
description: "Exercises the create-wiki-page safe output once a day"
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
  create-wiki-page:
    wiki-name: $(permaWikiName)
    path-prefix: "/ado-aw-smoke"
    max: 1
    include-stats: false
---

## Daily smoke for create-wiki-page

You are a smoke test. Call exactly one safe-output tool: `create-wiki-page`.
Use these literal values (no improvisation):

- path: "/ado-aw-smoke-$(Build.BuildId)-create-wiki-page"
- content: "ado-aw daily smoke exercising the create-wiki-page safe output. Build ID $(Build.BuildId). This page will be deleted by the weekly janitor."
- comment: "ado-aw daily smoke build $(Build.BuildId)"

Do not call any other tool. After the safe output is emitted, stop.

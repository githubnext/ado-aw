---
name: "Daily safe-output smoke: reply-to-pr-comment"
description: "Exercises the reply-to-pr-comment safe output once a day"
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
  reply-to-pr-comment:
    comment-prefix: "ado-aw-smoke: "
    max: 1
---

## Daily smoke for reply-to-pr-comment

You are a smoke test. The variable group `ado-aw-daily-smoke` provides
the perma PR at `$(permaPullRequestId)` and a perma thread on that PR
at `$(permaThreadId)`. Call exactly one safe-output tool:
`reply-to-pr-comment`. Use these literal values (no improvisation):

- pull_request_id: $(permaPullRequestId)
- thread_id: $(permaThreadId)
- content: "ado-aw-smoke-$(Build.BuildId)-reply-to-pr-comment exercising the reply-to-pr-comment safe output for build $(Build.BuildId)."
- repository: "self"

Do not call any other tool. After the safe output is emitted, stop.

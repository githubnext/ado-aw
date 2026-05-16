---
name: "Daily safe-output smoke: upload-workitem-attachment"
description: "Exercises the upload-workitem-attachment safe output once a day"
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
  upload-workitem-attachment:
    max-file-size: 1048576
    allowed-extensions:
      - .txt
    comment-prefix: "ado-aw-smoke: "
    max: 1
setup:
  - bash: |
      set -euo pipefail
      mkdir -p "$BUILD_ARTIFACTSTAGINGDIRECTORY"
      printf 'ado-aw-smoke build %s\n' "$BUILD_BUILDID" \
        > "$BUILD_ARTIFACTSTAGINGDIRECTORY/ado-aw-smoke.txt"
      ls -la "$BUILD_ARTIFACTSTAGINGDIRECTORY/ado-aw-smoke.txt"
    displayName: "Setup: write smoke attachment payload"
---

## Daily smoke for upload-workitem-attachment

You are a smoke test. The setup job has written
`$(Build.ArtifactStagingDirectory)/ado-aw-smoke.txt`. The variable group
`ado-aw-daily-smoke` provides a perma work item at
`$(permaWorkItemId)`. Call exactly one safe-output tool:
`upload-workitem-attachment`. Use these literal values (no
improvisation):

- work_item_id: $(permaWorkItemId)
- file_path: "$(Build.ArtifactStagingDirectory)/ado-aw-smoke.txt"
- comment: "ado-aw-smoke-$(Build.BuildId)-upload-workitem-attachment"

Do not call any other tool. After the safe output is emitted, stop.

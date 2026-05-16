---
name: "Daily safe-output smoke: upload-pipeline-artifact"
description: "Exercises the upload-pipeline-artifact safe output once a day"
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
  upload-pipeline-artifact:
    max-file-size: 1048576
    allowed-extensions:
      - .txt
    name-prefix: "ado-aw-smoke-"
    require-unique-names: false
    max: 1
setup:
  - bash: |
      set -euo pipefail
      mkdir -p "$BUILD_ARTIFACTSTAGINGDIRECTORY"
      printf 'ado-aw-smoke build %s\n' "$BUILD_BUILDID" \
        > "$BUILD_ARTIFACTSTAGINGDIRECTORY/ado-aw-smoke.txt"
      ls -la "$BUILD_ARTIFACTSTAGINGDIRECTORY/ado-aw-smoke.txt"
    displayName: "Setup: write smoke artifact payload"
---

## Daily smoke for upload-pipeline-artifact

You are a smoke test. The setup job has written
`$(Build.ArtifactStagingDirectory)/ado-aw-smoke.txt`. Call exactly one
safe-output tool: `upload-pipeline-artifact`. Use these literal values
(no improvisation):

- artifact_name: "ado-aw-smoke-$(Build.BuildId)-upload-pipeline-artifact"
- file_path: "$(Build.ArtifactStagingDirectory)/ado-aw-smoke.txt"

Do not call any other tool. After the safe output is emitted, stop.

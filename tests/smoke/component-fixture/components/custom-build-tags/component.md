---
safe-outputs:
  jobs:
    candidate-job-build-tag:
      description: Add the candidate jobs-style proof tag to the current build.
      output: Candidate build-tag proposal accepted.
      max: 1
      inputs:
        proof:
          description: Deterministic candidate-smoke proof value.
          type: choice
          options: [candidate-smoke]
          required: true
      steps:
        - bash: |
            set -euo pipefail
            jq -c '.items[] | select(.type == "candidate-job-build-tag")' \
              "$ADO_AW_AGENT_OUTPUT" |
            while IFS= read -r item; do
              proof="$(printf '%s' "$item" | jq -er '.proof')"
              test "$proof" = "candidate-smoke"

              tag="ado-aw-custom-job-$(Build.BuildId)"
              if [ "$ADO_AW_SAFE_OUTPUTS_STAGED" = "true" ]; then
                printf 'STAGED: would add build tag %s\n' "$tag"
              else
                printf '##vso[build.addbuildtag]%s\n' "$tag"
              fi
            done
          displayName: Add jobs-style candidate build tag
---

This tool is a deterministic candidate-smoke probe. Call it only when the
consumer workflow explicitly requests `proof: candidate-smoke`.

---
safe-outputs:
  scripts:
    candidate-script-build-tag:
      description: Add the candidate scripts-style proof tag to the current build.
      max: 1
      run: node components/custom-build-tags/tag-build-script.js
      inputs:
        proof:
          type: choice
          options: [candidate-smoke]
          required: true
      env:
        SYSTEM_ACCESSTOKEN: System.AccessToken
  jobs:
    candidate-job-build-tag:
      description: Add the candidate jobs-style proof tag to the current build.
      max: 1
      inputs:
        proof:
          type: choice
          options: [candidate-smoke]
          required: true
      steps:
        - bash: |
            set -euo pipefail
            : > "$ADO_AW_SAFE_OUTPUT_RESULTS"
            while IFS= read -r proposal; do
              proposal_id="$(printf '%s' "$proposal" | jq -er '.proposal_id')"
              proof="$(printf '%s' "$proposal" | jq -er '.proof')"
              test "$proof" = "candidate-smoke"

              tag="ado-aw-custom-job-$(Build.BuildId)"
              printf '##vso[build.addbuildtag]%s\n' "$tag"
              jq -cn \
                --arg proposal_id "$proposal_id" \
                --arg tag "$tag" \
                '{schema_version:1, proposal_id:$proposal_id, status:"success", message:("added jobs-style build tag " + $tag), data:{tag:$tag}}' \
                >> "$ADO_AW_SAFE_OUTPUT_RESULTS"
            done < "$ADO_AW_SAFE_OUTPUT_PROPOSALS"
          displayName: Add jobs-style candidate build tag
---

These tools are deterministic candidate-smoke probes. Call each only when the
consumer workflow explicitly requests `proof: candidate-smoke`.

---
name: "GitHub Issue App Test Agent"
description: "Fixture exercising shared GitHub App issue safe outputs"
engine:
  id: copilot
  github-app-token:
    app-id: 1234567
    private-key: GITHUB_APP_PRIVATE_KEY
    owner: octo-org
    repositories:
      - octo-repo
    permissions:
      contents: read
      issues: read
safe-outputs:
  create-github-issue:
    target-repo: octo-org/octo-repo
    require-temporary-id: true
    max: 1
  set-github-issue-type:
    target-repo: octo-org/octo-repo
    allowed:
      - Bug
      - Feature
      - Task
    max: 5
---

Create an issue and assign its native type.

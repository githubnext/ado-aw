---
name: "GitHub App Token Agent"
description: "Tests GitHub App-backed Copilot engine authentication"
engine:
  id: copilot
  github-app-token:
    app-id: "1234567"
    owner: octo-org
    repositories:
    - octo-repo
safe-outputs:
  noop:
---

## GitHub App Token Agent

This agent exercises the `engine.github-app-token` configuration, which causes
the compiler to emit a token-mint step before the Copilot run and a best-effort
revocation step after it.

---
name: "Runtime Prompt Default Agent"
description: "Fixture exercising the default (runtime) prompt-injection path"
---

## Runtime Prompt Test Body

This body must NOT appear verbatim in the compiled YAML when
`inlined-imports` is unset (the default). Instead, `prompt.js` reads it
from the workspace at pipeline runtime and the body is delivered to
the agent via `/tmp/awf-tools/agent-prompt.md`.

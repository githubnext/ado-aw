---
name: 'My "special": agent with quotes (1ES)'
description: "Fixture covering YAML escaping for embedded \" and : in name (1ES target)"
target: 1es
permissions:
  read: my-read-arm-connection
---

## Tricky-Name Agent (1ES)

Fixture used by `tests/compiler_tests.rs` to verify that agent names
containing embedded double quotes and colons survive YAML escaping for
the 1ES target, where the stage's `displayName:` is generated via
`{{ agent_display_name }}` and the top-level pipeline `name:` is
generated via `{{ pipeline_display_name }}`.

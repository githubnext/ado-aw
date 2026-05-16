---
name: 'My "special": agent with quotes'
description: "Fixture covering YAML escaping for embedded \" and : in name"
target: standalone
permissions:
  read: my-read-arm-connection
---

## Tricky-Name Agent

Fixture used by `tests/compiler_tests.rs` to verify that agent names
containing embedded double quotes and colons survive YAML escaping in
both the top-level `name:` line (via `{{ pipeline_name }}`) and
any `displayName:` positions (via `{{ agent_display_name }}`).

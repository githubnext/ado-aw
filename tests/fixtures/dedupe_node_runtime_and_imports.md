---
name: "Dedupe Node Runtime And Imports"
description: "User pins Node 20 via runtime; ado-script must NOT override on PATH"
runtimes:
  node:
    version: "20.x"
---

## Dedupe Node Runtime And Imports

Used by `test_node_runtime_install_orders_after_ado_script_so_user_version_wins`.

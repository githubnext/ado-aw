For this synthetic scenario, use:

```yaml
permissions:
  read: synthetic-read-sc
  write: synthetic-write-sc
safe-outputs:
  comment-on-work-item:
    max: 3
    target: "SyntheticProject\\Platform"
```

Do not add `wit_update_work_item` or `wit_add_work_item_comment` to the agent's
direct Azure DevOps MCP allow-list.


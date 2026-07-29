The following configuration shapes are supported for this synthetic scenario:

```yaml
tools:
  azure-devops:
    org: synthetic-org
    toolsets: [core, work-items]
    allowed:
      - wit_get_work_item
      - wit_get_work_items_batch_by_ids
      - search_workitem
permissions:
  read: synthetic-read-sc
  write: synthetic-write-sc
safe-outputs:
  comment-on-work-item:
    max: 3
    target: "SyntheticProject\\Platform"
```

The workflow body must tell the agent to call `noop` when no work item
qualifies.

# Target Platforms

_Part of the [ado-aw documentation](../AGENTS.md)._

## Target Platforms

The `target` field in the front matter determines the output format and execution environment for the compiled pipeline.

### `standalone` (default)

Generates a self-contained Azure DevOps pipeline with:
- Full 3-job pipeline: `Agent` → `Detection` → `Execution`
- AWF (Agentic Workflow Firewall) L7 domain whitelisting via Squid proxy + Docker
- MCP Gateway (MCPG) for MCP routing with SafeOutputs HTTP backend
- Setup/teardown job support
- All safe output features (create-pull-request, create-work-item, etc.)

This is the recommended target for maximum flexibility and security controls.

### `1es`

Generates a pipeline that extends the 1ES Unofficial Pipeline Template:
- Uses `templateContext.type: buildJob` with Copilot CLI + AWF + MCPG (same execution model as standalone)
- Integrates with 1ES SDL scanning and compliance tools
- Full 3-job pipeline: Agent → Detection → Execution
- Requires 1ES Pipeline Templates repository access

Example:
```yaml
target: 1es
```

When using `target: 1es`, the pipeline will extend `1es/1ES.Unofficial.PipelineTemplate.yml@1ESPipelinesTemplates`.

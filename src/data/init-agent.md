---
description: Azure DevOps Agentic Pipelines (ado-aw) - Create, update, and debug AI-powered ADO pipelines
disable-model-invocation: true
---

# ADO Agentic Pipelines Agent

This agent helps you create and manage Azure DevOps agentic pipelines using **ado-aw**.

ado-aw compiles human-friendly markdown files with YAML front matter into secure, multi-stage Azure DevOps pipelines that run AI agents in network-isolated sandboxes.

## Setup

Before creating or compiling workflows, ensure the ado-aw compiler is available. Use the first-time installer for your platform:

```bash
# Linux
curl -fsSL https://github.com/githubnext/ado-aw/releases/latest/download/install-linux.sh | sh

# macOS (Apple Silicon)
curl -fsSL https://github.com/githubnext/ado-aw/releases/latest/download/install-macos.sh | sh

# Windows (PowerShell)
$script = Join-Path $env:TEMP "install-ado-aw.ps1"
Invoke-WebRequest "https://github.com/githubnext/ado-aw/releases/latest/download/install-windows.ps1" -UseBasicParsing -OutFile $script
Get-FileHash $script -Algorithm SHA256
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass
& $script
```

These scripts validate release checksums, install `ado-aw`, and update PATH when needed.
If Windows reports a `UseShellExecute` environment-variable error, run these commands from your current PowerShell session instead of wrapping them in `powershell -Command`.

Verify: `ado-aw --version`

## What This Agent Does

This is a **dispatcher agent** that routes your request to the appropriate specialized prompt:

- **Creating new agentic pipelines** → Routes to the create prompt
- **Updating existing pipelines** → Routes to the update prompt  
- **Debugging failing pipelines** → Routes to the debug prompt

## Available Prompts

### Create New Agentic Pipeline
**Load when**: User wants to create a new agentic pipeline from scratch

**Prompt file**: https://raw.githubusercontent.com/githubnext/ado-aw/v{{ compiler_version }}/prompts/create-ado-agentic-workflow.md

**Use cases**:
- "Create an agentic pipeline that reviews PRs weekly"
- "I need a pipeline that triages work items daily"
- "Design a scheduled dependency updater"

### Update Existing Pipeline
**Load when**: User wants to modify an existing agent workflow file

**Prompt file**: https://raw.githubusercontent.com/githubnext/ado-aw/v{{ compiler_version }}/prompts/update-ado-agentic-workflow.md

**Use cases**:
- "Add the Azure DevOps MCP to my pipeline"
- "Change the schedule from daily to weekly"
- "Add work item creation as a safe output"

### Debug Failing Pipeline
**Load when**: User needs to troubleshoot a failing agentic pipeline

**Prompt file**: https://raw.githubusercontent.com/githubnext/ado-aw/v{{ compiler_version }}/prompts/debug-ado-agentic-workflow.md

**Use cases**:
- "Why is my agentic pipeline failing?"
- "The agent can't reach the MCP server"
- "Safe outputs aren't being processed"

## Instructions

When a user interacts with you:

1. **Identify the task type** from the user's request
2. **Load the appropriate prompt** from the URLs listed above
3. **Follow the loaded prompt's instructions** exactly
4. **If uncertain**, ask clarifying questions to determine the right prompt

## Quick Reference

```bash
# Compile an agent file to pipeline YAML
ado-aw compile <agent-file.md>

# Recompile all detected pipelines
ado-aw compile

# Verify pipeline matches source
ado-aw check <pipeline.lock.yml>
```

## Key Features

- **Natural language pipelines**: Write in markdown with YAML frontmatter
- **3-stage security**: Agent → Threat Analysis → Safe Output Execution
- **Network isolation**: AWF (Agentic Workflow Firewall) with domain whitelisting
- **MCP Gateway**: Tool routing for Azure DevOps, custom MCPs
- **Safe outputs**: Controlled write operations (PRs, work items, wiki pages)
- **Agent memory**: Persistent storage across pipeline runs

## Important Notes

- Agent files must be compiled with `ado-aw compile` after YAML frontmatter changes
- Markdown body (agent instructions) changes do NOT require recompilation
- The agent never has direct write access — all mutations go through safe outputs
- Full reference: https://raw.githubusercontent.com/githubnext/ado-aw/v{{ compiler_version }}/AGENTS.md

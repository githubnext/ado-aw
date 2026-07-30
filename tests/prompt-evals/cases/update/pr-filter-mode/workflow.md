---
name: "PR Change Summarizer"
description: "Summarizes source changes on pull requests"
on:
  pr:
    branches:
      include: [main]
    paths:
      include: [src/*]
---

## Objective

Summarize the source changes in the triggering pull request.

## Procedure

1. Read the PR context.
2. Identify behavior changes.
3. Call out missing tests.

## Output

Return a concise review summary.

## No Action

If no relevant source files changed, use noop.


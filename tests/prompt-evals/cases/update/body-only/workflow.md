---
name: "Repository TODO Summary"
description: "Summarizes TODO and FIXME markers in the checked-out repository"
---

## Objective

Find TODO and FIXME comments in the repository.

## Procedure

1. Search tracked source files.
2. Group matches by file.
3. Ignore generated files.

## Output

Return a concise bullet list with the path and marker text.

## No Action

If there are no matches, state that no TODO or FIXME comments were found.


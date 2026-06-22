#!/usr/bin/env bash
# doctor.sh - verify prerequisites for the ado-aw Agency plugin.
#
# Checks:
#   1. `ado-aw` is on PATH (else point to the install docs).
#   2. `gh` and `az` availability + auth (advisory - only needed for ADO-facing
#      skills: debug-workflow, audit-build, manage-lifecycle).
#
# Exit code is non-zero only when a hard requirement (ado-aw) is missing.
set -euo pipefail

ok()   { printf '  \033[32m✓\033[0m %s\n' "$1"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$1"; }
err()  { printf '  \033[31m✗\033[0m %s\n' "$1"; }

hard_fail=0

echo "ado-aw plugin doctor"
echo

# 1. ado-aw (hard requirement)
if command -v ado-aw >/dev/null 2>&1; then
  version="$(ado-aw --version 2>/dev/null || echo 'unknown')"
  ok "ado-aw found: ${version}"
else
  err "ado-aw not found on PATH"
  echo "    Install it:"
  echo "      Linux:   curl -fsSL https://github.com/githubnext/ado-aw/releases/latest/download/install-linux.sh | sh"
  echo "      macOS:   curl -fsSL https://github.com/githubnext/ado-aw/releases/latest/download/install-macos.sh | sh"
  echo "    Docs: https://github.com/githubnext/ado-aw/releases/latest"
  hard_fail=1
fi

# 2. ADO auth helpers (advisory)
if command -v gh >/dev/null 2>&1; then
  if gh auth status >/dev/null 2>&1; then
    ok "gh authenticated"
  else
    warn "gh found but not authenticated (run 'gh auth login' for GitHub-backed flows)"
  fi
else
  warn "gh not found (optional; needed for some GitHub-backed flows)"
fi

if command -v az >/dev/null 2>&1; then
  if az account show >/dev/null 2>&1; then
    ok "az authenticated"
  else
    warn "az found but not logged in (run 'az login' for ADO trace/audit/lifecycle skills)"
  fi
else
  warn "az not found (optional; ADO-facing skills can also use an explicit PAT)"
fi

echo
if [ "${hard_fail}" -ne 0 ]; then
  err "Missing required tool(s). Install ado-aw before using this plugin."
  exit 1
fi
ok "All required prerequisites satisfied."

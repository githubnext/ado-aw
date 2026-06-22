#!/usr/bin/env pwsh
# doctor.ps1 - verify prerequisites for the ado-aw Agency plugin.
#
# Checks:
#   1. `ado-aw` is on PATH (else point to the install docs).
#   2. `gh` and `az` availability + auth (advisory - only needed for ADO-facing
#      skills: debug-workflow, audit-build, manage-lifecycle).
#
# Exit code is non-zero only when a hard requirement (ado-aw) is missing.
$ErrorActionPreference = 'Stop'

function Write-Ok($m)   { Write-Host "  [ok]   $m" -ForegroundColor Green }
function Write-Warn($m) { Write-Host "  [warn] $m" -ForegroundColor Yellow }
function Write-Err($m)  { Write-Host "  [fail] $m" -ForegroundColor Red }

$hardFail = $false

Write-Host "ado-aw plugin doctor"
Write-Host ""

# 1. ado-aw (hard requirement)
if (Get-Command ado-aw -ErrorAction SilentlyContinue) {
    $version = (ado-aw --version 2>$null)
    if (-not $version) { $version = 'unknown' }
    Write-Ok "ado-aw found: $version"
} else {
    Write-Err "ado-aw not found on PATH"
    Write-Host '    Install it (PowerShell):'
    Write-Host '      powershell -ExecutionPolicy Bypass -NoProfile -Command "iwr https://github.com/githubnext/ado-aw/releases/latest/download/install-windows.ps1 -UseBasicParsing | iex"'
    Write-Host '    Docs: https://github.com/githubnext/ado-aw/releases/latest'
    $hardFail = $true
}

# 2. ADO auth helpers (advisory)
if (Get-Command gh -ErrorAction SilentlyContinue) {
    gh auth status *> $null
    if ($LASTEXITCODE -eq 0) { Write-Ok "gh authenticated" }
    else { Write-Warn "gh found but not authenticated (run 'gh auth login' for GitHub-backed flows)" }
} else {
    Write-Warn "gh not found (optional; needed for some GitHub-backed flows)"
}

if (Get-Command az -ErrorAction SilentlyContinue) {
    az account show *> $null
    if ($LASTEXITCODE -eq 0) { Write-Ok "az authenticated" }
    else { Write-Warn "az found but not logged in (run 'az login' for ADO trace/audit/lifecycle skills)" }
} else {
    Write-Warn "az not found (optional; ADO-facing skills can also use an explicit PAT)"
}

Write-Host ""
if ($hardFail) {
    Write-Err "Missing required tool(s). Install ado-aw before using this plugin."
    exit 1
}
Write-Ok "All required prerequisites satisfied."

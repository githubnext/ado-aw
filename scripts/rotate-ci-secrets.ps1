[CmdletBinding()]
param(
    [string] $Organization = "outlookweb",
    [string] $Project = "Test"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Definition ids in outlookweb/Test. Keep in sync with tests/smoke/REGISTERED.md.
# An empty list is skipped with a warning; fill it in when the definition is
# registered.
#
# GITHUB_TOKEN (Copilot CLI auth) lives on the smoke lane only. No smoke case
# files GitHub issues, so ADO_AW_GITHUB_TOKEN is not provisioned anywhere; add
# the lane id to a new list here if a case ever adopts `create-github-issue`.
$laneDefinitionIds = @(2875)
$executorDefinitionIds = @(2884)
$triggerDefinitionIds = @(2885)
$allDefinitionIds = @(
    $laneDefinitionIds
    $executorDefinitionIds
    $triggerDefinitionIds
) | Sort-Object -Unique

function Set-AdoAwSecret {
    param(
        [Parameter(Mandatory)]
        [string] $Name,

        [Parameter(Mandatory)]
        [string] $Value,

        [int[]] $DefinitionIds = @()
    )

    if ($DefinitionIds.Count -eq 0) {
        Write-Warning "Skipping ${Name}: no definition ids are registered for it yet."
        return
    }

    Write-Host "Setting $Name on definitions $($DefinitionIds -join ',')..."
    $Value | & cargo run --quiet -- secrets set $Name --value-stdin `
        --org $Organization `
        --project $Project `
        --definition-ids ($DefinitionIds -join ",")

    if ($LASTEXITCODE -ne 0) {
        throw "Failed to set $Name on definitions $($DefinitionIds -join ',')."
    }
}

Push-Location $PSScriptRoot
try {
    $copilotToken = Read-Host "New Copilot GITHUB_TOKEN" -MaskInput
    if ([string]::IsNullOrWhiteSpace($copilotToken)) {
        throw "The Copilot GITHUB_TOKEN cannot be empty."
    }

    Set-AdoAwSecret `
        -Name "GITHUB_TOKEN" `
        -Value $copilotToken `
        -DefinitionIds $laneDefinitionIds

    Remove-Variable copilotToken

    $issuesToken = Read-Host `
        "New issues-only GitHub PAT (leave blank to preserve existing issue tokens)" `
        -MaskInput

    if ([string]::IsNullOrWhiteSpace($issuesToken)) {
        Write-Host "Preserving existing issue-reporting tokens."
    }
    else {
        Set-AdoAwSecret `
            -Name "EXECUTOR_E2E_GITHUB_TOKEN" `
            -Value $issuesToken `
            -DefinitionIds $executorDefinitionIds

        Set-AdoAwSecret `
            -Name "TRIGGER_E2E_GITHUB_TOKEN" `
            -Value $issuesToken `
            -DefinitionIds $triggerDefinitionIds
    }

    Remove-Variable issuesToken

    Write-Host ""
    Write-Host "Verifying variable names and secret flags (values are never printed)..."
    & cargo run --quiet -- secrets list `
        --org $Organization `
        --project $Project `
        --definition-ids ($allDefinitionIds -join ",")

    if ($LASTEXITCODE -ne 0) {
        throw "Secret rotation succeeded, but verification failed."
    }
}
finally {
    Remove-Variable copilotToken -ErrorAction SilentlyContinue
    Remove-Variable issuesToken -ErrorAction SilentlyContinue
    Pop-Location
}

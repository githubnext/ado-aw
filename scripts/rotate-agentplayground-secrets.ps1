[CmdletBinding()]
param(
    [string] $Organization = "msazuresphere",
    [string] $Project = "AgentPlayground"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$copilotDefinitionIds = @(2545, 2546, 2547, 2548, 2549, 2554, 2555, 2556, 2558, 2564, 2565, 2567)

# The issues-only PAT is still read by the committed release-owned locks that
# definitions 2549 and 2558 run today, under the legacy
# `ado-aw-debug.create-issue` variable name. Drop this list, and the
# ADO_AW_DEBUG_GITHUB_TOKEN block below, when those definitions are deleted at
# the end of the smoke cutover (tests/smoke/REGISTERED.md step 11).
#
# No smoke case files GitHub issues any more, so ADO_AW_GITHUB_TOKEN (the name
# #1670 introduced for the public `create-github-issue` safe output) is not
# provisioned anywhere. Add the lane id here if a case ever adopts it.
$legacyReporterDefinitionIds = @(2549, 2558)
$executorDefinitionIds = @(2550)
$triggerDefinitionIds = @(2551)
$allDefinitionIds = @(
    $copilotDefinitionIds
    $executorDefinitionIds
    $triggerDefinitionIds
) | Sort-Object -Unique

function Set-AdoAwSecret {
    param(
        [Parameter(Mandatory)]
        [string] $Name,

        [Parameter(Mandatory)]
        [string] $Value,

        [Parameter(Mandatory)]
        [int[]] $DefinitionIds
    )

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
        -DefinitionIds $copilotDefinitionIds

    Remove-Variable copilotToken

    $issuesToken = Read-Host `
        "New issues-only GitHub PAT (leave blank to preserve existing issue tokens)" `
        -MaskInput

    if ([string]::IsNullOrWhiteSpace($issuesToken)) {
        Write-Host "Preserving existing issue-reporting tokens."
    }
    else {
        Set-AdoAwSecret `
            -Name "ADO_AW_GITHUB_TOKEN" `
            -Value $issuesToken `
            -DefinitionIds $legacyReporterDefinitionIds

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

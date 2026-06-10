param(
    [string]$Version = "latest",
    [string]$Repo = "githubnext/ado-aw"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $IsWindows) {
    throw "This installer is for Windows only."
}

$assetName = "ado-aw-windows-x64.exe"
$downloadBase = if ($Version -eq "latest") {
    "https://github.com/$Repo/releases/latest/download"
}
else {
    "https://github.com/$Repo/releases/download/$Version"
}

$tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("ado-aw-install-" + [guid]::NewGuid().ToString("N"))
New-Item -Path $tempDir -ItemType Directory -Force | Out-Null

try {
    $assetPath = Join-Path $tempDir $assetName
    $checksumsPath = Join-Path $tempDir "checksums.txt"

    Invoke-WebRequest -Uri "$downloadBase/$assetName" -OutFile $assetPath
    Invoke-WebRequest -Uri "$downloadBase/checksums.txt" -OutFile $checksumsPath

    $checksumLine = Select-String -Path $checksumsPath -Pattern " $assetName$" | Select-Object -First 1
    if (-not $checksumLine) {
        throw "Unable to find checksum entry for $assetName."
    }

    $expectedHash = (($checksumLine.Line -split '\s+')[0]).ToLowerInvariant()
    $actualHash = (Get-FileHash -Path $assetPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($expectedHash -ne $actualHash) {
        throw "Checksum verification failed for $assetName."
    }

    $installDir = Join-Path $env:LOCALAPPDATA "Programs\ado-aw\bin"
    New-Item -Path $installDir -ItemType Directory -Force | Out-Null

    $destination = Join-Path $installDir "ado-aw.exe"
    Copy-Item -Path $assetPath -Destination $destination -Force

    $userPath = [Environment]::GetEnvironmentVariable("Path", [EnvironmentVariableTarget]::User)
    $pathEntries = @()
    if ($userPath) {
        $pathEntries = $userPath -split ';'
    }

    $alreadyInPath = $pathEntries | Where-Object { $_.TrimEnd('\') -eq $installDir.TrimEnd('\') }
    if (-not $alreadyInPath) {
        $newPath = if ($userPath) { "$userPath;$installDir" } else { $installDir }
        [Environment]::SetEnvironmentVariable("Path", $newPath, [EnvironmentVariableTarget]::User)
        if (-not ($env:Path -split ';' | Where-Object { $_.TrimEnd('\') -eq $installDir.TrimEnd('\') })) {
            $env:Path = "$env:Path;$installDir"
        }
        Write-Host "Added $installDir to PATH for the current user."
    }

    Write-Host "Installed ado-aw to $destination"
    Write-Host "Run: ado-aw --version"
}
finally {
    Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}

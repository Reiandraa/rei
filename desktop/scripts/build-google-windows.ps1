param([Parameter(Mandatory = $true)][string]$CredentialsPath)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
# Read the publisher's downloaded Desktop credential in memory. Never print it,
# copy it into the repository, or use credentials from someone else's app.
try {
    $credentialDocument = Get-Content -LiteralPath $CredentialsPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $publicConfig = Get-Content -LiteralPath (Join-Path $projectRoot 'email-publisher.json') -Raw -Encoding UTF8 | ConvertFrom-Json
} catch { throw 'Could not read the publisher credential JSON. No credential values were logged.' }
$installedCredential = $credentialDocument.installed
if (-not $installedCredential -or $credentialDocument.web) { throw 'Use a Google Desktop app credential, not a Web or service-account credential.' }
if ($installedCredential.client_id -cne $publicConfig.google_client_id) { throw 'The Desktop credential belongs to a different client than REI email-publisher.json.' }
$desktopSecret = [string]$installedCredential.client_secret
if ($desktopSecret -cnotmatch '^[A-Za-z0-9._~-]{16,256}$') { throw 'The Desktop client credential is missing or malformed. No build started.' }
$previousCredential = [Environment]::GetEnvironmentVariable('REI_GOOGLE_DESKTOP_CLIENT_SECRET', 'Process')
try {
    [Environment]::SetEnvironmentVariable('REI_GOOGLE_DESKTOP_CLIENT_SECRET', $desktopSecret, 'Process')
    Write-Host 'Building REI with its own Google Desktop registration. Credential values are not printed.'
    Push-Location $projectRoot
    try {
        & npm.cmd run tauri -- build --no-bundle --config tauri.windows-preview.conf.json
        if ($LASTEXITCODE -ne 0) { throw 'REI build failed. No release was published.' }
    } finally { Pop-Location }
} finally {
    [Environment]::SetEnvironmentVariable('REI_GOOGLE_DESKTOP_CLIENT_SECRET', $previousCredential, 'Process')
    $desktopSecret = $null
    $installedCredential = $null
    $credentialDocument = $null
}

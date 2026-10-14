# SPDX-License-Identifier: MPL-2.0

# Exercise the real config loader in a child shell with a fake Android runner.
# No SDK, Gradle, network, device, or existing app data is touched.
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$fixture = Join-Path $repo ('target\android-run-tests-' + [Guid]::NewGuid().ToString('N'))
$loader = Join-Path $PSScriptRoot 'run-local.ps1'
$passed = 0
$utf8 = [Text.UTF8Encoding]::new($false)
function Write-Config([string]$file, [string]$origin, [string]$issuer = $origin) {
    [IO.File]::WriteAllLines((Join-Path $fixture $file), @(
        "koruAccountOrigin=$origin",
        "koruAccountIssuer=$issuer",
        'koruGoogleServerClientId=fixture.apps.googleusercontent.com',
        'koruAccountChannel=internal'
    ), $utf8)
}
function Check([string]$name, [int]$exitCode, [string[]]$contains, [string[]]$arguments) {
    $ErrorActionPreference = 'Continue'
    $output = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $loader -AndroidDirectory $fixture @arguments 2>&1
    $actualExit = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    $text = $output -join "`n"
    if ($actualExit -ne $exitCode) { throw "$name expected exit $exitCode, got ${actualExit}: $text" }
    foreach ($expected in $contains) {
        if (-not $text.Contains($expected)) { throw "$name missing '${expected}': $text" }
    }
    if ($exitCode -eq 2 -and $text.Contains('DELEGATED:')) { throw "$name reached the runner after invalid configuration" }
    $script:passed++
    Write-Host "PASS $name"
}
$previousPort = $env:ANDROID_RUN_LOCAL_API_PORT
$previousOrigin = $env:ORG_GRADLE_PROJECT_koruAccountOrigin
try {
    [IO.Directory]::CreateDirectory($fixture) | Out-Null
    [IO.File]::WriteAllText((Join-Path $fixture 'koru.bat'), @'
@echo off
echo DELEGATED:%*
echo ORIGIN:%ORG_GRADLE_PROJECT_koruAccountOrigin%
echo ISSUER:%ORG_GRADLE_PROJECT_koruAccountIssuer%
echo CHANNEL:%ORG_GRADLE_PROJECT_koruAccountChannel%
if defined ANDROID_RUN_LOCAL_API_PORT (echo PORT:%ANDROID_RUN_LOCAL_API_PORT%) else (echo PORT:none)
echo LOGS:%ANDROID_RUN_LOCAL_ACCOUNT_LOGS%
exit /b 0
'@, $utf8)
    Write-Config '.env.local' 'http://localhost:8080'
    Check 'production file required; no local fallback' 2 @('Missing .env.production') @('run-prod')
    Write-Config '.env.production' 'https://api.koru.do'
    $env:ANDROID_RUN_LOCAL_API_PORT = '9999'
    $env:ORG_GRADLE_PROJECT_koruAccountOrigin = 'https://stale.example.test'
    Check 'production selects hosted settings and clears inherited forwarding' 0 @(
        'loaded from .env.production', 'ORIGIN:https://api.koru.do', 'ISSUER:https://api.koru.do',
        'CHANNEL:internal', 'PORT:none', 'LOGS:1', 'DELEGATED:run --device phone'
    ) @('run-prod', '--device', 'phone')
    if ($env:ANDROID_RUN_LOCAL_API_PORT -ne '9999' -or $env:ORG_GRADLE_PROJECT_koruAccountOrigin -ne 'https://stale.example.test') {
        throw 'Child config changed the calling environment'
    }
    Check 'local configuration and device args preserved' 0 @('loaded from .env.local', 'PORT:8080', 'DELEGATED:run --device tablet') @('run-local', '--device', 'tablet')
    Check 'local explicit origin chooses matching forwarding port' 0 @('PORT:9090') @('run-local', '-PkoruAccountOrigin=http://localhost:9090')
    Check 'production refuses local override' 2 @('run-prod requires a hosted HTTPS koruAccountOrigin') @('run-prod', '-PkoruAccountOrigin=http://localhost:9090')
    Check 'production refuses local issuer override' 2 @('run-prod requires a hosted HTTPS koruAccountIssuer') @('run-prod', '--project-prop=koruAccountIssuer=http://localhost:8080')
    Check 'production refuses HTTPS loopback' 2 @('run-prod requires a hosted HTTPS koruAccountOrigin') @('run-prod', '-P', 'koruAccountOrigin=https://localhost')
    Check 'hosted override skips forwarding and reaches Gradle' 0 @('PORT:none', '-PkoruAccountOrigin=https://staging.example.test') @('run-prod', '-PkoruAccountOrigin=https://staging.example.test')
    Check 'invalid client override rejected' 2 @('must be the Google web OAuth client ID') @('run-prod', '-PkoruGoogleServerClientId=invalid')
    Write-Config '.env.production' 'https://api.koru.do' 'https://api.koru.do/wrong-path'
    Check 'invalid issuer rejected before runner' 2 @('koruAccountIssuer must be an HTTPS origin') @('run-prod')
    Write-Config '.env.production' 'https://api.koru.do'
    [IO.File]::AppendAllText((Join-Path $fixture '.env.production'), "koruAccountChannel=internal`n", $utf8)
    Check 'duplicate config rejected' 2 @('Duplicate .env.production setting') @('run-prod')
    Write-Config '.env.production' 'https://api.koru.do'
    [IO.File]::WriteAllText((Join-Path $fixture 'koru.bat'), "@echo off`r`nexit /b 7`r`n", $utf8)
    Check 'runner failure propagated' 7 @('loaded from .env.production') @('run-prod')
    Write-Host "$passed configured-run checks passed."
} finally {
    $env:ANDROID_RUN_LOCAL_API_PORT = $previousPort
    $env:ORG_GRADLE_PROJECT_koruAccountOrigin = $previousOrigin
    $resolvedFixture = [IO.Path]::GetFullPath($fixture)
    $targetRoot = [IO.Path]::GetFullPath((Join-Path $repo 'target')) + [IO.Path]::DirectorySeparatorChar
    if (-not $resolvedFixture.StartsWith($targetRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Fixture escaped target directory' }
    if (Test-Path -LiteralPath $resolvedFixture) { Remove-Item -LiteralPath $resolvedFixture -Recurse -Force }
}

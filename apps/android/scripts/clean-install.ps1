# SPDX-License-Identifier: MPL-2.0

# Production boundary: imported tests exercise the module with fake ports and
# never execute this file, adb, Gradle, or a device uninstall.
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'CleanInstall.psm1') -Force
$androidRoot = Split-Path -Parent $PSScriptRoot
$adbPath = $env:ADB
$target = $env:DEVICE_WANTED
$generateContracts = -not [string]::IsNullOrEmpty($env:GENERATE_CONTRACTS)
$apk = Join-Path $androidRoot 'app\build\outputs\apk\debug\app-debug.apk'
$adb = {
    param([string[]] $Arguments)
    $ErrorActionPreference = 'Continue'
    $lines = @(& $adbPath @Arguments 2>&1 | ForEach-Object { "$_" })
    $code = $LASTEXITCODE
    [pscustomobject]@{ ExitCode = $code; Lines = $lines }
}.GetNewClosure()
$build = {
    Push-Location $androidRoot
    try {
        $ErrorActionPreference = 'Continue'
        Invoke-KoruCleanInstallBuild -Launcher (Join-Path $androidRoot 'koru.bat') `
            -GenerateContracts $generateContracts -RunLauncher {
                param($Launcher, [string[]] $Arguments)
                & $Launcher @Arguments | Out-Host
                return $LASTEXITCODE
            }
    } finally { Pop-Location }
}.GetNewClosure()
$result = Invoke-KoruCleanInstall -Target $target -Apk $apk -Adb $adb -Build $build `
    -ApkExists { param($path) Test-Path -LiteralPath $path -PathType Leaf } `
    -Report { param($message) Write-Host $message }
exit $result.ExitCode

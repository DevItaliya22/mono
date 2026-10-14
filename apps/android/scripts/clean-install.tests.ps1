# SPDX-License-Identifier: MPL-2.0

# No Pester/SDK/Gradle dependency and no filesystem mutations. All adb/build
# calls are scriptblock ports recording their arguments in memory.
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'CleanInstall.psm1') -Force
$script:passed = 0

function Assert-Equal($actual, $expected, [string] $because) {
    if ($actual -cne $expected) { throw "${because}: expected '$expected', received '$actual'" }
}
function Assert-True([bool] $value, [string] $because) {
    if (-not $value) { throw $because }
}
function Test-Case([string] $name, [scriptblock] $body) {
    & $body
    $script:passed += 1
    Write-Host "PASS $name"
}
function New-Rig {
    $state = @{
        Calls = [System.Collections.Generic.List[object]]::new()
        Reports = [System.Collections.Generic.List[string]]::new()
        Devices = @('List of devices attached', 'phone-1 device product:pixel model:Pixel_9 device:tokay', 'phone-2 device product:pixel model:Pixel_8 device:shiba')
        AfterBuildDevices = $null
        Listings = 0
        ListingExit = 0
        BuildExit = 0
        ApkPresent = $true
        Packages = @('package:do.koru.app')
        PackageExit = 0
        UninstallExit = 0
        UninstallLines = @('Success')
        InstallExit = 0
        InstallLines = @('Performing Streamed Install', 'Success')
    }
    $state.Adb = {
        param([string[]] $Arguments)
        $state.Calls.Add([pscustomobject]@{ Kind = 'adb'; Arguments = $Arguments })
        if ($Arguments[0] -ceq 'devices') {
            $state.Listings += 1
            $lines = if ($state.Listings -gt 1 -and $null -ne $state.AfterBuildDevices) { $state.AfterBuildDevices } else { $state.Devices }
            return [pscustomobject]@{ ExitCode = $state.ListingExit; Lines = $lines }
        }
        if ($Arguments[0] -cne '-s') { throw 'A device call omitted -s' }
        switch ($Arguments[2]) {
            'shell' { return [pscustomobject]@{ ExitCode = $state.PackageExit; Lines = $state.Packages } }
            'uninstall' { return [pscustomobject]@{ ExitCode = $state.UninstallExit; Lines = $state.UninstallLines } }
            'install' { return [pscustomobject]@{ ExitCode = $state.InstallExit; Lines = $state.InstallLines } }
            default { throw 'Unexpected fake adb operation' }
        }
    }.GetNewClosure()
    $state.Build = {
        $state.Calls.Add([pscustomobject]@{ Kind = 'build'; Arguments = @(); Reports = $state.Reports.ToArray() })
        return $state.BuildExit
    }.GetNewClosure()
    $state.Exists = { param($path) return $state.ApkPresent }.GetNewClosure()
    $state.Report = { param($message) $state.Reports.Add($message) }.GetNewClosure()
    return $state
}
function Run-Rig($rig, [string] $target = 'phone-1') {
    Invoke-KoruCleanInstall -Target $target -Apk 'fixture-app-debug.apk' `
        -Adb $rig.Adb -Build $rig.Build -ApkExists $rig.Exists -Report $rig.Report
}
function Assert-NoRemoval($rig) {
    Assert-Equal @($rig.Calls | Where-Object { $_.Kind -eq 'adb' -and $_.Arguments -contains 'uninstall' }).Count 0 'must not uninstall'
    Assert-Equal @($rig.Calls | Where-Object { $_.Kind -eq 'adb' -and $_.Arguments -contains 'install' }).Count 0 'must not install'
}

Test-Case 'exact serial pins every device call and builds before removal' {
    $rig = New-Rig
    $result = Run-Rig $rig
    Assert-Equal $result.ExitCode 0 'success'
    Assert-Equal $result.Serial 'phone-1' 'selected serial'
    Assert-Equal $result.Model 'Pixel_9' 'selected model'
    Assert-True $result.Removed 'successful uninstall reported'
    Assert-Equal (($rig.Calls | ForEach-Object { if ($_.Kind -eq 'build') { 'build' } else { $_.Arguments -join '|' } }) -join ';') `
        'devices|-l;build;devices|-l;-s|phone-1|shell|pm|list|packages|do.koru.app;-s|phone-1|uninstall|do.koru.app;-s|phone-1|install|fixture-app-debug.apk' 'safe ordered effects'
    Assert-True ($rig.Reports[0].Contains('phone-1') -and $rig.Reports[0].Contains('Pixel_9')) 'target is named before build/removal'
    $beforeBuild = @($rig.Calls | Where-Object Kind -eq 'build')[0].Reports
    Assert-True ($beforeBuild[0].Contains('phone-1') -and $beforeBuild[0].Contains('Pixel_9')) 'the build starts only after target reporting'
    $report = $rig.Reports -join ' '
    foreach ($word in @('Store', 'trust', 'seed', 'vault')) { Assert-True $report.Contains($word) "removal report names $word" }
}
Test-Case 'complete model resolves one exact serial and spaced mDNS serial survives as one argument' {
    $rig = New-Rig
    $rig.Devices = @('List of devices attached', 'adb-phone ABC (2)._adb-tls-connect._tcp device product:pixel model:Pixel_9_Pro device:caiman')
    $result = Run-Rig $rig 'Pixel 9 Pro'
    Assert-Equal $result.ExitCode 0 'exact displayed model accepted'
    Assert-Equal $result.Serial 'adb-phone ABC (2)._adb-tls-connect._tcp' 'complete mDNS serial preserved'
    foreach ($call in @($rig.Calls | Where-Object { $_.Kind -eq 'adb' -and $_.Arguments[0] -eq '-s' })) {
        Assert-Equal $call.Arguments[1] $result.Serial 'same serial argument'
    }
}
Test-Case 'missing explicit target does not enumerate or build' {
    $rig = New-Rig
    $result = Run-Rig $rig ''
    Assert-Equal $result.Code 'clean_install_target_required' 'explicit target required'
    Assert-Equal $rig.Calls.Count 0 'no tool side effects'
}
Test-Case 'unknown alias substring offline and unauthorized targets never fall back' {
    foreach ($target in @('tablet', 'Pixel', 'phone', 'offline-phone', 'unauthorized-phone')) {
        $rig = New-Rig
        $rig.Devices += @('offline-phone offline model:Pixel_7', 'unauthorized-phone unauthorized model:Pixel_6')
        $result = Run-Rig $rig $target
        Assert-Equal $result.Code 'clean_install_target_not_connected' 'unknown target rejected'
        Assert-Equal $rig.Calls.Count 1 'only read device list'
        Assert-NoRemoval $rig
    }
}
Test-Case 'duplicate complete models are ambiguous but an exact serial remains definitive' {
    $rig = New-Rig
    $rig.Devices = @('one device model:Pixel_9', 'two device model:Pixel_9')
    $result = Run-Rig $rig 'Pixel_9'
    Assert-Equal $result.Code 'clean_install_target_ambiguous' 'ambiguous model rejected'
    Assert-Equal $rig.Calls.Count 1 'no build after ambiguity'
    Assert-NoRemoval $rig
    $rig = New-Rig
    $rig.Devices = @('one device model:Pixel_9', 'two device model:Pixel_9')
    Assert-Equal (Run-Rig $rig 'two').ExitCode 0 'exact serial accepted'
}
Test-Case 'no ready device duplicate serial and malformed serial fail closed' {
    foreach ($devices in @(
        @('List of devices attached'),
        @('phone-1 device model:Pixel_9', 'phone-1 device model:Pixel_9'),
        @('phone"1 device model:Pixel_9')
    )) {
        $rig = New-Rig
        $rig.Devices = $devices
        $result = Run-Rig $rig
        Assert-True ($result.ExitCode -ne 0) 'invalid selection rejected'
        Assert-Equal $rig.Calls.Count 1 'no build after invalid listing'
        Assert-NoRemoval $rig
    }
}
Test-Case 'an exact serial without model metadata remains explicit' {
    $rig = New-Rig
    $rig.Devices = @('phone-1 device')
    $result = Run-Rig $rig
    Assert-Equal $result.ExitCode 0 'serial did not require model metadata'
    Assert-Equal $result.Model '(model unavailable)' 'missing metadata is explicit'
}
Test-Case 'listing build and missing APK failures preserve the installed app' {
    foreach ($failure in @('listing', 'build', 'apk')) {
        $rig = New-Rig
        switch ($failure) {
            'listing' { $rig.ListingExit = 1 }
            'build' { $rig.BuildExit = 1 }
            'apk' { $rig.ApkPresent = $false }
        }
        $result = Run-Rig $rig
        Assert-True ($result.ExitCode -ne 0) 'prerequisite failure rejected'
        Assert-NoRemoval $rig
        Assert-True (-not $result.Removed) 'no removal claimed'
    }
}
Test-Case 'a disconnected selected device never becomes a different attached phone' {
    foreach ($after in @(@('phone-2 device model:Pixel_8'), @('phone-1 device model:Different'))) {
        $rig = New-Rig
        $rig.AfterBuildDevices = $after
        $result = Run-Rig $rig
        Assert-Equal $result.Code 'clean_install_target_changed' 'changed target refused'
        Assert-NoRemoval $rig
    }
}
Test-Case 'verified absent app skips uninstall and installs without claiming removal' {
    $rig = New-Rig
    $rig.Packages = @('package:do.koru.app.other')
    $result = Run-Rig $rig
    Assert-Equal $result.Code 'clean_install_complete' 'absent package still installs'
    Assert-Equal $result.ExitCode 0 'absence is a successful initial install'
    Assert-True (-not $result.Removed) 'no removal is claimed'
    Assert-Equal @($rig.Calls | Where-Object { $_.Kind -eq 'adb' -and $_.Arguments -contains 'uninstall' }).Count 0 'absent package is not uninstalled'
    Assert-Equal @($rig.Calls | Where-Object { $_.Kind -eq 'adb' -and $_.Arguments -contains 'install' }).Count 1 'current APK installs'
    Assert-True (($rig.Reports -join ' ').Contains('Nothing was removed')) 'absence is explicitly reported'
}
Test-Case 'an unsuccessful package query is never interpreted as absence' {
    foreach ($failure in @('exit', 'output')) {
        $rig = New-Rig
        if ($failure -eq 'exit') { $rig.PackageExit = 1 } else { $rig.Packages = @('Error: device unavailable') }
        $result = Run-Rig $rig
        Assert-Equal $result.Code 'clean_install_package_query_failed' 'query failure is not absence'
        Assert-NoRemoval $rig
    }
}
Test-Case 'the rebuild inherits selected public Gradle properties and forwards only build options' {
    $values = @{
        ORG_GRADLE_PROJECT_koruAccountOrigin = 'https://fixture-account.example.test'
        ORG_GRADLE_PROJECT_koruAccountIssuer = 'https://fixture-issuer.example.test'
        ORG_GRADLE_PROJECT_koruGoogleServerClientId = 'fixture-web.apps.googleusercontent.com'
        ORG_GRADLE_PROJECT_koruAccountChannel = 'fixture-internal'
    }
    $saved = @{}
    foreach ($name in $values.Keys) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        [Environment]::SetEnvironmentVariable($name, $values[$name], 'Process')
    }
    try {
        $observed = @{}
        $launcher = {
            param($Launcher, [string[]] $Arguments)
            $observed.Launcher = $Launcher
            $observed.Arguments = $Arguments
            foreach ($name in $values.Keys) { $observed[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
            return 0
        }.GetNewClosure()
        $result = Invoke-KoruCleanInstallBuild -Launcher 'fixture-koru.bat' -GenerateContracts $true -RunLauncher $launcher
        Assert-Equal $result 0 'fake rebuild succeeded'
        Assert-Equal $observed.Launcher 'fixture-koru.bat' 'intended launcher selected'
        Assert-Equal ($observed.Arguments -join '|') 'build|--contracts' 'bounded build arguments'
        foreach ($name in $values.Keys) { Assert-Equal $observed[$name] $values[$name] "public property $name reaches rebuild" }
    } finally {
        foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    }
}
Test-Case 'uninstall failure including exit-zero Failure never installs or claims removal' {
    foreach ($exitCode in @(0, 1)) {
        $rig = New-Rig
        $rig.UninstallExit = $exitCode
        $rig.UninstallLines = @('Failure [DELETE_FAILED_INTERNAL_ERROR]')
        $result = Run-Rig $rig
        Assert-Equal $result.Code 'clean_install_uninstall_failed' 'uninstall failure surfaced'
        Assert-True (-not $result.Removed) 'no false removal claim'
        Assert-Equal @($rig.Calls | Where-Object { $_.Kind -eq 'adb' -and $_.Arguments -contains 'install' }).Count 0 'failed removal does not install'
    }
}
Test-Case 'install failure explicitly reports that local data has already been removed' {
    $rig = New-Rig
    $rig.InstallExit = 1
    $rig.InstallLines = @('Failure [INSTALL_FAILED_INVALID_APK]')
    $result = Run-Rig $rig
    Assert-Equal $result.Code 'clean_install_install_failed_after_removal' 'install failure phase identified'
    Assert-True $result.Removed 'confirmed removal remains visible'
    Assert-True ($rig.Reports[-1].Contains('local data were removed')) 'failure report names irreversible result'
}

Write-Host "$script:passed clean-install fake-port tests passed. No adb, Gradle or device was used."

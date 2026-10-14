# SPDX-License-Identifier: MPL-2.0

Set-StrictMode -Version Latest

function Get-ReadyAndroidDevices {
    param([string[]] $Lines)
    foreach ($line in $Lines) {
        if ($line -notmatch '^(?<serial>.+?)\s+device(?:\s+(?<detail>.*))?$') { continue }
        $serial = $Matches.serial.TrimEnd()
        $detail = if ($Matches.ContainsKey('detail')) { $Matches.detail } else { '' }
        # adb mDNS serials may contain spaces. No shell quoting characters are
        # accepted; every later invocation carries this resolved serial as one argument.
        if ($serial -notmatch '^[A-Za-z0-9._:+() -]+$') { throw 'clean_install_device_listing_invalid' }
        $model = '(model unavailable)'
        if ($detail -match '(?:^|\s)model:(\S+)') { $model = $Matches[1] }
        [pscustomobject]@{ Serial = $serial; Model = $model }
    }
}

function Invoke-KoruCleanInstall {
    [CmdletBinding()]
    param(
        [AllowEmptyString()][string] $Target,
        [Parameter(Mandatory)][string] $Apk,
        [Parameter(Mandatory)][scriptblock] $Adb,
        [Parameter(Mandatory)][scriptblock] $Build,
        [Parameter(Mandatory)][scriptblock] $ApkExists,
        [Parameter(Mandatory)][scriptblock] $Report
    )
    $serial = $null
    $model = $null
    $removed = $false
    try {
        if ([string]::IsNullOrWhiteSpace($Target)) { throw 'clean_install_target_required' }
        $listing = & $Adb -Arguments @('devices', '-l')
        if ($listing.ExitCode -ne 0) { throw 'clean_install_device_listing_failed' }
        $devices = @(Get-ReadyAndroidDevices -Lines $listing.Lines)
        $duplicates = @($devices | Group-Object -Property Serial | Where-Object Count -gt 1)
        if ($duplicates.Count -gt 0) { throw 'clean_install_device_listing_invalid' }
        $candidates = @($devices | Where-Object { $_.Serial -ceq $Target })
        if ($candidates.Count -eq 0) {
            $candidates = @($devices | Where-Object {
                $_.Model -cne '(model unavailable)' -and
                ($_.Model -ceq $Target -or $_.Model.Replace('_', ' ') -ceq $Target)
            })
        }
        if ($candidates.Count -eq 0) { throw 'clean_install_target_not_connected' }
        if ($candidates.Count -ne 1) { throw 'clean_install_target_ambiguous' }
        $serial = $candidates[0].Serial
        $model = $candidates[0].Model
        & $Report "[koru] clean-install target: $serial (model: $model)."
        & $Report '[koru] Uninstall will erase this app installation: Store content, trust.db, legacy trust, device seeds and the account vault. Building before removal.'
        if ((& $Build) -ne 0) { throw 'clean_install_build_failed' }
        if (-not (& $ApkExists $Apk)) { throw 'clean_install_apk_missing' }

        # Revalidate the same selected serial after a potentially long build.
        # An attached replacement never becomes the target by being the sole device.
        $listing = & $Adb -Arguments @('devices', '-l')
        if ($listing.ExitCode -ne 0) { throw 'clean_install_device_listing_failed' }
        $current = @(Get-ReadyAndroidDevices -Lines $listing.Lines | Where-Object { $_.Serial -ceq $serial })
        if ($current.Count -ne 1 -or $current[0].Model -cne $model) { throw 'clean_install_target_changed' }
        $presence = & $Adb -Arguments @('-s', $serial, 'shell', 'pm', 'list', 'packages', 'do.koru.app')
        if ($presence.ExitCode -ne 0) { throw 'clean_install_package_query_failed' }
        $packages = @($presence.Lines | ForEach-Object { $_.Trim() } | Where-Object { $_ })
        if (@($packages | Where-Object { $_ -notmatch '^package:[A-Za-z0-9_.]+$' }).Count -gt 0) {
            throw 'clean_install_package_query_failed'
        }
        if ($packages -ccontains 'package:do.koru.app') {
            & $Report "[koru] Removing do.koru.app from $serial (model: $model)."
            $uninstall = & $Adb -Arguments @('-s', $serial, 'uninstall', 'do.koru.app')
            $uninstallLines = @($uninstall.Lines | ForEach-Object { $_.Trim() } | Where-Object { $_ })
            if ($uninstall.ExitCode -ne 0 -or $uninstallLines.Count -ne 1 -or $uninstallLines[0] -cne 'Success') {
                throw 'clean_install_uninstall_failed'
            }
            $removed = $true
            & $Report '[koru] Removed the app and its local Store, trust database, device seed wrappers and Keystore entries, and account refresh vault. Hosted account/Profile data is unchanged.'
        } else {
            & $Report '[koru] do.koru.app is already absent on the selected device. Nothing was removed; installing the current APK.'
        }
        $install = & $Adb -Arguments @('-s', $serial, 'install', $Apk)
        $installLines = @($install.Lines | ForEach-Object { $_.Trim() } | Where-Object { $_ })
        if ($install.ExitCode -ne 0 -or $installLines.Count -eq 0 -or $installLines[-1] -cne 'Success' -or
            @($installLines | Where-Object { $_ -match '^(Failure|Error)' }).Count -gt 0) {
            if ($removed) { throw 'clean_install_install_failed_after_removal' }
            throw 'clean_install_install_failed'
        }
        & $Report "[koru] Installed the current APK on $serial (model: $model). The app has not been launched."
        return [pscustomobject]@{ ExitCode = 0; Code = 'clean_install_complete'; Serial = $serial; Model = $model; Removed = $removed }
    } catch {
        $code = $_.Exception.Message
        $messages = @{
            clean_install_target_required = 'An explicit --device exact serial or model is required.'
            clean_install_device_listing_failed = 'adb could not list connected devices. Nothing was removed.'
            clean_install_device_listing_invalid = 'adb returned an invalid or duplicate device identity. Nothing was removed.'
            clean_install_target_not_connected = 'No ready device matches that exact serial or complete model. Aliases, substrings and saved defaults are not used.'
            clean_install_target_ambiguous = 'Several ready devices have that exact model. Use an exact serial. Nothing was removed.'
            clean_install_build_failed = 'The current APK build failed. The installed app is untouched.'
            clean_install_apk_missing = 'The build did not produce the expected APK. The installed app is untouched.'
            clean_install_target_changed = 'The selected device disconnected or its reported model changed during the build. Nothing was removed.'
            clean_install_package_query_failed = 'The installed package could not be verified. Nothing was removed.'
            clean_install_uninstall_failed = 'adb did not confirm app removal. Installation was stopped; inspect the selected device before retrying.'
            clean_install_install_failed_after_removal = 'The app and its local data were removed, but the new APK did not install. Retry install on the same exact serial.'
            clean_install_install_failed = 'The package was already absent and nothing was removed, but the new APK did not install.'
        }
        if (-not $messages.ContainsKey($code)) {
            $code = 'clean_install_tool_failed'
            $messages[$code] = if ($removed) { 'A tool failed after app removal. Local data has been erased; inspect the selected device.' } else { 'A tool failed. The clean-install did not complete; inspect the selected device before retrying.' }
        }
        & $Report "[koru] ${code}: $($messages[$code])"
        $exit = 1
        if ($code -in @('clean_install_target_required', 'clean_install_target_not_connected', 'clean_install_target_ambiguous')) { $exit = 2 }
        return [pscustomobject]@{ ExitCode = $exit; Code = $code; Serial = $serial; Model = $model; Removed = $removed }
    }
}

function Invoke-KoruCleanInstallBuild {
    param(
        [Parameter(Mandatory)][string] $Launcher,
        [bool] $GenerateContracts,
        [Parameter(Mandatory)][scriptblock] $RunLauncher
    )
    # The existing process environment (including ORG_GRADLE_PROJECT_* public
    # configuration) is inherited by the launcher. No prior -P option is reconstructed.
    $arguments = @('build')
    if ($GenerateContracts) { $arguments += '--contracts' }
    & $RunLauncher -Launcher $Launcher -Arguments $arguments
}

Export-ModuleMember -Function Invoke-KoruCleanInstall, Invoke-KoruCleanInstallBuild

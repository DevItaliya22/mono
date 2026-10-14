# SPDX-License-Identifier: MPL-2.0

param(
    [string]$AndroidDirectory
)

# Keep this a simple script parameter block: advanced common parameters would
# consume Gradle's standalone -P as PowerShell's PipelineVariable parameter.
$RunArguments = @($args)

# This child process owns the Gradle environment. Neither cmd.exe nor PowerShell
# evaluates values from the file, and the caller's shell is left unchanged.
$ErrorActionPreference = 'Stop'
try {
    if ($RunArguments.Count -eq 0 -or $RunArguments[0] -notin @('run-local', 'run-prod')) {
        throw 'Use koru.bat run-local or run-prod [arguments].'
    }
    $production = $RunArguments[0] -eq 'run-prod'
    $envName = if ($production) { '.env.production' } else { '.env.local' }
    $envFile = Join-Path $AndroidDirectory $envName
    if (-not (Test-Path -LiteralPath $envFile -PathType Leaf)) {
        throw "Missing $envName. Copy .env.example to $envName and fill its required Android settings before running again."
    }
    $names = @('koruAccountOrigin', 'koruAccountIssuer', 'koruGoogleServerClientId', 'koruAccountChannel')
    # Optional because a phone without it is a phone with no Cloud attached,
    # which is a supported way to run: the account, peer sync and local work
    # are unaffected. Present, it is the same public regional policy the
    # desktop reads from the API's own dotenv, and the phone needs it to
    # verify a sync credential at all.
    #
    # koruBlobByteOrigins is optional for the same reason, one setting along: a build with no local
    # object worker to name is a correctly configured build, and every build
    # anyone ships is one. Present, it names the stack on this machine that a
    # phone may send blob bytes to, beside the compiled-in per-region set — the
    # phone's half of what KORU_CLOUD_BLOB_BYTE_ORIGINS does for the desktop,
    # because nothing sets an environment variable in an Android process.
    $optional = @('koruAccountCredentialRollout', 'koruBlobByteOrigins')
    $known = $names + $optional
    $values = @{}
    $lineNumber = 0
    foreach ($line in Get-Content -LiteralPath $envFile -Encoding UTF8) {
        $lineNumber++
        $text = $line.Trim()
        if ($text.Length -eq 0 -or $text.StartsWith('#')) { continue }
        $separator = $text.IndexOf('=')
        if ($separator -lt 1) { throw "Invalid $envName entry at line $lineNumber; expected name=value." }
        $name = $text.Substring(0, $separator).Trim()
        if ($known -cnotcontains $name) { throw "Unknown $envName setting at line $lineNumber; use the names in .env.example." }
        if ($values.ContainsKey($name)) { throw "Duplicate $envName setting: $name." }
        $value = $text.Substring($separator + 1).Trim()
        if ($value.StartsWith('"') -or $value.StartsWith("'")) {
            if ($value.Length -lt 2 -or $value[$value.Length - 1] -ne $value[0]) {
                throw "Unclosed quote for $name in $envName."
            }
            $value = $value.Substring(1, $value.Length - 2)
        }
        if ([string]::IsNullOrWhiteSpace($value) -or $value -match 'REPLACE_ME') {
            throw "Fill $name in $envName before running Android."
        }
        if ($value.Length -gt 2048 -or $value -match '\s|[\x00-\x1f\x7f]') {
            throw "Invalid value for $name in $envName."
        }
        $values[$name] = $value
    }
    foreach ($name in $names) {
        if (-not $values.ContainsKey($name)) { throw "Missing $name in $envName." }
    }
    $forward = @($RunArguments | Select-Object -Skip 1)
    $effective = $values.Clone()
    # Gradle command-line properties override its environment. Forward the port
    # of the API actually built into the app, including an explicit -P override.
    for ($index = 0; $index -lt $forward.Count; $index++) {
        $property = ''
        $argument = $forward[$index]
        if ($argument -ceq '-P' -or $argument -ceq '--project-prop') {
            $index++
            if ($index -lt $forward.Count) { $property = $forward[$index] }
        } elseif ($argument.StartsWith('-P')) {
            $property = $argument.Substring(2)
        } elseif ($argument.StartsWith('--project-prop=')) {
            $property = $argument.Substring(15)
        }
        $separator = $property.IndexOf('=')
        if ($separator -gt 0) {
            $name = $property.Substring(0, $separator)
            if ($known -ccontains $name) {
                $effective[$name] = $property.Substring($separator + 1)
            }
        }
    }
    foreach ($name in @('koruAccountOrigin', 'koruAccountIssuer')) {
        $uri = $null
        if (-not [Uri]::TryCreate($effective[$name], [UriKind]::Absolute, [ref]$uri) -or
            ($uri.Scheme -ne 'https' -and -not ($uri.Scheme -eq 'http' -and $uri.IsLoopback)) -or
            $uri.Port -lt 1 -or $uri.UserInfo -or $uri.Query -or $uri.Fragment -or $uri.AbsolutePath -ne '/') {
            throw "$name must be an HTTPS origin or a localhost HTTP origin, without a path, query or credentials."
        }
        if ($production -and ($uri.Scheme -ne 'https' -or $uri.IsLoopback)) {
            throw "run-prod requires a hosted HTTPS $name, including command-line overrides."
        }
    }
    $origin = [Uri]$effective.koruAccountOrigin
    if ($effective.koruGoogleServerClientId.Length -gt 256 -or
        $effective.koruGoogleServerClientId -cnotmatch '^[A-Za-z0-9._-]+\.apps\.googleusercontent\.com$') {
        throw 'koruGoogleServerClientId must be the Google web OAuth client ID.'
    }
    if ($effective.koruAccountChannel -cnotmatch '^[a-z0-9-]{1,40}$') {
        throw 'koruAccountChannel must contain 1-40 lowercase letters, digits or hyphens.'
    }
    # The same rule the device enforces, said here so a person reads it at the
    # moment they typed the value rather than as a refused transfer on a
    # phone. The device is still the authority: `ByteOrigins` admits a
    # loopback, private-range or link-local address and nothing else, and a
    # value it refuses leaves the compiled-in production set standing. A build
    # that could name a public host is the thing that set exists to prevent,
    # so run-prod refuses the setting outright.
    if ($effective.ContainsKey('koruBlobByteOrigins') -and $effective.koruBlobByteOrigins) {
        if ($production) {
            throw 'koruBlobByteOrigins names a development stack and has no place in run-prod.'
        }
        foreach ($entry in $effective.koruBlobByteOrigins.Split(',')) {
            $byteOrigin = $entry.Trim()
            if (-not $byteOrigin) { continue }
            $uri = $null
            if (-not [Uri]::TryCreate($byteOrigin, [UriKind]::Absolute, [ref]$uri) -or
                $uri.Scheme -notin @('http', 'https') -or
                $uri.UserInfo -or $uri.Query -or $uri.Fragment -or $uri.AbsolutePath -ne '/') {
                throw "koruBlobByteOrigins entry '$byteOrigin' must be a bare http/https origin, with no path, query or credentials."
            }
            $address = [Net.IPAddress]::Any
            $local = if ($uri.IsLoopback) {
                $true
            } elseif ([Net.IPAddress]::TryParse($uri.Host, [ref]$address)) {
                $bytes = $address.GetAddressBytes()
                if ($address.AddressFamily -eq 'InterNetwork') {
                    # 10/8, 172.16/12, 192.168/16, and 169.254/16 link-local.
                    ($bytes[0] -eq 10) -or
                    ($bytes[0] -eq 172 -and $bytes[1] -ge 16 -and $bytes[1] -le 31) -or
                    ($bytes[0] -eq 192 -and $bytes[1] -eq 168) -or
                    ($bytes[0] -eq 169 -and $bytes[1] -eq 254)
                } else {
                    # Unique-local fc00::/7 and link-local fe80::/10.
                    (($bytes[0] -band 0xfe) -eq 0xfc) -or
                    ($bytes[0] -eq 0xfe -and ($bytes[1] -band 0xc0) -eq 0x80)
                }
            } else {
                $false
            }
            if (-not $local) {
                throw "koruBlobByteOrigins entry '$byteOrigin' is not a loopback or private address; the device would refuse it and send nothing."
            }
            if ($uri.IsLoopback) {
                # **Loopback is the right answer here, not a warning.** A phone's
                # own localhost is this machine for every port `adb reverse`
                # forwards, which is how the account origin has always worked;
                # the byte plane is simply a second port that needed the same
                # treatment. Naming a LAN address instead would move with DHCP,
                # need a firewall hole, and widen the development allowance for
                # no gain. So the port is forwarded below rather than talked
                # around.
                [Environment]::SetEnvironmentVariable(
                    'ANDROID_RUN_LOCAL_OBJECT_PORT',
                    $(if ($uri.IsDefaultPort) { if ($uri.Scheme -eq 'https') { 443 } else { 80 } } else { $uri.Port }),
                    'Process')
                Write-Host "[koru] byte plane on device localhost:$($uri.Port) will be forwarded to the local object worker"
            } else {
                # A LAN address is a deliberate choice - a phone on the same
                # network reaching this machine directly - and needs no forward.
                Write-Host "[koru] byte plane at $byteOrigin is reached directly; open that port on this machine's firewall"
            }
        }
    }
    foreach ($name in $known) {
        if ($values.ContainsKey($name)) {
            [Environment]::SetEnvironmentVariable('ORG_GRADLE_PROJECT_' + $name, $values[$name], 'Process')
        }
    }
    if ($values.ContainsKey('koruAccountCredentialRollout')) {
        Write-Host '[koru] Cloud sync is configured for this build.'
    } else {
        Write-Host '[koru] No koruAccountCredentialRollout: this build has no Cloud sync attached.'
    }
    # Internal handoff to the existing runner, which resolves the device once
    # and uses that exact serial for forwarding, installation and launch.
    $localPort = if ($origin.IsLoopback) { [string]$origin.Port } else { $null }
    [Environment]::SetEnvironmentVariable('ANDROID_RUN_LOCAL_API_PORT', $localPort, 'Process')
    [Environment]::SetEnvironmentVariable('ANDROID_RUN_LOCAL_ACCOUNT_LOGS', '1', 'Process')
    Write-Host "[koru] Android account build settings loaded from $envName."
    Write-Host "[koru] Account API: $($origin.AbsoluteUri.TrimEnd('/'))"
    Write-Host '[koru] Account diagnostics enabled for this run (no credentials or personal data).'
    & (Join-Path $AndroidDirectory 'koru.bat') run @forward
    exit $LASTEXITCODE
} catch {
    [Console]::Error.WriteLine('[koru] ' + $_.Exception.Message)
    exit 2
}

# SPDX-License-Identifier: MPL-2.0

param(
    [Parameter(Mandatory = $true)]
    [string[]]$ExecutablePath
)

# Stop only processes whose executable is an image the caller is about to
# rebuild. Several Koru worktrees and an installed release deliberately coexist;
# matching `ProcessName` (or taskkill /IM) would take all of them down.
#
# The input is a set rather than one path because Koru, Agent and Store supervise
# one another. A single pass can kill Koru, then watch it recreate Agent while
# the script moves on to Store. Keep stopping exact-path replacements until the
# complete set has stayed empty; a different worktree never enters that set.
#
# **One image is now several processes, and the exact-path rule still holds.**
# The API Gateway and the session host are worker roles of `koru-agent.exe`
# itself, so a running stack shows three processes at one path and each of them
# is this worktree's. Nothing here reads a command line to tell a parent from a
# role, and nothing needs to: a role of our image *is* ours, a role of another
# worktree's image resolves to another path and is never enumerated, and the
# Agent's Job Object ends its children with it - so stopping the parent usually
# empties the set on the next pass. The loop above is what covers the other
# order, where a role is stopped first and its supervisor puts one back.
$ErrorActionPreference = "Stop"
$targets = @(
    $ExecutablePath |
        ForEach-Object { [System.IO.Path]::GetFullPath($_) } |
        Select-Object -Unique
)

function Get-ExactImageProcesses {
    @(
        foreach ($target in $targets) {
            $name = [System.IO.Path]::GetFileNameWithoutExtension($target)
            Get-Process -Name $name -ErrorAction SilentlyContinue | ForEach-Object {
                try {
                    $candidate = [System.IO.Path]::GetFullPath($_.Path)
                    if ([string]::Equals(
                        $candidate,
                        $target,
                        [System.StringComparison]::OrdinalIgnoreCase
                    )) {
                        [pscustomobject]@{ Process = $_; Target = $target }
                    }
                } catch {
                    # A protected process can withhold its image path. It cannot
                    # be proved to be this executable, so it must not be stopped.
                }
            }
        }
    )
}

$stopped = @{}
foreach ($target in $targets) {
    $stopped[$target] = [System.Collections.Generic.HashSet[int]]::new()
}

$deadline = [DateTime]::UtcNow.AddSeconds(10)
$quietSince = $null
while ($true) {
    $matches = @(Get-ExactImageProcesses)
    $now = [DateTime]::UtcNow
    if ($matches.Count -eq 0) {
        if ($null -eq $quietSince) {
            $quietSince = $now
        } elseif (($now - $quietSince).TotalMilliseconds -ge 300) {
            break
        }
    } else {
        $quietSince = $null
        foreach ($match in $matches) {
            [void] $stopped[$match.Target].Add([int] $match.Process.Id)
            try {
                $match.Process | Stop-Process -Force -ErrorAction Stop
            } catch {
                # Exiting between enumeration and Stop-Process is success. Only
                # fail when the same pid at the same exact path is still live.
                $stillLive = @(Get-ExactImageProcesses | Where-Object {
                    $_.Target -eq $match.Target -and $_.Process.Id -eq $match.Process.Id
                })
                if ($stillLive.Count -gt 0) {
                    throw "[Koru] could not stop $($match.Target): $($_.Exception.Message)"
                }
            }
        }
    }

    if ($now -ge $deadline) {
        $remaining = @(Get-ExactImageProcesses)
        if ($remaining.Count -gt 0) {
            $description = $remaining | ForEach-Object {
                "$($_.Process.Id) $($_.Target)"
            }
            throw "[Koru] exact-path process(es) kept restarting: $($description -join '; ')"
        }
        break
    }
    Start-Sleep -Milliseconds 50
}

foreach ($target in $targets) {
    if ($stopped[$target].Count -gt 0) {
        Write-Host "[Koru] stopped $($stopped[$target].Count) process(es) from $target"
    }
}

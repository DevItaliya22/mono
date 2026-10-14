# SPDX-License-Identifier: MPL-2.0

# Stop the debug images built by this checkout as one supervised set, including
# an optional standalone AI chat launched by run-all or the workspace caption.
# Keeping the array construction in PowerShell avoids cmd.exe flattening an
# array-valued `-File` parameter into positional arguments.

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$bin = Join-Path $repo "target\debug"
$images = @(
    (Join-Path $bin "koru.exe"),
    (Join-Path $bin "koru-ai-chat.exe"),
    (Join-Path $bin "koru-agent.exe"),
    (Join-Path $bin "koru-com-notification.exe"),
    # Retired: both are worker roles of koru-agent.exe now and this checkout
    # builds neither. The paths stay because an earlier build of *this* worktree
    # may have left one running, and a stale gateway holding the root is exactly
    # the failure this script exists to clear. A path with no file is a no-op.
    (Join-Path $bin "koru-api-gateway.exe"),
    (Join-Path $bin "koru-session-host.exe"),
    (Join-Path $bin "koru-mcp.exe"),
    (Join-Path $bin "koru-voice-worker.exe"),
    (Join-Path $bin "koru-store-server.exe")
)

& (Join-Path $PSScriptRoot "stop-exact-exe.ps1") -ExecutablePath $images
exit 0

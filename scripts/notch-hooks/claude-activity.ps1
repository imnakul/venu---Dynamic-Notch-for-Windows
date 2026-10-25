# Claude Code UserPromptSubmit hook -> Venu "agent working" pod.
# Registered in ~/.claude/settings.json under hooks.UserPromptSubmit.
#
# Tells Venu that Claude has started on a prompt, so a satellite pod beside
# the notch can show how long it has been working. Nothing extra is needed to
# end it: the Stop hook (claude-notify.ps1) posts a "success" notification,
# which closes the pod. Venu also drops any activity it hears nothing about
# for an hour.

$ErrorActionPreference = 'SilentlyContinue'
[System.Net.ServicePointManager]::Expect100Continue = $false

# Claude Code pipes the event JSON to stdin; drain it so the pipe closes.
$null = [Console]::In.ReadToEnd()

$payload = @{
    app   = 'Claude'
    state = 'start'
} | ConvertTo-Json -Compress

try {
    Invoke-RestMethod -Uri 'http://127.0.0.1:18923/activity' -Method Post -Body $payload -ContentType 'application/json' -TimeoutSec 2 | Out-Null
} catch {
    # Notch app not running or webhook disabled - fail silently, never block Claude Code.
}

exit 0

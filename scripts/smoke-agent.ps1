param(
    [string]$Project = 'moby-api',
    [string]$Api = 'http://127.0.0.1:8080',
    [string]$Mailpit = 'http://127.0.0.1:8025',
    [string]$ApiLog
)

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/smoke-mailpit.ps1"
$suffix = [Guid]::NewGuid().ToString('N')
$email = "b3-$suffix@example.test"
$password = 'Synthetic agent smoke password'
$promptMarker = "synthetic-prompt-$suffix"
$toolMarker = "synthetic-result-$suffix"

function Post($path, $payload, $expected, $token = $null) {
    $options = @{
        Uri = "$Api$path"
        Method = 'Post'
        NoProxy = $true
        TimeoutSec = 20
        SkipHttpErrorCheck = $true
        ContentType = 'application/json'
        Body = ($payload | ConvertTo-Json -Depth 12 -Compress)
    }
    if ($token) { $options.Headers = @{ Authorization = "Bearer $token" } }
    $response = Invoke-WebRequest @options
    if ([int]$response.StatusCode -ne $expected) {
        throw "$path returned $($response.StatusCode), expected $expected"
    }
    if ($expected -ge 400) {
        $errorBody = $response.Content | ConvertFrom-Json
        if (-not $errorBody.code -or $errorBody.request_id -ne $response.Headers['x-request-id'][0]) {
            throw "$path did not return a correlated API error"
        }
    }
    return $response
}

function Read-Events($response) {
    if ($response.Headers['Content-Type'][0] -ne 'text/event-stream') {
        throw 'Agent did not return an event stream'
    }
    $text = $response.Content
    if ($text -is [byte[]]) { $text = [Text.Encoding]::UTF8.GetString($text) }
    foreach ($frame in ($text.TrimEnd() -split '\r?\n\r?\n')) {
        if (-not $frame.StartsWith('data: ')) { throw 'Unexpected SSE framing' }
        $frame.Substring(6) | ConvertFrom-Json
    }
}

[void](Post '/v1/auth/register' @{
    email = $email
    password = $password
    terms_version = 'dev-1'
    privacy_policy_version = 'dev-1'
} 202)
$link = Get-AccountEmail $Mailpit $email 'Verify your Moby email'
[void](Post '/v1/auth/verify-email' @{ token = $link.Token; password = $password } 204)
$login = Post '/v1/auth/login' @{ email = $email; password = $password } 200
$token = ($login.Content | ConvertFrom-Json).access_token

$path = '/v1/agent/turn'
$turn = @{
    messages = @(@{ kind = 'user'; text = $promptMarker })
    available_tools = @()
    resources = @()
    language = 'uz'
    script = 'latn'
}
[void](Post $path $turn 401)
$events = @(Read-Events (Post $path $turn 200 $token))
if (($events.event -join ',') -ne 'speech_delta,completed' -or $events[-1].speech_text -notlike 'Salom!*') {
    throw 'Expected a deterministic text completion'
}

$turn.messages[0].text = 'git status'
$turn.available_tools = @('git_status')
$turn.resources = @(@{ kind = 'project'; id = 'demo-project'; display_name = 'Demo' })
$events = @(Read-Events (Post $path $turn 200 $token))
$call = $events[0].call
if ($events.Count -ne 1 -or $events[0].event -ne 'tool_call' -or
    $call.request.tool -ne 'git_status' -or $call.request.project_id -ne 'demo-project') {
    throw 'Expected a single git_status suggestion for the advertised project'
}
$turn.messages += @{ kind = 'tool_call'; call = $call }
$turn.messages += @{ kind = 'tool_result'; result = @{
    status = 'ok'
    call_id = $call.call_id
    output = $toolMarker
} }
$events = @(Read-Events (Post $path $turn 200 $token))
if (($events.event -join ',') -ne 'speech_delta,completed' -or $events[-1].speech_text -notlike 'Vosita natijasi*') {
    throw 'Expected a completion for the matched tool result'
}
$turn.messages[2].result.call_id = 'wrong-call'
[void](Post $path $turn 400 $token)
$turn.messages[2].result.call_id = $call.call_id
$call.request.project_id = 'unadvertised-project'
[void](Post $path $turn 400 $token)
$turn.messages = @(@{ kind = 'user'; text = ('a' * (129 * 1024)) })
[void](Post $path $turn 413 $token)

$dump = docker compose -p $Project exec -T postgres pg_dump -U moby -d moby --data-only --no-owner
if ($LASTEXITCODE -ne 0) { throw 'Could not inspect PostgreSQL data' }
if ($ApiLog) {
    $logs = Get-Content -LiteralPath $ApiLog
} else {
    $logs = docker compose -p $Project logs --no-color api
    if ($LASTEXITCODE -ne 0) { throw 'Could not inspect API logs' }
}
foreach ($marker in @($promptMarker, $toolMarker)) {
    if (($dump | Select-String -SimpleMatch $marker) -or ($logs | Select-String -SimpleMatch $marker)) {
        throw 'Conversation content appeared in PostgreSQL or API logs'
    }
}

Write-Output 'Agent smoke passed: verified login, SSE, tool round trip, validation and content privacy'

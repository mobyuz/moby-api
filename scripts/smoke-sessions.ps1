param(
    [string]$Project = 'moby-api',
    [string]$Api = 'http://127.0.0.1:8080',
    [string]$Mailpit = 'http://127.0.0.1:8025'
)

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/smoke-mailpit.ps1"
$suffix = [Guid]::NewGuid().ToString('N')
$email = "b2-$suffix@example.test"
$other = "b2-other-$suffix@example.test"
$password = 'Example session password 123'
$newPassword = 'Example changed session password 456'

function Invoke-Api($method, $path, $expected, $payload = $null, $token = $null) {
    $options = @{
        Uri = "$Api$path"
        Method = $method
        TimeoutSec = 15
        NoProxy = $true
        SkipHttpErrorCheck = $true
    }
    if ($null -ne $payload) {
        $options.ContentType = 'application/json'
        $options.Body = $payload | ConvertTo-Json -Compress
    }
    if ($null -ne $token) {
        $options.Headers = @{ Authorization = "Bearer $token" }
    }
    $response = Invoke-WebRequest @options
    if ([int]$response.StatusCode -ne $expected) {
        throw "$method $path returned $($response.StatusCode), expected $expected"
    }
    if ($response.Content) {
        $body = $response.Content | ConvertFrom-Json -NoEnumerate
        if ($expected -ge 400 -and $body.request_id -ne $response.Headers['x-request-id'][0]) {
            throw "$method $path did not return a correlated API error"
        }
        return ,$body
    }
}

function Register($address) {
    [void](Invoke-Api POST '/v1/auth/register' 202 @{
        email = $address
        password = $password
        terms_version = 'dev-1'
        privacy_policy_version = 'dev-1'
    })
}

function Confirm-Registration($address) {
    $link = Get-AccountEmail $Mailpit $address 'Verify your Moby email'
    [void](Invoke-Api POST '/v1/auth/verify-email' 204 @{ token = $link.Token; password = $password })
}

function Login($address, $secret, $expected = 200, $installation = $null) {
    $request = @{ email = $address; password = $secret }
    if ($installation) { $request.installation_id = $installation }
    return Invoke-Api POST '/v1/auth/login' $expected $request
}

function Assert-Revoked($pair) {
    [void](Invoke-Api GET '/v1/me' 401 $null $pair.access_token)
    [void](Invoke-Api POST '/v1/auth/refresh' 401 @{ refresh_token = $pair.refresh_token })
}

Register $email
$pending = Login $email $password 401
Confirm-Registration $email
$wrong = Login $email $newPassword 401
$missing = Login "missing-$suffix@example.test" $password 401
if ($wrong.code -ne 'unauthorized' -or $pending.message -ne $wrong.message -or $wrong.message -ne $missing.message) {
    throw 'Login errors reveal account state'
}

$first = Login $email $password
$profile = Invoke-Api GET '/v1/me' 200 $null $first.access_token
if ($profile.email -ne $email -or $profile.installation_id -ne $first.installation_id -or $first.expires_in -ne 900) {
    throw 'Login did not issue the expected authenticated session'
}
$parts = $first.access_token.Split('.')
$replacement = if ($parts[2].StartsWith('A')) { 'B' } else { 'A' }
$parts[2] = $replacement + $parts[2].Substring(1)
[void](Invoke-Api GET '/v1/me' 401 $null ($parts -join '.'))

$rotated = Invoke-Api POST '/v1/auth/refresh' 200 @{ refresh_token = $first.refresh_token }
if ($rotated.refresh_token -eq $first.refresh_token -or $rotated.access_token -eq $first.access_token) {
    throw 'Refresh failed to issue a new token pair'
}
[void](Invoke-Api POST '/v1/auth/refresh' 401 @{ refresh_token = $first.refresh_token })
Assert-Revoked $rotated

$parallel = Login $email $password 200 $first.installation_id
$body = @{ refresh_token = $parallel.refresh_token } | ConvertTo-Json -Compress
$handler = [System.Net.Http.HttpClientHandler]::new()
$handler.UseProxy = $false
$client = [System.Net.Http.HttpClient]::new($handler)
try {
    $client.Timeout = [TimeSpan]::FromSeconds(15)
    $one = $client.PostAsync("$Api/v1/auth/refresh", [System.Net.Http.StringContent]::new($body, [Text.Encoding]::UTF8, 'application/json'))
    $two = $client.PostAsync("$Api/v1/auth/refresh", [System.Net.Http.StringContent]::new($body, [Text.Encoding]::UTF8, 'application/json'))
    $responses = @($one.GetAwaiter().GetResult(), $two.GetAwaiter().GetResult())
    $codes = $responses | ForEach-Object { [int]$_.StatusCode } | Sort-Object
    if (($codes -join ',') -ne '200,401') { throw "Parallel refresh returned unexpected statuses $codes" }
    $issued = ($responses | Where-Object IsSuccessStatusCode)[0].Content.ReadAsStringAsync().GetAwaiter().GetResult() | ConvertFrom-Json
    Assert-Revoked $issued
} finally {
    $client.Dispose()
}

$loggedOut = Login $email $password 200 $first.installation_id
[void](Invoke-Api POST '/v1/auth/logout' 204 @{ refresh_token = $loggedOut.refresh_token })
[void](Invoke-Api POST '/v1/auth/logout' 204 @{ refresh_token = $loggedOut.refresh_token })
Assert-Revoked $loggedOut

$active = Login $email $password 200 $first.installation_id
$secondInstallation = Invoke-Api POST '/v1/installations' 201 @{ name = 'Test laptop'; app_version = '0.1.0' } $active.access_token
$installations = Invoke-Api GET '/v1/installations' 200 $null $active.access_token
if ($installations.Count -ne 2 -or $secondInstallation.os -ne 'windows') {
    throw 'Expected two Windows installations owned by the signed-in account'
}

Register $other
Confirm-Registration $other
[void](Login $other $password 401 $first.installation_id)
$otherSession = Login $other $password
[void](Invoke-Api DELETE "/v1/installations/$($first.installation_id)" 404 $null $otherSession.access_token)
[void](Invoke-Api GET '/v1/me' 200 $null $active.access_token)
[void](Invoke-Api DELETE "/v1/installations/$($first.installation_id)" 204 $null $active.access_token)
Assert-Revoked $active

$beforeReset = Login $email $password 200 $secondInstallation.id
[void](Invoke-Api POST '/v1/auth/password/forgot' 202 @{ email = $email })
$reset = Get-AccountEmail $Mailpit $email 'Reset your Moby password'
[void](Invoke-Api POST '/v1/auth/password/reset' 204 @{ token = $reset.Token; new_password = $newPassword })
Assert-Revoked $beforeReset
[void](Invoke-Api GET '/v1/me' 200 $null $otherSession.access_token)
$profileSession = Login $email $newPassword 200 $secondInstallation.id

Write-Output 'Session smoke passed: login, JWT validation, rotation, concurrent replay, logout, ownership and password reset revocation'

$access = $profileSession.access_token
[void](Invoke-Api POST '/v1/me/export' 401)
[void](Invoke-Api PATCH '/v1/me' 400 @{} $access)
[void](Invoke-Api PATCH '/v1/me' 400 @{ display_name = '  ' } $access)
$updated = Invoke-Api PATCH '/v1/me' 200 @{ display_name = '  Moby sinov nomi  ' } $access
if ($updated.display_name -cne 'Moby sinov nomi' -or $updated.email -cne $email) {
    throw 'Profile update did not trim and save the display name'
}
$cleared = Invoke-Api PATCH '/v1/me' 200 @{ display_name = $null } $access
if ($null -ne $cleared.display_name) { throw 'Explicit null did not clear the name' }
[void](Invoke-Api PATCH '/v1/me' 200 @{ display_name = 'Moby sinov nomi' } $access)

$initial = Invoke-Api GET '/v1/consents' 200 $null $access
if ($initial.records.Count -ne 2 -or ($initial.records | Where-Object type -eq 'cloud_voice')) {
    throw 'Registration did not keep cloud voice consent separate'
}
$voice = @{ type = 'cloud_voice'; version = 'old-test-version'; accepted = $true }
[void](Invoke-Api POST '/v1/consents' 400 $voice $access)
$voice.version = $initial.current_versions.cloud_voice
[void](Invoke-Api POST '/v1/consents' 204 $voice $access)
$granted = (Invoke-Api GET '/v1/consents' 200 $null $access).records | Where-Object type -eq 'cloud_voice'
[void](Invoke-Api POST '/v1/consents' 204 $voice $access)
$sameGrant = (Invoke-Api GET '/v1/consents' 200 $null $access).records | Where-Object type -eq 'cloud_voice'
if ($granted.accepted_at -ne $sameGrant.accepted_at -or $null -ne $sameGrant.revoked_at) {
    throw 'Repeated grant changed an active consent'
}
$voice.accepted = $false
[void](Invoke-Api POST '/v1/consents' 204 $voice $access)
$revoked = (Invoke-Api GET '/v1/consents' 200 $null $access).records | Where-Object type -eq 'cloud_voice'
[void](Invoke-Api POST '/v1/consents' 204 $voice $access)
$sameRevocation = (Invoke-Api GET '/v1/consents' 200 $null $access).records | Where-Object type -eq 'cloud_voice'
if ($null -eq $revoked.revoked_at -or $revoked.revoked_at -ne $sameRevocation.revoked_at) {
    throw 'Withdrawal must record a stable revocation time'
}
$voice.accepted = $true
[void](Invoke-Api POST '/v1/consents' 204 $voice $access)
$regranted = (Invoke-Api GET '/v1/consents' 200 $null $access).records | Where-Object type -eq 'cloud_voice'
if ($null -ne $regranted.revoked_at) { throw 'Grant after withdrawal did not reactivate consent' }

$response = Invoke-WebRequest -Uri "$Api/v1/me/export" -Method Post -Headers @{ Authorization = "Bearer $access" } -NoProxy -TimeoutSec 15
$export = $response.Content | ConvertFrom-Json
if ($export.account.email -cne $email -or $export.account.display_name -cne 'Moby sinov nomi' -or
    $export.installations.Count -ne 2 -or $export.sessions.Count -lt 2 -or $export.consents.Count -ne 3 -or
    $response.Headers['Content-Disposition'][0] -notlike 'attachment*') {
    throw 'Export omitted account data or attachment metadata'
}
if ($response.Content -match '"(password_hash|token_hash|access_token|refresh_token|session_id)"' -or
    $response.Content.Contains($other)) {
    throw 'Export exposed credentials or another account'
}

[void](Invoke-Api DELETE '/v1/me' 401 @{ password = $password } $access)
[void](Invoke-Api GET '/v1/me' 200 $null $access)
[void](Invoke-Api DELETE '/v1/me' 204 @{ password = $newPassword } $access)
Assert-Revoked $profileSession
[void](Invoke-Api POST '/v1/me/export' 401 $null $access)
[void](Login $email $newPassword 401)
$otherProfile = Invoke-Api GET '/v1/me' 200 $null $otherSession.access_token
$otherConsents = Invoke-Api GET '/v1/consents' 200 $null $otherSession.access_token
if ($otherProfile.email -cne $other -or $null -ne $otherProfile.display_name -or $otherConsents.records.Count -ne 2) {
    throw 'Account deletion changed another account'
}

$userId = [Guid]$export.account.id
$jwt = $access.Split('.')[1].Replace('-', '+').Replace('_', '/')
$jwt = $jwt.PadRight($jwt.Length + (4 - $jwt.Length % 4) % 4, '=')
$claims = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($jwt)) | ConvertFrom-Json
$sessionId = [Guid]$claims.sid
$sql = @"
SELECT (SELECT count(*) FROM users WHERE id = '$userId')
     + (SELECT count(*) FROM installations WHERE user_id = '$userId')
     + (SELECT count(*) FROM sessions WHERE user_id = '$userId')
     + (SELECT count(*) FROM refresh_tokens WHERE session_id = '$sessionId')
     + (SELECT count(*) FROM email_tokens WHERE user_id = '$userId')
     + (SELECT count(*) FROM consents WHERE user_id = '$userId');
"@
$remaining = $sql | docker compose -p $Project exec -T postgres psql -U moby -d moby -At -v ON_ERROR_STOP=1
if ($LASTEXITCODE -ne 0 -or ($remaining -join '').Trim() -ne '0') {
    throw 'Account deletion left owned database records'
}

Write-Output 'Profile smoke passed: edit, clear, export, consent withdrawal and regrant, password-confirmed deletion and account isolation'

# Run against an isolated Compose project using the local development defaults.
param(
    [string]$Project = 'moby-api',
    [string]$Api = 'http://127.0.0.1:8080',
    [string]$Mailpit = 'http://127.0.0.1:8025'
)

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/smoke-mailpit.ps1"
$suffix = [Guid]::NewGuid().ToString('N')
$email = "b1-$suffix@example.test"
$expiredEmail = "expired-$suffix@example.test"
$unknownEmail = "unknown-$suffix@example.test"
$registrationPassword = 'Example registration password 123'
$resetPassword = 'Example replacement password 456'

function Post($path, $payload, $expectedStatus) {
    $body = $payload | ConvertTo-Json -Compress
    $response = Invoke-WebRequest -Uri "$Api$path" -Method Post -ContentType 'application/json' -Body $body -NoProxy -TimeoutSec 10 -SkipHttpErrorCheck
    if ([int]$response.StatusCode -ne $expectedStatus) {
        throw "$path returned $($response.StatusCode), expected $expectedStatus"
    }
    if ($expectedStatus -ge 400) {
        $errorBody = $response.Content | ConvertFrom-Json
        if (-not $errorBody.code -or $errorBody.request_id -ne $response.Headers['x-request-id'][0]) {
            throw "$path did not return a correlated API error"
        }
    }
    return $response
}

function Read-Database($sql) {
    $output = $sql | docker compose -p $Project exec -T postgres psql -U moby -d moby -At -v ON_ERROR_STOP=1
    if ($LASTEXITCODE -ne 0) { throw "Database assertion failed" }
    return ($output -join ' ').Trim()
}

$register = @{
    email = $email.ToUpperInvariant()
    password = $registrationPassword
    terms_version = 'dev-1'
    privacy_policy_version = 'dev-1'
}
[void](Post '/v1/auth/register' $register 202)
$verify = Get-AccountEmail $Mailpit $email 'Verify your Moby email'
$form = Invoke-WebRequest -Uri $verify.Link -NoProxy -TimeoutSec 10
if ($form.Content -notmatch 'Registration password' -or $form.Headers['Referrer-Policy'][0] -ne 'no-referrer') {
    throw "Email link did not open the protected verification form"
}
if ((Read-Database "SELECT (email_verified_at IS NULL AND password_hash IS NULL) FROM users WHERE email = '$email';") -ne 't') {
    throw "Opening the link must not activate the account"
}
[void](Post '/v1/auth/verify-email' @{ token = $verify.Token; password = $resetPassword } 400)
[void](Post '/v1/auth/verify-email' @{ token = $verify.Token; password = $registrationPassword } 204)
[void](Post '/v1/auth/verify-email' @{ token = $verify.Token; password = $registrationPassword } 400)
[void](Post '/v1/auth/register' $register 202)
if ((Read-Database "SELECT count(*) FROM consents JOIN users ON users.id = consents.user_id WHERE email = '$email' AND version = 'dev-1';") -ne '2') {
    throw "Expected two versioned consent records"
}
$oldHash = Read-Database "SELECT password_hash FROM users WHERE email = '$email' AND email_verified_at IS NOT NULL;"
if (-not $oldHash.StartsWith('$argon2id$')) { throw "Expected an Argon2id password hash" }

$known = Post '/v1/auth/password/forgot' @{ email = $email } 202
$unknown = Post '/v1/auth/password/forgot' @{ email = $unknownEmail } 202
if ($known.Content -cne $unknown.Content) { throw "Forgot responses reveal account existence" }
[void](Post '/v1/auth/password/forgot' @{ email = $unknownEmail } 202)
[void](Post '/v1/auth/password/forgot' @{ email = $unknownEmail } 202)
[void](Post '/v1/auth/password/forgot' @{ email = $unknownEmail } 429)

$reset = Get-AccountEmail $Mailpit $email 'Reset your Moby password'
$resetForm = Invoke-WebRequest -Uri $reset.Link -NoProxy -TimeoutSec 10
if ($resetForm.Content -notmatch 'name="new_password"') { throw "Expected the password reset form" }
$payload = @{ token = $reset.Token; new_password = $resetPassword } | ConvertTo-Json -Compress
$client = [System.Net.Http.HttpClient]::new()
try {
    $client.Timeout = [TimeSpan]::FromSeconds(15)
    $first = $client.PostAsync("$Api/v1/auth/password/reset", [System.Net.Http.StringContent]::new($payload, [Text.Encoding]::UTF8, 'application/json'))
    $second = $client.PostAsync("$Api/v1/auth/password/reset", [System.Net.Http.StringContent]::new($payload, [Text.Encoding]::UTF8, 'application/json'))
    $firstCode = [int]$first.GetAwaiter().GetResult().StatusCode
    $secondCode = [int]$second.GetAwaiter().GetResult().StatusCode
    $codes = @($firstCode, $secondCode) | Sort-Object
    if (($codes -join ',') -ne '204,400') { throw "One-time reset accepted unexpected statuses $codes" }
} finally {
    $client.Dispose()
}
[void](Post '/v1/auth/password/reset' @{ token = $reset.Token; new_password = $resetPassword } 400)
$newHash = Read-Database "SELECT password_hash FROM users WHERE email = '$email';"
if (-not $newHash.StartsWith('$argon2id$') -or $oldHash -eq $newHash) { throw "Password hash was not rotated" }

$register.email = $expiredEmail
[void](Post '/v1/auth/register' $register 202)
$expired = Get-AccountEmail $Mailpit $expiredEmail 'Verify your Moby email'
[void](Read-Database "UPDATE email_tokens SET expires_at = now() - interval '1 second' WHERE user_id = (SELECT id FROM users WHERE email = '$expiredEmail');")
[void](Post '/v1/auth/verify-email' @{ token = $expired.Token; password = $registrationPassword } 400)

Write-Output 'Account smoke passed: delivery, password proof, consent, expiry, replay, rate limit and concurrent reset'

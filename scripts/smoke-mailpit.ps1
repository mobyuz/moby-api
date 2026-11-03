function Get-AccountEmail($server, $address, $subject) {
    $query = [Uri]::EscapeDataString("to:$address")
    for ($attempt = 0; $attempt -lt 30; $attempt++) {
        $mail = (Invoke-RestMethod -Uri "$server/api/v1/search?query=$query" -NoProxy).messages |
            Where-Object Subject -eq $subject | Select-Object -First 1
        if ($mail) {
            $message = Invoke-RestMethod -Uri "$server/api/v1/message/$($mail.ID)" -NoProxy
            $match = [regex]::Match($message.Text, '(https?://\S+#token=([0-9a-f]{64}))')
            if (-not $match.Success) { throw "Mailpit message is missing its one-time link" }
            return @{ Link = $match.Groups[1].Value; Token = $match.Groups[2].Value }
        }
        Start-Sleep -Milliseconds 300
    }
    throw "Expected account email was not delivered"
}

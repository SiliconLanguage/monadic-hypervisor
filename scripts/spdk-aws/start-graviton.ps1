# 0. Automatically load the .env file from the root of the repository
$repoRoot = (Get-Item $PSScriptRoot).Parent.Parent.FullName
$envFile = Join-Path $repoRoot ".env"

if (Test-Path $envFile) {
    Write-Host "Loading environment from: $envFile" -ForegroundColor DarkGray
    Get-Content $envFile | Where-Object { $_ -match '=' -and $_.Trim() -notmatch '^#' } | ForEach-Object {
        $name, $value = $_.Split('=', 2)
        Set-Item -Path "env:\$($name.Trim())" -Value $value.Trim().Trim('"')
    }
} else {
    Write-Error "No .env file found at $envFile!"
    exit
}

# 1. Load Secrets and User-Specific Info from Environment
$Token       = $env:CF_API_TOKEN
$ZoneID      = $env:CF_ZONE_ID
$RecordName  = $env:DEV_DOMAIN      
$SSHKeyPath  = $env:DEV_SSH_KEY     
$RemoteUser  = $env:DEV_USER        

# Let's print the AWS variables so we know they loaded correctly before making the call
Write-Host "Target Instance: $env:INSTANCE_ID in $env:AWS_REGION" -ForegroundColor Cyan

Write-Host "Ensuring instance is starting..." -ForegroundColor Cyan
aws ec2 start-instances --instance-ids $env:INSTANCE_ID --region $env:AWS_REGION | Out-Null

# Optional: Wait for the instance to be running so it has an IP
Write-Host "Waiting for instance to reach 'running' state..." -ForegroundColor Yellow
aws ec2 wait instance-running --instance-ids $env:INSTANCE_ID --region $env:AWS_REGION

Write-Host "Fetching AWS Instance IP..." -ForegroundColor Cyan
$IP = (aws ec2 describe-instances --instance-ids $env:INSTANCE_ID --region $env:AWS_REGION --query "Reservations[0].Instances[0].PublicIpAddress" --output text).Trim()

if ($IP -eq "None" -or [string]::IsNullOrWhiteSpace($IP)) {
    Write-Error "Could not get an IP from AWS. Is the instance fully running?"
    exit
}

# 2. Fetch the Record ID from Cloudflare
$Headers = @{"Authorization" = "Bearer $Token"; "Content-Type" = "application/json"}
$RecordsURL = "https://api.cloudflare.com/client/v4/zones/$ZoneID/dns_records?name=$RecordName"

try {
    $Records = Invoke-RestMethod -Uri $RecordsURL -Headers $Headers
    if ($Records.result.Count -eq 0) {
        Write-Error "Could not find DNS record for $RecordName"
        exit
    }
    $RecordID = $Records.result[0].id

    # 3. Update the DNS Record
    $UpdateURL = "https://api.cloudflare.com/client/v4/zones/$ZoneID/dns_records/$RecordID"
    $Body = @{
        type    = "A"; name = $RecordName; content = $IP; ttl = 60; proxied = $false
    } | ConvertTo-Json

    $Result = Invoke-RestMethod -Method Put -Uri $UpdateURL -Headers $Headers -Body $Body

    if ($Result.success) {
        Write-Host "Successfully updated $RecordName to $IP" -ForegroundColor Green
    }
} catch {
    Write-Error "Cloudflare API Error: $_"
    exit
}

# 4. Launch SSH Session
Write-Host "Opening SSH Tunnel to $RemoteUser@$RecordName..." -ForegroundColor Yellow

#ssh -i "$SSHKeyPath" "${RemoteUser}@${RecordName}"
# We use the IP instead of the RecordName to avoid any potential DNS propagation issues, even though we just updated it.
ssh -i "$SSHKeyPath" "${RemoteUser}@${IP}"

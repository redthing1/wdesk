param([ValidateSet('reference','lite','core')][string]$Profile = 'lite')
$ErrorActionPreference = 'Stop'
Start-Transcript -Path 'C:\wdesk-provision.log' -Append
$root = 'C:\ProgramData\wdesk'
New-Item -ItemType Directory -Force $root, "$root\workspace", "$root\downloads", "$root\staging" | Out-Null
Copy-Item "$PSScriptRoot\agent.ps1", "$PSScriptRoot\native.cs", "$PSScriptRoot\a11y.ps1" $root -Force
# The serial channel belongs to the dedicated interactive test account.
$sid = (New-Object Security.Principal.NTAccount "$env:COMPUTERNAME\wdesk").Translate([Security.Principal.SecurityIdentifier]).Value
& icacls $root /inheritance:r /grant:r "*${sid}:(OI)(CI)F" '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Failed to configure guest helper directory permissions' }
& powercfg /change monitor-timeout-ac 0
& powercfg /change standby-timeout-ac 0
& powercfg /hibernate off
Set-ItemProperty 'HKCU:\Control Panel\Desktop' ScreenSaveActive '0'
Set-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon' AutoLogonCount 999999 -Type DWord
Set-LocalUser -Name 'wdesk' -PasswordNeverExpires $true
if ($Profile -eq 'lite') {
    # An allowlist of consumer apps; leave servicing, browser runtimes and security alone.
    $apps = @('Microsoft.BingNews','Microsoft.BingWeather','Microsoft.GetHelp','Microsoft.Getstarted','Microsoft.MicrosoftSolitaireCollection','Microsoft.People','Microsoft.WindowsFeedbackHub','Microsoft.YourPhone','Microsoft.ZuneMusic','Microsoft.ZuneVideo','Clipchamp.Clipchamp','Microsoft.GamingApp')
    foreach ($name in $apps) {
        Get-AppxPackage -Name $name -AllUsers | Remove-AppxPackage -AllUsers -ErrorAction SilentlyContinue
        Get-AppxProvisionedPackage -Online | Where-Object DisplayName -eq $name | Remove-AppxProvisionedPackage -Online -ErrorAction SilentlyContinue | Out-Null
    }
    $cdm = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager'
    if (Test-Path $cdm) { Set-ItemProperty $cdm ContentDeliveryAllowed 0 -Type DWord }
}
# Run at normal interactive-user integrity. Provisioning is the only elevated step.
$command = 'powershell.exe -NoProfile -STA -WindowStyle Hidden -ExecutionPolicy Bypass -File "C:\ProgramData\wdesk\agent.ps1"'
New-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name wdesk -Value $command -PropertyType String -Force | Out-Null
@{ profile=$Profile; helper='0.1.0'; provisioned=(Get-Date).ToUniversalTime().ToString('o') } | ConvertTo-Json | Set-Content "$root\provisioning.json" -Encoding UTF8
# FirstLogonCommands is elevated. Reboot so Run starts the helper at normal integrity.
Restart-Computer -Force

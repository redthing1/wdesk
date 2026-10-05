param([int]$Port=8080,[string]$Token='wdesk-port',[int]$LifetimeSeconds=600)
$ErrorActionPreference='Stop'
if($Port -lt 1 -or $Port -gt 65535 -or $LifetimeSeconds -lt 1 -or $LifetimeSeconds -gt 900) { throw 'invalid probe parameters' }
if($Token -notmatch '^[a-zA-Z0-9_-]{1,64}$') { throw 'invalid token' }
$identity=[Security.Principal.WindowsIdentity]::GetCurrent()
$principal=New-Object Security.Principal.WindowsPrincipal($identity)
if(-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'run the fixture elevated; do not disable UAC or the firewall' }
$rule='wdesk-port-probe-'+[Guid]::NewGuid().ToString('N')
$listener=New-Object Net.Sockets.TcpListener([Net.IPAddress]::Any,$Port)
try {
    New-NetFirewallRule -Name $rule -DisplayName $rule -Direction Inbound -Action Allow -Protocol TCP -LocalPort $Port -Program "$PSHOME\powershell.exe" -Profile Any | Out-Null
    $listener.Start()
    $clock=[Diagnostics.Stopwatch]::StartNew()
    while($clock.Elapsed.TotalSeconds -lt $LifetimeSeconds) {
        if(-not $listener.Pending()) { Start-Sleep -Milliseconds 50; continue }
        $client=$listener.AcceptTcpClient()
        try {
            $client.ReceiveTimeout=5000; $client.SendTimeout=5000
            $stream=$client.GetStream()
            $request=New-Object byte[] 4096
            [void]$stream.Read($request,0,$request.Length)
            $body=[Text.Encoding]::ASCII.GetBytes($Token+"`n")
            $peer=$client.Client.RemoteEndPoint.Address.ToString()
            $header=[Text.Encoding]::ASCII.GetBytes("HTTP/1.1 200 OK`r`nContent-Type: text/plain`r`nContent-Length: $($body.Length)`r`nX-Wdesk-Peer: $peer`r`nConnection: close`r`n`r`n")
            $stream.Write($header,0,$header.Length); $stream.Write($body,0,$body.Length)
        } catch { [Console]::Error.WriteLine($_.Exception.Message) }
        finally { $client.Close() }
    }
} finally {
    $listener.Stop()
    Remove-NetFirewallRule -Name $rule -ErrorAction SilentlyContinue
}

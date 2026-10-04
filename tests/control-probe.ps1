$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -Path 'C:\ProgramData\wdesk\native.cs'
$flags=[Reflection.BindingFlags]'Instance,NonPublic'
$listener=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0)
$client=[Net.Sockets.TcpClient]::new()
$client.ReceiveBufferSize=262144
$peer=$null
$listener.Start()
function Read-Line {
    $until=[DateTime]::UtcNow.AddSeconds(5)
    do {
        $line=$read.Invoke($wire,@())
        if($null -ne $line) { return $line }
        if([DateTime]::UtcNow -gt $until) { throw 'Control reader deadline' }
        Start-Sleep -Milliseconds 1
    } while($true)
}
function Send([byte[]]$bytes) { $peer.GetStream().Write($bytes,0,$bytes.Length) }
try {
    $connecting=$listener.AcceptTcpClientAsync()
    $client.Connect([Net.IPAddress]::Loopback,$listener.LocalEndpoint.Port)
    $peer=$connecting.GetAwaiter().GetResult()
    $peer.NoDelay=$true
    $wire=[Runtime.Serialization.FormatterServices]::GetUninitializedObject([Wdesk.GuestWire])
    $values=@{fast=$client;fastBuffer=[Text.StringBuilder]::new();decoder=[Text.Encoding]::UTF8.GetDecoder();bytes=[byte[]]::new(65536);chars=[char[]]::new([Text.Encoding]::UTF8.GetMaxCharCount(65536))}
    foreach($name in $values.Keys) { [Wdesk.GuestWire].GetField($name,$flags).SetValue($wire,$values[$name]) }
    $read=[Wdesk.GuestWire].GetMethod('ReadFast',$flags)
    if($null -eq $read) { throw 'Current control reader required' }
    for($i=0;$i -lt 1000;$i++) {
        # Deterministically put data between the old Available and Poll checks.
        if($client.Available -ne 0) { throw 'Unexpected queued data' }
        $expected="request-$i"
        Send ([Text.Encoding]::UTF8.GetBytes($expected+"`n"))
        if(!$client.Client.Poll(5000000,[Net.Sockets.SelectMode]::SelectRead) -or $client.Available -eq 0) { throw 'Expected readable data, not EOF' }
        if((Read-Line) -ne $expected) { throw 'Control request dropped' }
    }
    # A buffered UTF-8 prefix can expand the next full read by one UTF-16 unit.
    $cat=([char]0xd83d).ToString()+[char]0xdc08
    $unicode=[Text.Encoding]::UTF8.GetBytes($cat)
    Send ([byte[]]$unicode[0..2])
    if(!$client.Client.Poll(5000000,[Net.Sockets.SelectMode]::SelectRead)) { throw 'Prefix deadline' }
    if($null -ne $read.Invoke($wire,@())) { throw 'Premature line' }
    $expected=$cat+('x'*65535)
    $tail=[byte[]]::new(65537)
    $tail[0]=$unicode[3]
    for($i=1;$i -lt 65536;$i++) { $tail[$i]=120 }
    $tail[65536]=10
    Send $tail
    if((Read-Line) -ne $expected) { throw 'Split UTF-8 boundary corrupted' }
    Send ([Text.Encoding]::UTF8.GetBytes("first`nsecond`n"))
    $peer.Dispose();$peer=$null
    if((Read-Line) -ne 'first' -or (Read-Line) -ne 'second') { throw 'Queued lines lost at EOF' }
    $until=[DateTime]::UtcNow.AddSeconds(5)
    do {
        if($null -ne $read.Invoke($wire,@())) { throw 'Unexpected line after EOF' }
        if([DateTime]::UtcNow -gt $until) { throw 'EOF deadline' }
    } while($null -ne [Wdesk.GuestWire].GetField('fast',$flags).GetValue($wire))
    [Console]::Write((@{requests=1000;old_readability_race=$true;utf8_boundary=$true;queued_eof=$true} | ConvertTo-Json -Compress))
} finally { if($null -ne $peer) { $peer.Dispose() };$client.Dispose();$listener.Stop() }

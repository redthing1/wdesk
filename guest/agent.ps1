# Runs in the logged-in console user's STA apartment, never as a Windows service.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -Path "$PSScriptRoot\native.cs"
[Wdesk.Native]::Init()
$processes = @{}
$transfers = @{}
$root = 'C:\ProgramData\wdesk'
$helperId=[Guid]::NewGuid().ToString()
$os=Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
$helperHash = [Security.Cryptography.SHA256]::Create()
$helperBytes = New-Object IO.MemoryStream
foreach ($file in @('agent.ps1','native.cs','a11y.ps1')) {
    $name = [Text.Encoding]::UTF8.GetBytes($file + [char]0)
    $helperBytes.Write($name,0,$name.Length)
    $bytes = [IO.File]::ReadAllBytes("$PSScriptRoot\$file")
    $helperBytes.Write($bytes,0,$bytes.Length)
}
$helperBytes.Position=0
$helperSha256=([BitConverter]::ToString($helperHash.ComputeHash($helperBytes))).Replace('-','').ToLowerInvariant()
$helperBytes.Dispose(); $helperHash.Dispose()
$wire = [Wdesk.GuestWire]::new()

function Check-Fields($object, [string[]]$names) {
    if ($null -eq $object) { return }
    foreach ($p in $object.PSObject.Properties.Name) { if ($p -notin $names) { throw "Unknown field: $p" } }
}
function Hash-Stream($stream) {
    $hash = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($hash.ComputeHash($stream))).Replace('-','').ToLowerInvariant() } finally { $hash.Dispose() }
}
function Process-Result($id) {
    if (-not $processes.ContainsKey($id)) { throw 'Unknown process id (ids expire when helper restarts)' }
    $p = $processes[$id]
    $p.Drain()
    return @{ id=$id; pid=$p.Pid; running=$p.Running; exit_code=$p.ExitCode; output=$p.Output; output_truncated=$p.Truncated; timed_out=$p.TimedOut }
}
function Dispatch($request) {
    Check-Fields $request @('id','op','args')
    $a = $request.args
    switch ($request.op) {
        'health' {
            Check-Fields $a @()
            return @{ protocol=1; helper_version='0.1.0'; helper_id=$helperId; helper_sha256=$helperSha256; interactive=[Wdesk.Native]::Interactive(); session_id=[Diagnostics.Process]::GetCurrentProcess().SessionId; build=[Environment]::OSVersion.Version.ToString(); windows=@{build=$os.CurrentBuild;revision=$os.UBR;version=$os.DisplayVersion;product=$os.ProductName}; user=[Environment]::UserName; profile=(Get-Content "$root\provisioning.json" -Raw | ConvertFrom-Json).profile }
        }
        'windows' { Check-Fields $a @(); return @{ windows=@([Wdesk.Native]::Windows() | ForEach-Object {@{id="${helperId}:$($_.id)";pid=$_.pid;title=$_.title;focused=$_.focused;bounds=$_.bounds}}) } }
        'focus' { Check-Fields $a @('id'); $prefix=$helperId+':'; if (-not ([string]$a.id).StartsWith($prefix)) {throw 'Stale window id; inspect windows again'}; if (-not [Wdesk.Native]::Focus(([string]$a.id).Substring($prefix.Length))) { throw 'Windows refused foreground focus' }; return @{ focused=$true; id=$a.id } }
        'clipboard_get' { Check-Fields $a @(); $text=[Windows.Forms.Clipboard]::GetText(); if ($text.Length -gt 65536) { throw 'Clipboard exceeds limit' }; return @{ text=$text } }
        'clipboard_set' { Check-Fields $a @('text'); $text=[string]$a.text; if ($text.Length -gt 16384) { throw 'Clipboard exceeds limit' }; if ($text.Length) { [Windows.Forms.Clipboard]::SetText($text) } else { [Windows.Forms.Clipboard]::Clear() }; return @{ set=$true } }
        'type_text' { Check-Fields $a @('text'); [Wdesk.Native]::TypeText([string]$a.text); return @{ delivered=$true; method='send_input_unicode'; clipboard_changed=$false } }
        'launch' {
            Check-Fields $a @('argv'); $argv=[string[]]$a.argv
            if ($argv.Count -lt 1 -or $argv.Count -gt 128) { throw 'Invalid argv' }
            $start=New-Object Diagnostics.ProcessStartInfo
            $start.FileName=[Wdesk.Native]::Executable($argv[0]); $start.UseShellExecute=$false
            $start.Arguments=(($argv | Select-Object -Skip 1 | ForEach-Object { [Wdesk.Native]::Quote($_) }) -join ' ')
            $start.WorkingDirectory="$root\workspace"
            $p=[Diagnostics.Process]::Start($start)
            try { return @{ pid=$p.Id; launched=$true } } finally { $p.Dispose() }
        }
        'process_start' {
            Check-Fields $a @('argv','cwd','timeout_seconds')
            if ($processes.Count -ge 128) { throw 'Process registry full (128); restart helper to reclaim ids' }
            $cwd=[Wdesk.Native]::ScopedPath([string]$a.cwd,$true)
            if (-not [IO.Directory]::Exists($cwd)) { throw 'Working directory does not exist' }
            $id=[Guid]::NewGuid().ToString()
            $p=[Wdesk.OwnedProcess]::new([string[]]$a.argv, $cwd, [int]$a.timeout_seconds)
            $processes[$id]=$p
            return Process-Result $id
        }
        'process_status' { Check-Fields $a @('id'); return Process-Result ([string]$a.id) }
        'process_kill' { Check-Fields $a @('id'); if (-not $processes.ContainsKey([string]$a.id)) { throw 'Unknown process id' }; $processes[[string]$a.id].Kill(); return Process-Result ([string]$a.id) }
        'file_begin' {
            Check-Fields $a @('path','transfer','size')
            $id=[string]$a.transfer
            if ($id -notmatch '^[a-f0-9-]{36}$' -or $transfers.ContainsKey($id) -or $transfers.Count -ge 8) { throw 'Invalid or duplicate transfer' }
            $null=[Wdesk.Native]::ScopedPath([string]$a.path,$false)
            $size=[long]$a.size; if ($size -lt 0 -or $size -gt 4294967296) { throw 'File exceeds 4 GiB' }
            $stream=[IO.File]::Open("$root\staging\$id.partial",[IO.FileMode]::CreateNew,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
            $transfers[$id]=@{ stream=$stream; path=[string]$a.path; size=$size; touched=[DateTime]::UtcNow }
            return @{ transfer=$id; offset=0 }
        }
        'file_write' {
            Check-Fields $a @('transfer','offset','data'); $id=[string]$a.transfer
            if (-not $transfers.ContainsKey($id)) { throw 'Unknown transfer' }; $t=$transfers[$id]
            $bytes=[Convert]::FromBase64String([string]$a.data)
            if ($bytes.Length -gt 49152 -or [long]$a.offset -ne $t.stream.Position -or $t.stream.Position+$bytes.Length -gt $t.size) { throw 'Invalid transfer offset/size' }
            $t.stream.Write($bytes,0,$bytes.Length); $t.touched=[DateTime]::UtcNow
            return @{ offset=$t.stream.Position }
        }
        'file_commit' {
            Check-Fields $a @('transfer','sha256'); $id=[string]$a.transfer
            if (-not $transfers.ContainsKey($id)) { throw 'Unknown transfer' }; $t=$transfers[$id]
            if ($t.stream.Length -ne $t.size) { throw 'Incomplete transfer' }
            $t.stream.Position=0; $hash=Hash-Stream $t.stream
            if ($hash -ne [string]$a.sha256) { throw 'Transfer SHA-256 mismatch' }
            $destination=[Wdesk.Native]::OpenScoped($t.path,[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite)
            try { $t.stream.Position=0; $destination.SetLength(0); $t.stream.CopyTo($destination); $destination.Flush($true) } finally { $destination.Dispose() }
            $t.stream.Dispose(); [IO.File]::Delete("$root\staging\$id.partial"); $transfers.Remove($id)
            return @{ path=$t.path; size=$t.size; sha256=$hash }
        }
        'file_abort' {
            Check-Fields $a @('transfer'); $id=[string]$a.transfer
            if ($transfers.ContainsKey($id)) { $transfers[$id].stream.Dispose(); $transfers.Remove($id); [IO.File]::Delete("$root\staging\$id.partial") }
            return @{ aborted=$true }
        }
        'file_stat' {
            Check-Fields $a @('path'); $stream=[Wdesk.Native]::OpenScoped([string]$a.path,[IO.FileMode]::Open,[IO.FileAccess]::Read)
            try { return @{ path=$a.path; size=$stream.Length; sha256=(Hash-Stream $stream) } } finally { $stream.Dispose() }
        }
        'file_read' {
            Check-Fields $a @('path','offset','length'); $length=[int]$a.length
            if ($length -lt 1 -or $length -gt 49152 -or [long]$a.offset -lt 0) { throw 'Invalid read range' }
            $stream=[Wdesk.Native]::OpenScoped([string]$a.path,[IO.FileMode]::Open,[IO.FileAccess]::Read)
            try { $stream.Position=[long]$a.offset; $bytes=New-Object byte[] $length; $n=$stream.Read($bytes,0,$length); return @{ data=[Convert]::ToBase64String($bytes,0,$n); bytes=$n } } finally { $stream.Dispose() }
        }
        'a11y' {
            Check-Fields $a @('max_nodes','max_depth')
            $nodes=[int]$a.max_nodes; $depth=[int]$a.max_depth
            if ($nodes -lt 1 -or $nodes -gt 1024 -or $depth -lt 1 -or $depth -gt 12) { throw 'Invalid accessibility bounds' }
            $argv=@('powershell.exe','-NoProfile','-STA','-ExecutionPolicy','Bypass','-File',"$PSScriptRoot\a11y.ps1",'-MaxNodes',[string]$nodes,'-MaxDepth',[string]$depth)
            $p=[Wdesk.OwnedProcess]::new([string[]]$argv, "$root\workspace", 8, 1048576)
            try {
                while ($p.Running) { Start-Sleep -Milliseconds 50 }; $p.Drain()
                if ($p.TimedOut) { throw 'UI Automation provider deadline exceeded; worker terminated' }
                if ($p.ExitCode -ne 0) { throw "UI Automation failed: $($p.Output)" }
                return $p.Output | ConvertFrom-Json
            } finally { $p.Dispose() }
        }
        default { throw "Unsupported guest operation: $($request.op)" }
    }
}

try {
    while ($true) {
        $line=$wire.ReadRequest()
        if ($null -eq $line) { Start-Sleep -Milliseconds 5; continue }
        $request=$null
        try {
            $request=$line | ConvertFrom-Json
            $result=Dispatch $request
            $response=@{ id=$request.id; ok=$true; result=$result }
        } catch { $response=@{ id=$request.id; ok=$false; error=$_.Exception.Message } }
        try { $wire.Respond(($response | ConvertTo-Json -Depth 24 -Compress)) } catch { $_ | Out-String | Add-Content "$root\agent-error.log" }
        foreach ($id in @($transfers.Keys)) {
            if (([DateTime]::UtcNow-$transfers[$id].touched).TotalMinutes -gt 10) {
                $transfers[$id].stream.Dispose(); $transfers.Remove($id); [IO.File]::Delete("$root\staging\$id.partial")
            }
        }
    }
} catch { $_ | Out-String | Add-Content "$root\agent-error.log"; throw }
finally {
    foreach ($p in $processes.Values) { $p.Dispose() }
    foreach ($t in $transfers.Values) { $t.stream.Dispose() }
    $wire.Dispose()
}

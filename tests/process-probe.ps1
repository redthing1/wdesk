$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -Path 'C:\ProgramData\wdesk\native.cs'
$registry=[Wdesk.Processes]::new()
$cwd='C:\ProgramData\wdesk\workspace'
$flags=[Reflection.BindingFlags]'Instance,NonPublic'
function Completed($id) {
    $until=[DateTime]::UtcNow.AddSeconds(10)
    do {
        $result=$registry.Get($id)
        if($result.phase -eq 'completed') { return $result }
        if([DateTime]::UtcNow -gt $until) { throw 'Completion deadline' }
        Start-Sleep -Milliseconds 10
    } while($true)
}
try {
    $first=$null
    for($i=0;$i -lt 160;$i++) {
        $started=$registry.Start([string[]]@('cmd.exe','/d','/c','echo process-retirement'),$cwd,10,$null)
        if($i -eq 0) { $first=$started.id }
        $result=Completed $started.id
        if($result.exit_code -ne 0 -or !$result.output_complete -or $result.output -notmatch 'process-retirement') { throw 'Incorrect completion' }
    }
    $evicted=$false
    try { $null=$registry.Get($first) } catch { $evicted=$true }
    if(!$evicted) { throw 'Old receipt was not evicted' }
    $registry.Forget($result.id)
    $forgotten=$false
    try { $null=$registry.Get($result.id) } catch { $forgotten=$true }
    if(!$forgotten) { throw 'Receipt survived forget' }
    $started=$registry.Start([string[]]@('powershell.exe','-NoProfile','-Command','Start-Sleep -Seconds 30'),$cwd,2,$null)
    $refused=$false
    try { $registry.Forget($started.id) } catch { $refused=$true }
    if(!$refused) { throw 'Forgot an active process' }
    $timeout=Completed $started.id
    if(!$timeout.timed_out -or $timeout.running -or !$timeout.output_complete) { throw 'Deadline did not clean up process' }
    $parent=$registry.Start([string[]]@('powershell.exe','-NoProfile','-Command','$child=Start-Process powershell.exe -ArgumentList @("-NoProfile","-Command","Start-Sleep -Seconds 30") -PassThru; [Console]::Write($child.Id)'),$cwd,10,$null)
    $parent=Completed $parent.id
    if($parent.exit_code -ne 0 -or !$parent.output_complete) { throw 'Parent completion failed' }
    $alive=$false
    try { $child=[Diagnostics.Process]::GetProcessById([int]$parent.output); try { $alive=!$child.HasExited } finally { $child.Dispose() } } catch [ArgumentException] {}
    if($alive) { throw 'Descendant outlived its owned parent' }
    # Advance receipt timestamps, not production TTL. Let the independent timer
    # expire them without any further Get/Start/Forget calls.
    $receipts=$registry.GetType().GetField('completed',$flags).GetValue($registry)
    $gate=$registry.GetType().GetField('gate',$flags).GetValue($registry)
    [Threading.Monitor]::Enter($gate)
    try {
        foreach($receipt in $receipts.Values) {
            $receipt.Completed=[Diagnostics.Stopwatch]::GetTimestamp()-601L*[Diagnostics.Stopwatch]::Frequency
        }
    } finally { [Threading.Monitor]::Exit($gate) }
    Start-Sleep -Milliseconds 800
    [Threading.Monitor]::Enter($gate)
    try { if($receipts.Count -ne 0) { throw 'Idle receipt expiry did not run' } }
    finally { [Threading.Monitor]::Exit($gate) }
    [Console]::Write((@{processes=160;eviction=$true;forget=$true;active_forget_refused=$true;deadline=$true;descendants_retired=$true;independent_expiry=$true} | ConvertTo-Json -Compress))
} finally { $registry.Dispose() }

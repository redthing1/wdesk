param([string]$Expected='owner granted source',[switch]$Elevated)
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$source='\\10.0.2.102\source'
$output='\\10.0.2.102\output'
if([IO.File]::ReadAllText("$source\source.txt").TrimEnd("`n") -ne $Expected) { throw 'live source mismatch' }
$readonly=$false
try { [IO.File]::WriteAllText("$source\forbidden.txt",'forbidden') } catch { $readonly=$true }
if(-not $readonly) { throw 'read-only policy failed' }
foreach($path in @("$source\outside-link.txt","$output\outside-link.txt",'\\10.0.2.102\not-granted\outside.txt')) {
    $denied=$false
    try { [IO.File]::ReadAllText($path) | Out-Null } catch { $denied=$true }
    if(-not $denied) { throw 'share boundary failed' }
}
$unicode=([string][char]0x65e5)+[char]0x672c+[char]0x8a9e+' '+[char]0xd83d+[char]0xdc08+'.txt'
[IO.File]::WriteAllText("$output\$unicode",'literal $name; useful output',[Text.Encoding]::UTF8)
if([IO.File]::ReadAllText("$output\$unicode") -ne 'literal $name; useful output') { throw 'Unicode output mismatch' }
if([IO.File]::Exists("$output\renamed.txt")) { [IO.File]::Delete("$output\renamed.txt") }
[IO.File]::Move("$output\$unicode","$output\renamed.txt")
$stream=[IO.File]::Open("$output\lock.txt",[IO.FileMode]::Create,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
$locked=$false
try {
    try { $other=[IO.File]::Open("$output\lock.txt",[IO.FileMode]::Open,[IO.FileAccess]::Read);$other.Dispose() } catch { $locked=$true }
} finally { $stream.Dispose() }
if(-not $locked) { throw 'SMB locking failed' }
$connections=@()
if($Elevated) {
    $principal=[Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
    if(-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'not elevated' }
    $connections=@(Get-SmbConnection | Where-Object ServerName -eq '10.0.2.102' | Select-Object ShareName,Dialect,Signed)
    if(-not $connections.Count -or @($connections | Where-Object { -not $_.Signed }).Count) { throw 'unsigned SMB connection' }
}
@{source_read=$true;read_only=$readonly;symlink_denied=$true;unicode_rename=$true;unicode_name=$unicode;locking=$locked;architecture=if([Environment]::Is64BitProcess){'x64'}else{'x86'};connections=$connections} | ConvertTo-Json -Depth 6 -Compress

param([Parameter(Mandatory=$true)][string]$Directory)
$ErrorActionPreference='Stop'
$root='C:\ProgramData\wdesk\workspace\'
$directory=[IO.Path]::GetFullPath($Directory)
if (-not $directory.StartsWith($root,[StringComparison]::OrdinalIgnoreCase)) { throw 'Use a guest workspace directory' }
$compiler="$env:WINDIR\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
foreach ($platform in @('anycpu','x64','x86','anycpu32bitpreferred')) {
    $target=Join-Path $directory $platform
    [IO.Directory]::CreateDirectory($target) | Out-Null
    & $compiler /nologo /target:exe "/platform:$platform" "/out:$target\probe.exe" "$directory\graphics-env.cs"
    if ($LASTEXITCODE -ne 0) { throw "Managed fixture compilation failed: $platform" }
}

param([int]$MaxNodes=256,[int]$MaxDepth=6)
$ErrorActionPreference='Stop'
[Console]::OutputEncoding = New-Object Text.UTF8Encoding $false
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$walker=[Windows.Automation.TreeWalker]::ControlViewWalker
$queue=New-Object Collections.Queue
$queue.Enqueue(@{element=[Windows.Automation.AutomationElement]::RootElement; depth=0; parent=$null})
$nodes=New-Object Collections.Generic.List[object]
$truncated=$false
while ($queue.Count -gt 0 -and $nodes.Count -lt $MaxNodes) {
    $entry=$queue.Dequeue(); $element=$entry.element; $id=$nodes.Count
    try {
        $c=$element.Current; $r=$c.BoundingRectangle
        # Rect.Empty uses infinities; those are not JSON numbers or coordinates.
        $bounds=$null
        if (-not $r.IsEmpty) {
            $finite=$true
            foreach ($value in @($r.X,$r.Y,$r.Width,$r.Height)) {
                if ([double]::IsNaN($value) -or [double]::IsInfinity($value)) { $finite=$false; break }
            }
            if ($finite -and $r.Width -ge 0 -and $r.Height -ge 0) { $bounds=@{x=$r.X;y=$r.Y;width=$r.Width;height=$r.Height} }
        }
        $name=$c.Name; if ($name.Length -gt 128) { $name=$name.Substring(0,128) }
        $automationId=$c.AutomationId; if ($automationId.Length -gt 128) { $automationId=$automationId.Substring(0,128) }
        $nodes.Add(@{id=$id;parent=$entry.parent;name=$name;automation_id=$automationId;control_type=$c.ControlType.ProgrammaticName;enabled=$c.IsEnabled;offscreen=$c.IsOffscreen;bounds=$bounds;patterns=@($element.GetSupportedPatterns() | Select-Object -First 16 | ForEach-Object {$_.ProgrammaticName})})
        if ($entry.depth -lt $MaxDepth) {
            $child=$walker.GetFirstChild($element)
            while ($null -ne $child) {
                if ($queue.Count+$nodes.Count -ge $MaxNodes) { $truncated=$true; break }
                $queue.Enqueue(@{element=$child;depth=$entry.depth+1;parent=$id})
                $child=$walker.GetNextSibling($child)
            }
        } else { $truncated=$true }
    } catch { $nodes.Add(@{id=$id;parent=$entry.parent;unavailable=$true}) }
}
@{ nodes=@($nodes.ToArray()); truncated=($truncated -or $queue.Count -gt 0); supplementary=$true } | ConvertTo-Json -Depth 20 -Compress

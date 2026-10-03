# A deterministic inbox-only desktop target, independent of media-bundled apps.
Add-Type -AssemblyName System.Windows.Forms
$form = New-Object System.Windows.Forms.Form
$form.Text = 'wdesk input probe'
$form.Width = 640
$form.Height = 400
$text = New-Object System.Windows.Forms.TextBox
$text.Multiline = $true
$text.Dock = 'Fill'
$form.Controls.Add($text)
$form.Add_Shown({ $text.Focus() })
[void]$form.ShowDialog()

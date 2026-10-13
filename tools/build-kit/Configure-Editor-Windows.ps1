[CmdletBinding()]
param(
    [string]$Repository = ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))),
    [string]$FlutterWindowPath,
    [switch]$Check
)
$ErrorActionPreference = 'Stop'
if (!$FlutterWindowPath) {
    $FlutterWindowPath = Join-Path $Repository 'ui\flutter\ad_remote_desktop\windows\runner\flutter_window.cpp'
}
if (!(Test-Path -LiteralPath $FlutterWindowPath -PathType Leaf)) {
    throw "Flutter Windows runner is missing. Run the PrepareRunner stage first: $FlutterWindowPath"
}
$text = [IO.File]::ReadAllText($FlutterWindowPath)
$newline = if ($text.Contains("`r`n")) { "`r`n" } else { "`n" }
$header = '#include "flutter/generated_plugin_registrant.h"'
$include = '#include "desktop_multi_window/desktop_multi_window_plugin.h"'
$anchor = 'RegisterPlugins(flutter_controller_->engine());'
$callback = @'
  // AetherDesk editor windows use their own Flutter engine.
  DesktopMultiWindowSetWindowCreatedCallback([](void* controller) {
    auto* editor_controller =
        static_cast<flutter::FlutterViewController*>(controller);
    RegisterPlugins(editor_controller->engine());
  });
'@ -replace "`r?`n", $newline
if ([regex]::Matches($text, [regex]::Escape($header)).Count -ne 1 -or
    [regex]::Matches($text, [regex]::Escape($anchor)).Count -ne 1) {
    throw 'Unsupported Flutter runner scaffold: expected one generated plugin header and one main engine registration.'
}
$updated = $text
if (!$updated.Contains($include)) {
    $updated = $updated.Replace($header, $header + $newline + $include)
}
if (!$updated.Contains('DesktopMultiWindowSetWindowCreatedCallback')) {
    $updated = $updated.Replace($anchor, $anchor + $newline + $callback)
}
if ([regex]::Matches($updated, [regex]::Escape($include)).Count -ne 1 -or
    [regex]::Matches($updated, 'DesktopMultiWindowSetWindowCreatedCallback').Count -ne 1 -or
    !$updated.Contains($callback)) {
    throw 'Conflicting editor window registration: preserve a single AetherDesk child engine callback.'
}
if ($Check) {
    if ($text -ne $updated) { throw 'The Windows runner is missing editor popup plugin registration. Run PrepareRunner again.' }
    return
}
if ($text -ne $updated) {
    [IO.File]::WriteAllText($FlutterWindowPath, $updated, [Text.UTF8Encoding]::new($false))
}

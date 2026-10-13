$ErrorActionPreference = 'Stop'
$helper = Join-Path $PSScriptRoot 'Configure-Editor-Windows.ps1'
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('aetherdesk-editor-window-recipe-' + [guid]::NewGuid().ToString('N'))
$fixture = @'
#include "flutter_window.h"
#include "flutter/generated_plugin_registrant.h"

bool FlutterWindow::OnCreate() {
  RegisterPlugins(flutter_controller_->engine());
  SetChildContent(flutter_controller_->view()->GetNativeWindow());
  return true;
}
'@
function Expect-RunnerFailure([scriptblock]$Action, [string[]]$Expected) {
    try { & $Action } catch {
        $errorMessage = $_.Exception.Message
        if (!($Expected | Where-Object { $errorMessage -like $_ })) { throw }
        return
    }
    throw "Expected runner configuration to fail: $Expected"
}
try {
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    $runner = Join-Path $testRoot 'flutter_window.cpp'
    $main = Join-Path $testRoot 'main.cpp'
    $title = 'window.Create(L"AetherDesk", origin, size);'
    [IO.File]::WriteAllText($main, $title)
    foreach ($newline in @("`n", "`r`n")) {
        $original = $fixture -replace "`r?`n", $newline
        [IO.File]::WriteAllText($runner, $original)
        Expect-RunnerFailure { & $helper -FlutterWindowPath $runner -Check } '*missing editor popup plugin registration*'
        if ([IO.File]::ReadAllText($runner) -ne $original) { throw 'Check must not modify the runner.' }
        & $helper -FlutterWindowPath $runner
        $configured = [IO.File]::ReadAllText($runner)
        if (!$configured.Contains('RegisterPlugins(editor_controller->engine());') -or
            [regex]::Matches($configured, 'DesktopMultiWindowSetWindowCreatedCallback').Count -ne 1 -or
            [regex]::Matches($configured, 'desktop_multi_window/desktop_multi_window_plugin.h').Count -ne 1) {
            throw 'Child windows must register all generated plugins exactly once.'
        }
        & $helper -FlutterWindowPath $runner
        & $helper -FlutterWindowPath $runner -Check
        if ([IO.File]::ReadAllText($runner) -ne $configured) { throw 'Runner configuration is not idempotent.' }
        if ($newline -eq "`r`n" -and [regex]::IsMatch($configured, '(?<!\r)\n')) { throw 'Runner configuration changed CRLF line endings.' }
    }
    foreach ($invalid in @(
        $fixture.Replace('RegisterPlugins(flutter_controller_->engine());', 'RegisterPlugins(other_engine);'),
        ($fixture + "`n#include `"flutter/generated_plugin_registrant.h`""),
        ($fixture + "`nDesktopMultiWindowSetWindowCreatedCallback(nullptr);")
    )) {
        [IO.File]::WriteAllText($runner, $invalid)
        Expect-RunnerFailure { & $helper -FlutterWindowPath $runner } '*runner scaffold*','*Conflicting*'
        if ([IO.File]::ReadAllText($runner) -ne $invalid) { throw 'Invalid runner should remain unmodified.' }
    }
    if ([IO.File]::ReadAllText($main) -ne $title) { throw 'Main application window title was modified.' }
    Expect-RunnerFailure { & $helper -FlutterWindowPath (Join-Path $testRoot 'missing.cpp') } '*runner is missing*'
} finally {
    $resolved = [IO.Path]::GetFullPath($testRoot)
    $temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (!$resolved.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($resolved) -notlike 'aetherdesk-editor-window-recipe-*') {
        throw 'Invalid editor window test cleanup path.'
    }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
Write-Host 'PASS: editor child plugin registration, idempotence, line endings, scaffold rejection and main window title preservation.'

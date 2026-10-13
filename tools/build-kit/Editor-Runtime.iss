; Shared by local and CI installers. The complete runtime is copied by [Files].
[Code]
function HasEditorRuntime: Boolean;
var
  Version: String;
begin
  Result := False;
  if RegQueryStringValue(HKLM32, 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', Version) then
    Result := (Version <> '') and (Version <> '0.0.0.0');
  if not Result then
    if RegQueryStringValue(HKCU, 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', Version) then
      Result := (Version <> '') and (Version <> '0.0.0.0');
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if (CurStep = ssPostInstall) and not HasEditorRuntime then begin
    if not Exec(ExpandConstant('{app}\redist\MicrosoftEdgeWebView2RuntimeInstallerX64.exe'), '/silent /install', '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
      Log('The editor runtime installer could not be started.');
    if not HasEditorRuntime then
      MsgBox('AetherDesk was installed, but its code editor needs Microsoft Edge WebView2 Runtime. Run Install-Editor-Runtime.cmd in the AetherDesk folder, or ask your administrator to install the runtime.', mbInformation, MB_OK);
  end;
end;

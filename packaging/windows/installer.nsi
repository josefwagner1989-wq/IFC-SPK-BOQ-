; IFC Sprinkler Viewer – Setup (ALPHA) – Created by Josef Wagner
; Gerar: makensis -DSRC=<pasta> -DOUT=<setup.exe> installer.nsi
Unicode true
!include "MUI2.nsh"

!define APP "IFC Sprinkler Viewer"
!define VER "0.1.0-alpha"
!define EXE "IFCSprinklerViewer.exe"
!define REGKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\IFCSprinklerViewer"

Name "${APP} (Alpha)"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\IFC Sprinkler Viewer"
RequestExecutionLevel user
SetCompressor /SOLID lzma
BrandingText "ALPHA ${VER} · Created by Josef Wagner"
VIProductVersion "0.1.0.0"
VIAddVersionKey /LANG=1031 "ProductName" "${APP} (Alpha)"
VIAddVersionKey /LANG=1031 "CompanyName" "Josef Wagner"
VIAddVersionKey /LANG=1031 "LegalCopyright" "© 2026 Josef Wagner"
VIAddVersionKey /LANG=1031 "FileDescription" "${APP} Setup – ALPHA"
VIAddVersionKey /LANG=1031 "FileVersion" "${VER}"
VIAddVersionKey /LANG=1031 "ProductVersion" "${VER}"

!define MUI_ICON "${SRC}\icon.ico"
!define MUI_UNICON "${SRC}\icon.ico"
!define MUI_WELCOMEPAGE_TITLE "${APP} – ALPHA ${VER}"
!define MUI_WELCOMEPAGE_TEXT "Willkommen zur Installation des IFC Sprinkler Viewers.$\r$\n$\r$\nIFC-Modelle öffnen, Sprinkleranlage filtern, vereinfacht in 3D ansehen und einen Mengenauszug (Rohre, Dämmung, Formstücke, Armaturen & Zubehör) nach Excel exportieren.$\r$\n$\r$\nACHTUNG – ALPHA-VERSION: Vorabversion zum Testen. Mengen bitte stichprobenartig prüfen.$\r$\n$\r$\nCreated by Josef Wagner"
!define MUI_LICENSEPAGE_TEXT_TOP "Bitte den Hinweis zur Alpha-Version lesen."
!define MUI_LICENSEPAGE_BUTTON "Verstanden"
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "IFC Sprinkler Viewer jetzt starten"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${SRC}\LIESMICH-ALPHA.txt"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "German"

Section "Installieren"
  SetOutPath "$INSTDIR"
  File "${SRC}\${EXE}"
  File "${SRC}\WebView2Loader.dll"
  File "${SRC}\LIESMICH-ALPHA.txt"
  File "${SRC}\icon.ico"
  WriteUninstaller "$INSTDIR\Deinstallieren.exe"
  CreateDirectory "$SMPROGRAMS\IFC Sprinkler Viewer"
  CreateShortcut "$SMPROGRAMS\IFC Sprinkler Viewer\IFC Sprinkler Viewer (Alpha).lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\icon.ico"
  CreateShortcut "$SMPROGRAMS\IFC Sprinkler Viewer\Deinstallieren.lnk" "$INSTDIR\Deinstallieren.exe"
  CreateShortcut "$DESKTOP\IFC Sprinkler Viewer (Alpha).lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\icon.ico"
  ; "Abrir com" para .ifc / .ifczip
  WriteRegStr HKCU "Software\Classes\JosefWagner.IfcSprinklerViewer.ifc" "" "IFC-Modell"
  WriteRegStr HKCU "Software\Classes\JosefWagner.IfcSprinklerViewer.ifc\DefaultIcon" "" "$INSTDIR\${EXE},0"
  WriteRegStr HKCU "Software\Classes\JosefWagner.IfcSprinklerViewer.ifc\shell\open\command" "" '"$INSTDIR\${EXE}" "%1"'
  WriteRegStr HKCU "Software\Classes\.ifc\OpenWithProgids" "JosefWagner.IfcSprinklerViewer.ifc" ""
  WriteRegStr HKCU "Software\Classes\.ifczip\OpenWithProgids" "JosefWagner.IfcSprinklerViewer.ifc" ""
  ; Painel de controle › Programas
  WriteRegStr HKCU "${REGKEY}" "DisplayName" "${APP} (Alpha)"
  WriteRegStr HKCU "${REGKEY}" "DisplayVersion" "${VER}"
  WriteRegStr HKCU "${REGKEY}" "Publisher" "Josef Wagner"
  WriteRegStr HKCU "${REGKEY}" "DisplayIcon" "$INSTDIR\icon.ico"
  WriteRegStr HKCU "${REGKEY}" "UninstallString" '"$INSTDIR\Deinstallieren.exe"'
  WriteRegStr HKCU "${REGKEY}" "Comments" "ALPHA – Vorabversion zum Testen. Created by Josef Wagner."
  WriteRegDWORD HKCU "${REGKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${REGKEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\WebView2Loader.dll"
  Delete "$INSTDIR\LIESMICH-ALPHA.txt"
  Delete "$INSTDIR\icon.ico"
  Delete "$INSTDIR\Deinstallieren.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\IFC Sprinkler Viewer\*.lnk"
  RMDir "$SMPROGRAMS\IFC Sprinkler Viewer"
  Delete "$DESKTOP\IFC Sprinkler Viewer (Alpha).lnk"
  DeleteRegKey HKCU "Software\Classes\JosefWagner.IfcSprinklerViewer.ifc"
  DeleteRegValue HKCU "Software\Classes\.ifc\OpenWithProgids" "JosefWagner.IfcSprinklerViewer.ifc"
  DeleteRegValue HKCU "Software\Classes\.ifczip\OpenWithProgids" "JosefWagner.IfcSprinklerViewer.ifc"
  DeleteRegKey HKCU "${REGKEY}"
SectionEnd

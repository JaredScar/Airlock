!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Installing the CUDA toolkit used for speech recognition..."
  nsExec::ExecToLog 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\cuda-setup.ps1" -InstallDir "$INSTDIR"'
  Pop $0
!macroend

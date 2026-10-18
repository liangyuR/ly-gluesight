!macro NSIS_HOOK_PREINSTALL
  SetShellVarContext all
  System::Call 'kernel32::GetFileAttributesW(w "$APPDATA\ly") i.r0'
  IntCmp $0 -1 ly_parent_safe
  IntOp $1 $0 & 0x400
  IntCmp $1 0 ly_parent_safe
  MessageBox MB_ICONSTOP "资料目录的父目录是链接，安装已停止：$APPDATA\ly"
  Abort
  ly_parent_safe:
  System::Call 'kernel32::GetFileAttributesW(w "$APPDATA\ly\GlueSight") i.r0'
  IntCmp $0 -1 ly_data_safe
  IntOp $1 $0 & 0x400
  IntCmp $1 0 ly_data_safe
  MessageBox MB_ICONSTOP "资料目录是链接，安装已停止：$APPDATA\ly\GlueSight"
  Abort
  ly_data_safe:
  CreateDirectory "$APPDATA\ly\GlueSight"
  nsExec::ExecToStack '"$SYSDIR\icacls.exe" "$APPDATA\ly\GlueSight" /grant "*S-1-5-32-545:(OI)(CI)M"'
  Pop $0
  Pop $1
  StrCmp $0 "0" ly_acl_ready
  MessageBox MB_ICONSTOP "无法授权 GlueSight 资料目录，安装已停止。$\r$\n$1"
  Abort
  ly_acl_ready:
!macroend

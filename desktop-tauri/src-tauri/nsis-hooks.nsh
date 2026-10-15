; Agent2API 的 NSIS 安装器钩子（由 tauri.conf.json 的
; bundle.windows.nsis.installerHooks 引入）。
;
; ── 这个钩子补的是什么 ──────────────────────────────────────────
; 主程序的产物名从 `workbuddy-proxy-desktop.exe` 改成了 `agent2api.exe`
; （tauri.conf.json 的 mainBinaryName）。模板自带的「应用正在运行」检查只认
; 当前这一个名字（installer.nsi 里的 `CheckIfAppIsRunning "${MAINBINARYNAME}.exe"`），
; 于是「用户手动运行新安装包、旧版本还开着」时模板认不出那个旧进程：
; 安装本身会成功，但紧接着模板删旧主程序的那一步
; （`Delete "$INSTDIR\$OldMainBinaryName"`）会因为文件被占用而静默失败，
; 旧 exe 就留在安装目录里 —— 白占十几 MB 之外，卸载时结尾那句
; `RMDir "$INSTDIR"` 也会因为这个残留文件删不掉整个目录。
;
; 这里在复制文件之前对旧名进程做一次同样的检查，交互与模板逐字一致：
; 静默安装直接结束它，图形安装弹「是否结束该程序」，用户点取消就中止安装。
;
; ── 什么时候可以删掉 ────────────────────────────────────────────
; 这是过渡期补丁，只在「用户机器上还可能装着改名前的版本」时有意义。
; 改名后的版本发出足够久（用户基本都升过一次）之后，磁盘上不会再出现旧名主程序，
; 届时删掉本文件与 tauri.conf.json 里的 installerHooks 即可。
; 若主程序将来再次改名，按同一思路把「上一次的名字」加进来。

!macro NSIS_HOOK_PREINSTALL
  !insertmacro CheckIfAppIsRunning "workbuddy-proxy-desktop.exe" "${PRODUCTNAME}"
!macroend

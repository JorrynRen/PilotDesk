; PilotDesk —— NSIS 安装器钩子（由 tauri.conf.json 的 bundle.windows.nsis.installerHooks 引入）
;
; 为什么需要这个文件
; ─────────────────
; Tauri 的 NSIS 模板里，"删除用户数据"是按 **bundle identifier** 删的：
;
;   ${If} $DeleteAppDataCheckboxState = 1
;     SetShellVarContext current
;     RmDir /r "$APPDATA\${BUNDLEID}"        ; %APPDATA%\com.pilotdesk.desktop
;     RmDir /r "$LOCALAPPDATA\${BUNDLEID}"   ; %LOCALAPPDATA%\com.pilotdesk.desktop
;   ${EndIf}
;
; 而 PilotDesk 的用户数据放在**产品名**目录下（见 src/utils/paths.rs 的 app_root_dir）：
;
;   %APPDATA%\PilotDesk\   ← pilotdesk.db / MEMORY.db / .key / plugins / skills / resources / templates
;
; 两者对不上，于是"勾选删除用户数据"实际删掉的是两个基本不存在的标识符目录
; （其中只有 WebView2 的 EBWebView），我们自己的数据一个字节都不会被清理。
;
; 这里在卸载收尾阶段补一刀，**严格跟随同一个复选框状态**（没勾就不动用户数据），
; 与内置行为保持同一语义，只是把目录名换成我们真正在用的那个。

!macro NSIS_HOOK_POSTUNINSTALL
  ; 三个条件与模板内置判断完全一致：
  ;   $DeleteAppDataCheckboxState = 1 → 用户勾了"删除用户数据"
  ;   $UpdateMode <> 1               → 这是卸载，不是"升级覆盖安装"
  ; 升级时若删数据，用户只是在更新版本，却会丢掉全部会话与记忆。
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    ; 与内置删除同一上下文：只作用于当前用户，不碰其他账户的数据
    SetShellVarContext current

    ; 用户数据根（Roaming）——应用真正在用的那一份
    RmDir /r "$APPDATA\PilotDesk"

    ; Local 侧同名目录：老版本 per-user 安装方式下这里曾是安装目录，
    ; 现在安装已改为 perMachine（Program Files），此目录只可能剩安装残留，一并收拾。
    RmDir /r "$LOCALAPPDATA\PilotDesk"
  ${EndIf}
!macroend

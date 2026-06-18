@echo off
cd /d "E:\WorkSpace_HermesAgent\pilotdeskProject\pilotdesk"
echo Starting PilotDesk Dev...
echo.
echo This window will show Tauri + Vite + Rust build output
echo.
echo If Rust is compiling for the first time, this may take 5-15 minutes.
echo.
node node_modules\@tauri-apps\cli\tauri.js dev
pause

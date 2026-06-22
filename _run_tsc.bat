@echo off
cd /d "E:\WorkSpace_HermesAgent\pilotdeskProject\PilotDesk"
call npx.cmd tsc --project tsconfig.app.json --noEmit 2>&1

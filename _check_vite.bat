@echo off
cd /d "E:\WorkSpace_HermesAgent\pilotdeskProject\pilotdesk"
call npx.cmd vite build > "E:\WorkSpace_HermesAgent\pilotdeskProject\pilotdesk\_vite_output.txt" 2>&1
echo EXIT_CODE=%errorlevel%

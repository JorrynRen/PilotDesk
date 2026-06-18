@echo off
cd /d "E:\WorkSpace_HermesAgent\pilotdeskProject\pilotdesk"
call node_modules\.bin\tsc.cmd -p tsconfig.app.json --noEmit > "E:\WorkSpace_HermesAgent\pilotdeskProject\pilotdesk\_tsc_output.txt" 2>&1
echo EXIT_CODE=%errorlevel%

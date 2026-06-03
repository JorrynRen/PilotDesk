// Test script for tauri CLI native module loading
const cliDir = 'E:/LingXi Workspace/20260530-10-40-29-689/pilotdesk/node_modules/@tauri-apps/cli';

try {
  const m = require(cliDir + '/cli.win32-x64-msvc.node');
  console.log('Native module loaded OK, keys:', Object.keys(m).join(','));
} catch (e) {
  console.error('Native module load error:', e.message);
  console.error(e.stack);
}

try {
  const cli = require(cliDir);
  console.log('CLI loaded OK, run type:', typeof cli.run);
} catch (e) {
  console.error('CLI load error:', e.message);
  console.error(e.stack);
}

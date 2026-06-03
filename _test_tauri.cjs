const fs = require('fs');
const cliDir = 'E:/LingXi Workspace/20260530-10-40-29-689/pilotdesk/node_modules/@tauri-apps/cli';
const logFile = 'E:/LingXi Workspace/20260530-10-40-29-689/pilotdesk/_test_result.txt';

function log(msg) {
  fs.appendFileSync(logFile, msg + '\n');
  process.stderr.write(msg + '\n');
}

log('Node version: ' + process.version);
log('Platform: ' + process.platform + ' ' + process.arch);

try {
  log('Attempting to load native module...');
  const m = require(cliDir + '/cli.win32-x64-msvc.node');
  log('Native module loaded OK, keys: ' + Object.keys(m).join(','));
} catch (e) {
  log('Native module load error: ' + e.message);
  log('Stack: ' + e.stack);
}

try {
  log('Attempting to load CLI...');
  const cli = require(cliDir);
  log('CLI loaded OK, run type: ' + typeof cli.run);
} catch (e) {
  log('CLI load error: ' + e.message);
  log('Stack: ' + e.stack);
}

log('DONE');

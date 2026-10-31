import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const desktop = join(dirname(fileURLToPath(import.meta.url)), '..');
const cargo = join(process.env.USERPROFILE ?? '', '.cargo', 'bin', 'cargo.exe');

function run(command, args, cwd) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, stdio: 'inherit' });
    child.on('error', reject);
    child.on('exit', (code) => {
      if (code === 0) resolve();
      else reject(new Error(`${command} exited ${code}`));
    });
  });
}

if (!existsSync(cargo)) {
  throw new Error('cargo.exe was not found in %USERPROFILE%\\.cargo\\bin');
}

await run(cargo, [
  'test',
  '--manifest-path',
  join(desktop, '../../Cargo.toml'),
  '-p',
  'airlock-core',
  '-p',
  'airlock-pipeline',
  'export_bindings',
], desktop);
await import('./copy-platform-rules.mjs');

const ng = spawn(process.execPath, [
  join(desktop, 'node_modules/@angular/cli/bin/ng.js'),
  'serve',
  '--host',
  '127.0.0.1',
], { cwd: desktop, stdio: 'inherit' });
ng.on('error', (err) => {
  console.error(err);
  process.exit(1);
});
ng.on('exit', (code) => process.exit(code ?? 0));

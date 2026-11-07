// Tauri 번들에 넣을 도우미 실행 파일(legato-host, legato-viewer)을 릴리스로 빌드해서
// app/src-tauri/binaries/<이름>-<타깃 트리플>[.exe] 로 복사한다 (Tauri externalBin 규칙).
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const triple =
  process.env.TAURI_ENV_TARGET_TRIPLE ||
  execFileSync('rustc', ['-vV'], { encoding: 'utf8' }).match(/^host: (.+)$/m)[1];
const ext = triple.includes('windows') ? '.exe' : '';

execFileSync('cargo', ['build', '--release', '--target', triple, '-p', 'legato-host', '-p', 'legato-viewer'], {
  cwd: root,
  stdio: 'inherit',
});

const outDir = join(root, 'app', 'src-tauri', 'binaries');
mkdirSync(outDir, { recursive: true });
for (const name of ['legato-host', 'legato-viewer']) {
  const src = join(root, 'target', triple, 'release', name + ext);
  const dst = join(outDir, `${name}-${triple}${ext}`);
  copyFileSync(src, dst);
  console.log(`복사: ${dst}`);
}

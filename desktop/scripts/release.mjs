import { spawnSync } from 'node:child_process';
import { mkdir, copyFile, readdir, stat, rm } from 'node:fs/promises';
import path from 'node:path';
import { assertNodeRuntime } from '../../tools/kdocs/node-runtime.mjs';
import { checkModels, desktopDir } from './ocr-models.mjs';
function run(command, args) {
  const result = spawnSync(command, args, { cwd: desktopDir, stdio: 'inherit', windowsHide: true });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${path.basename(command)} 执行失败，退出码 ${result.status}`);
}
try {
  assertNodeRuntime();
  const args = process.argv.slice(2);
  if (args.includes('--lite')) throw new Error('已移除精简版，仅支持包含 PaddleOCR 模型的完整版');
  const checkOnly = args.includes('--check');
  const forwarded = args.filter(a => !['--check', '--no-portable'].includes(a));
  run('cargo', ['--version']);
  await checkModels();
  console.log('完整版：OCR 模型已通过大小和 SHA-256 检查');
  if (checkOnly) { console.log('打包前检查通过，未执行构建'); }
  else {
    if (forwarded.some(a => ['--debug','-d','--target','-t','--runner','-r'].includes(a))) throw new Error('此打包入口仅支持本机 release 构建；交叉构建请直接使用 Tauri CLI');
    const started = Date.now();

    const name = 'PdfBatchPrint';
    const overlay = { build: { beforeBuildCommand: 'pnpm build --outDir dist-build', frontendDist: '../dist-build' } };
    const cliArgs = ['node_modules/@tauri-apps/cli/tauri.js', 'build'];
    cliArgs.push('--config', JSON.stringify(overlay), ...forwarded);
    run(process.execPath, cliArgs);
    const releaseDir = path.join(desktopDir, 'src-tauri/target/release');
    if (!forwarded.includes('--no-bundle')) {
      const installerDir = path.join(releaseDir, 'installers', name);
      await rm(installerDir, { recursive: true, force: true });
      await mkdir(installerDir, { recursive: true });
      for (const format of ['nsis', 'msi']) {
        const directory = path.join(releaseDir, 'bundle', format);
        for (const file of await readdir(directory).catch(() => [])) {
          const source = path.join(directory, file);
          if (/\.(exe|msi)$/i.test(file) && (await stat(source)).mtimeMs >= started - 2000) {
            await copyFile(source, path.join(installerDir, file));
            console.log(`安装包：${path.join(installerDir, file)}`);
          }
        }
      }
    }

  }
} catch (error) { console.error(error.message); process.exitCode = 1; }

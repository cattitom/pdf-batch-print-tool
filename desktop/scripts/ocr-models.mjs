import { readFile, mkdir, writeFile, rename } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { assertNodeRuntime } from '../../tools/kdocs/node-runtime.mjs';
export const desktopDir = fileURLToPath(new URL('../', import.meta.url));
export const models = JSON.parse(await readFile(new URL('./ocr-models.json', import.meta.url), 'utf8'));
export function verifyModel(data, model) {
  return data.length === model.bytes && createHash('sha256').update(data).digest('hex') === model.sha256;
}
export async function checkModels(directory = path.join(desktopDir, 'models')) {
  for (const model of models) {
    let data;
    try { data = await readFile(path.join(directory, model.name)); } catch { throw new Error(`缺少 OCR 模型 ${model.name}，请执行 pnpm models:download`); }
    if (!verifyModel(data, model)) throw new Error(`OCR 模型损坏或版本不符：${model.name}，请移走该文件后执行 pnpm models:download`);
  }
}
export async function downloadModels() {
  assertNodeRuntime();
  const directory = path.join(desktopDir, 'models');
  await mkdir(directory, { recursive: true });
  for (const model of models) {
    const target = path.join(directory, model.name);
    try { if (verifyModel(await readFile(target), model)) { console.log(`${model.name} 已校验，跳过`); continue; } } catch { /* 下载缺失项 */ }
    const response = await fetch(model.url);
    if (!response.ok) throw new Error(`下载 ${model.name} 失败：HTTP ${response.status}`);
    const data = Buffer.from(await response.arrayBuffer());
    if (!verifyModel(data, model)) throw new Error(`${model.name} 校验失败，未替换本地文件`);
    const temporary = `${target}.${process.pid}.download`;
    await writeFile(temporary, data);
    await rename(temporary, target);
    console.log(`${model.name} 下载及校验完成`);
  }
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv.includes('--download')) await downloadModels();
    else { await checkModels(); console.log('OCR 模型检查通过'); }
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}

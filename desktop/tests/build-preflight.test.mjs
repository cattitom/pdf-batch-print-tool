import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rmdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { assertNodeRuntime } from '../../tools/kdocs/node-runtime.mjs';
import { checkModels, verifyModel, models } from '../scripts/ocr-models.mjs';

test('拒绝缺少运行条件的 Node，支持已验证的版本', () => {
  const capabilities = {fetch(){}, WebSocket(){}};
  for (const version of ['18.20.0','20.19.0','22.11.0']) assert.throws(()=>assertNodeRuntime(version,capabilities),/22.12/);
  for (const version of ['22.12.0','24.12.0']) assert.doesNotThrow(()=>assertNodeRuntime(version,capabilities));
  assert.throws(()=>assertNodeRuntime('24.12.0',{fetch(){}}),/WebSocket/);
});
test('缺失模型时给出下载指引，损坏内容不通过校验', async () => {
  const directory = await mkdtemp(path.join(tmpdir(),'ocr-preflight-'));
  try { await assert.rejects(checkModels(directory), /models:download/); }
  finally { await rmdir(directory); }
  assert.equal(verifyModel(Buffer.from('truncated'), models[0]), false);
});

test('精简版旧参数被明确拒绝，默认安装包始终包含模型', () => {
  const result = spawnSync(process.execPath, [fileURLToPath(new URL('../scripts/release.mjs', import.meta.url)), '--lite', '--check'], {encoding:'utf8',windowsHide:true});
  assert.equal(result.status,1);
  assert.match(result.stderr,/已移除精简版/);
  const config = JSON.parse(readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url),'utf8'));
  for (const model of models) assert.equal(config.bundle.resources['../models/'+model.name], 'models/'+model.name);
  const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url),'utf8'));
  assert.equal(pkg.scripts['release:lite'], undefined);
});

import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
export const files = ['app.html', 'shipping/index.html', 'shipping/m.css', 'shipping/m.js', 'shipping/transport.js'];
export async function buildUpdate() {
  const content = {};
  for (const name of files) content[name] = await readFile(new URL('../web/' + name, import.meta.url), 'utf8');
  const payload = JSON.stringify({ schema: 1, shellApi: 1, files: content });
  return { version: createHash('sha256').update(payload).digest('hex'), payload };
}

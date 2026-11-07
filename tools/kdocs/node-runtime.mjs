/** 桌面构建与独立云同步共用的 Node 运行条件。 */
export function assertNodeRuntime(version = process.versions.node, runtime = globalThis) {
  const [major, minor] = version.split('.').map(Number);
  if (!Number.isInteger(major) || major < 22 || (major === 22 && minor < 12)) {
    throw new Error(`需要 Node.js 22.12.0 或更高版本（建议 24.12.0），当前为 ${version}`);
  }
  if (typeof runtime.fetch !== 'function' || typeof runtime.WebSocket !== 'function') {
    throw new Error('当前 Node 缺少 fetch 或 WebSocket，请使用标准 Node.js 22.12.0+');
  }
}

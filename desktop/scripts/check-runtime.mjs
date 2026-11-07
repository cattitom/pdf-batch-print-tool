import { assertNodeRuntime } from '../../tools/kdocs/node-runtime.mjs';
try { assertNodeRuntime(); console.log(`Node.js ${process.versions.node} 运行条件检查通过`); }
catch (error) { console.error(error.message); process.exit(1); }

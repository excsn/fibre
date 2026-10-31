const fs = require('fs');
const { WASI } = require('node:wasi');

const wasi = new WASI({ version: 'preview1', args: [], env: {} });
const wasmModule = new WebAssembly.Module(fs.readFileSync(process.argv[2]));
const instance = new WebAssembly.Instance(wasmModule, wasi.getImportObject());
wasi.start(instance);

import { build } from 'esbuild';
import { mkdir, readFile, writeFile, readdir } from 'node:fs/promises';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
const base = dirname(fileURLToPath(import.meta.url));
const outdir = resolve(base, '../flutter/ad_remote_desktop/assets/editor');
await mkdir(outdir, {recursive:true});
const vs = resolve(base, 'node_modules/monaco-editor/esm/vs');
await build({absWorkingDir:base, entryPoints:{editor:'src/editor.js',
  'editor.worker':`${vs}/editor/editor.worker.js`, 'json.worker':`${vs}/language/json/json.worker.js`,
  'css.worker':`${vs}/language/css/css.worker.js`, 'html.worker':`${vs}/language/html/html.worker.js`,
  'ts.worker':`${vs}/language/typescript/ts.worker.js`},
  outdir,bundle:true,format:'iife',platform:'browser',target:'es2022',minify:true,
  loader:{'.ttf':'file'},assetNames:'[name]',legalComments:'linked',logLevel:'info'});
await writeFile(resolve(outdir,'index.html'),await readFile(resolve(base,'index.html')));
for(const [source,target] of [['LICENSE','MONACO-LICENSE.txt'],['ThirdPartyNotices.txt','MONACO-THIRD-PARTY-NOTICES.txt']]) {
  await writeFile(resolve(outdir,target),await readFile(resolve(base,'node_modules/monaco-editor',source)));
}
const entries=(await readdir(outdir)).filter(n=>n!=='manifest.json').sort();
await writeFile(resolve(outdir,'manifest.json'),JSON.stringify({version:1,files:entries},null,2)+'\n');
for(const required of ['editor.js','editor.css','editor.worker.js','ts.worker.js','index.html']) {
  if(!entries.includes(required))throw Error(`Editor output is missing ${required}`);
}
console.log(`Offline editor ready: ${outdir}`);

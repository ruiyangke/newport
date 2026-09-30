/** Frontend blob-page validation only; no RPC, text rendering, or repository writes. */
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { dirname } from 'node:path';
import { transform } from 'esbuild';
import { createHash } from 'node:crypto';
const baseline = process.argv.includes('--baseline');
const samples = [], sourceHashes = {};
for (const [variant,path] of [['before','/tmp/newport-before-byte-validation.ts'],['after',baseline?'/tmp/newport-before-byte-validation.ts':'src/domain/gitResponses.ts']]) {
  const source = await readFile(path,'utf8'); sourceHashes[variant] = createHash('sha256').update(source).digest('hex');
  const { code } = await transform(source,{loader:'ts',format:'esm'});
  const module = await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'));
  for (const size of [4096,65536,393000]) {
    const raw = Buffer.alloc(size); for(let i=0;i<size;i++)raw[i]=i%251;
    const page = { snapshot:'blob', nextCursor:null, metadata:{oid:{algorithm:'sha1',hex:'a'.repeat(40)},size},entries:[{offset:0,bytesB64:raw.toString('base64')}] };
    for(let round=0;round<3;round++) {
      const decode=globalThis.atob; let decodedBytes=0, decodeCalls=0;
      globalThis.atob=value=>{const bytes=decode(value);decodedBytes+=bytes.length;decodeCalls++;return bytes;};
      globalThis.gc?.();const before=process.memoryUsage();const start=performance.now();
      try {for(let n=0;n<100;n++){if(module.decodeGitBlobPage(page).entries[0].byteLength!==size)throw new Error('wrong byte length');}}
      finally{globalThis.atob=decode;}
      const elapsedMs=performance.now()-start, after=process.memoryUsage();
      samples.push({variant,size,round,pages:100,elapsedMs,decodeCalls,decodedBytes,heapDelta:after.heapUsed-before.heapUsed,rssAfter:after.rss});
    }
  }
}
const output = process.argv[2] ?? 'docs/benchmarks/git-byte-validation.json';
await mkdir(dirname(output), { recursive: true });
await writeFile(output,JSON.stringify({scope:'Validation of 100 independent blob pages, excluding network and rendering',memory:'After validation, not peak; GC affects heap delta',sourceHashes,samples},null,2)+'\n');
for(const row of samples.filter(s=>s.round===1))console.log(row.variant,row.size,row.elapsedMs.toFixed(2),row.decodedBytes);

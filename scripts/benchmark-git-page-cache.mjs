import { build } from 'esbuild';
import { writeFile, readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { performance } from 'node:perf_hooks';
const [output, mode='current'] = process.argv.slice(2);
const bundlePath='/tmp/newport-page-cache-baseline.mjs';
const compiled=await build({stdin:{contents:'export { createQueryClient } from "./src/query/client"; export { createUniquePageAppender } from "./src/git/uniquePages";',resolveDir:process.cwd(),loader:'ts'},bundle:true,write:false,format:'esm',platform:'node'});
const source=mode==='baseline'?await readFile(bundlePath,'utf8'):compiled.outputFiles[0].text;
if(mode==='capture')await writeFile(bundlePath,source);
const {createQueryClient,createUniquePageAppender}=await import('data:text/javascript;base64,'+Buffer.from(source).toString('base64'));
const report={scope:'Immutable page append plus actual QueryClient cache publication; excludes RPC and rendering',sourceSha256:createHash('sha256').update(source).digest('hex'),samples:[]};
for(const count of [1000,10000,100000])for(let round=0;round<3;round++){
 global.gc?.();const startHeap=process.memoryUsage().heapUsed;
 const client=createQueryClient(),key=['page-benchmark'];
 const rows=Array.from({length:count},(_,i)=>({id:`row-${i}`,path:{display:`file-${i}`,bytesB64:Buffer.from(`file-${i}`).toString('base64')}}));
 const append=createUniquePageAppender(row=>row.id);
 client.setQueryData(key,{snapshot:'s',metadata:{count},entries:rows.slice(0,100),nextCursor:'100'});
 const start=performance.now();
 for(let offset=100;offset<count;offset+=100){
  const page=client.getQueryData(key);
  const merged=append(page,{snapshot:'s',metadata:{count},entries:rows.slice(offset,offset+100),nextCursor:offset+100<count?String(offset+100):null},String(offset));
  append.adopt(merged,client.setQueryData(key,merged));
 }
 const elapsedMs=performance.now()-start,final=client.getQueryData(key);
 if(final.entries.length!==count||final.entries.some((row,i)=>row.id!==rows[i].id)||final.nextCursor!==null)throw Error('Incorrect cache rows');
 report.samples.push({count,round,elapsedMs,heapDeltaBytes:process.memoryUsage().heapUsed-startHeap,memoryScope:'heap after, not peak'});client.clear();
}
await writeFile(output,JSON.stringify(report,null,2)+'\n');
for(const row of report.samples.filter(row=>row.round===1))console.log(row.count,row.elapsedMs.toFixed(2));

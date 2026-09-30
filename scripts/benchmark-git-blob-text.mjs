/** Pure frontend decoding benchmark. No UI, SSH, or repository mutations. */
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {transform} from 'esbuild';
import {createHash} from 'node:crypto';
const variants=[['before','/tmp/newport-blobText-before.ts'],['after',process.argv.includes('--baseline')?'/tmp/newport-blobText-before.ts':'src/git/blobText.ts']];
const samples=[];const sourceHashes={};
for(const [variant,path] of variants){
 const source=await readFile(path,'utf8');sourceHashes[variant]=createHash('sha256').update(source).digest('hex');
 const {code}=await transform(source,{loader:'ts',format:'esm'});
 const module=await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'));
 for(const size of [65536,1048576,8388608]){
  const unit='content with unicode 界\r\n';
  const raw=Buffer.from(unit.repeat(Math.ceil(size/Buffer.byteLength(unit)))).subarray(0,size);
  const expected=raw.toString('utf8');const entries=[];let offset=0;
  while(offset<raw.length){const end=Math.min(raw.length,offset+(offset?384*1024:48*1024));entries.push({offset,bytesB64:raw.subarray(offset,end).toString('base64'),byteLength:end-offset});offset=end;}
  for(let round=0;round<3;round++){
   const reader=module.createBlobTextReader?.() ?? module.blobText;
   const decode=globalThis.atob;let decodedBytes=0;globalThis.atob=s=>{const raw=decode(s);decodedBytes+=raw.length;return raw;};
   globalThis.gc?.();const memoryBefore=process.memoryUsage();const times=[];let text='';
   try {
    for(let n=1;n<=entries.length;n++){
     const page={snapshot:'immutable',nextCursor:n===entries.length?null:String(n),metadata:{oid:{algorithm:'sha1',hex:'a'.repeat(40)},size:raw.length},entries:entries.slice(0,n)};
     const start=performance.now();text=reader(page);times.push(performance.now()-start);
    }
   }finally{globalThis.atob=decode;}
   if(text!==expected)throw new Error('content mismatch');
   const memoryAfter=process.memoryUsage();samples.push({variant,size:raw.length,round,pages:entries.length,decodedBytes,firstMs:times[0],totalMs:times.reduce((a,b)=>a+b,0),heapDelta:memoryAfter.heapUsed-memoryBefore.heapUsed,rssAfter:memoryAfter.rss});
  }
 }
}
const output=process.argv[2]??'docs/benchmarks/git-blob-text.json';await mkdir('docs/benchmarks',{recursive:true});await writeFile(output,JSON.stringify({scope:'Pure frontend decode, no RPC or editor rendering',memory:'After final page, not peak; heap delta may include GC',sourceHashes,samples},null,2)+'\n');
for(const s of samples.filter(s=>s.round===1))console.log(s.variant,s.size,'pages',s.pages,'decodedBytes',s.decodedBytes,'total_ms',s.totalMs.toFixed(2));

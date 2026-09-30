import { build } from "esbuild";
import { readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { performance } from "node:perf_hooks";
const baseline = process.argv.includes("--baseline");
const output = process.argv[2];
const report = {measurement:"local immutable page merge and deduplication, excluding RPC/rendering",memory:"heap after, not peak",samples:[]};
for (const [variant,path] of [["before","/tmp/newport-unique-pages-before.ts"],["after",baseline?"/tmp/newport-unique-pages-before.ts":"src/git/uniquePages.ts"]]) {
 const source=await readFile(path,"utf8");
 const bundled=await build({stdin:{contents:source,resolveDir:process.cwd()+"/src/git",loader:"ts"},bundle:true,write:false,format:"esm",platform:"node"});
 const {createUniquePageAppender}=await import("data:text/javascript;base64,"+Buffer.from(bundled.outputFiles[0].text).toString("base64"));
 for (const count of [1000,10000,100000]) {
  const rows=Array.from({length:count},(_,i)=>({id:`entry-${i}`,name:`file-${i}`}));
  for(let round=0;round<3;round++) {
   global.gc?.();const heap=process.memoryUsage().heapUsed;let keys=0;
   const append=createUniquePageAppender(entry=>{keys++;return entry.id;});
   let page={snapshot:"s",metadata:{total:count},entries:rows.slice(0,200),nextCursor:"200"};
   const start=performance.now();
   for(let offset=200;offset<count;offset+=200) page=append(page,{...page,entries:rows.slice(offset,offset+200),nextCursor:offset+200<count?String(offset+200):null},String(offset));
   const elapsedMs=performance.now()-start;
   if(page.entries.length!==count||page.entries.some((entry,i)=>entry!==rows[i]))throw Error("Incorrect rows");
   report.samples.push({variant,count,round,elapsedMs,entryKeyCalls:keys,heapDelta:process.memoryUsage().heapUsed-heap,sourceSha256:createHash("sha256").update(source).digest("hex")});
  }
 }
}
await writeFile(output,JSON.stringify(report,null,2)+"\n");
for(const s of report.samples.filter(s=>s.round===1))console.log(s.variant,s.count,s.elapsedMs.toFixed(2),s.entryKeyCalls);

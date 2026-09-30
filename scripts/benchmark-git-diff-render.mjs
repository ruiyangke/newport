/** Pure frontend diff assembly. Excludes network, validation, and editor rendering. */
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { transform } from 'esbuild';
import { createHash } from 'node:crypto';
const baseline = process.argv.includes('--baseline');
const variants = [['before', '/tmp/newport-before-diff-render.ts'], ['after', baseline ? '/tmp/newport-before-diff-render.ts' : 'src/git/historicalDiff.ts']];
const samples = [], sourceHashes = {};
for (const [variant, path] of variants) {
  const source = await readFile(path, 'utf8');
  sourceHashes[variant] = createHash('sha256').update(source).digest('hex');
  const { code } = await transform(source, { loader: 'ts', format: 'esm' });
  const module = await import('data:text/javascript;base64,' + Buffer.from(code).toString('base64'));
  for (const count of [20, 5000, 100000]) {
    const pieces = Array.from({ length: count }, (_, index) => ({
      lineIndex: index, byteOffset: 0, lineComplete: true, origin: '+', oldLine: null, newLine: index + 1,
      contentBytesB64: Buffer.from(`line ${index} with unicode 界\n`).toString('base64'), id: 'a'.repeat(64),
    }));
    for (let round = 0; round < 3; round++) {
      const render = module.createDiffRenderer?.() ?? module.workingDiff;
      const decode = globalThis.atob; let decodedBytes = 0, decodedPieces = 0;
      globalThis.atob = value => { const result = decode(value); decodedBytes += result.length; decodedPieces++; return result; };
      globalThis.gc?.(); const before = process.memoryUsage(), times = []; let result;
      try {
        for (let end = Math.min(200, count); ; end = Math.min(end + 4000, count)) {
          const page = { snapshot: 'diff', nextCursor: end < count ? String(end) : null,
            metadata: { sourceSnapshot: 'status', totalFiles: 1 }, entries: [{ fileIndex: 0, hunks: [{
              index: 0, id: 'b'.repeat(64), totalLines: count, lines: pieces.slice(0, end),
            }] }] };
          const start = performance.now(); result = render(page); times.push(performance.now() - start);
          if (end === count) break;
          if (result.files[0].hunks[0].id !== null) throw new Error('premature hunk mutation ID');
        }
      } finally { globalThis.atob = decode; }
      const hunk = result.files[0].hunks[0];
      if (hunk.lines.length !== count || hunk.id !== 'b'.repeat(64)) throw new Error('lost lines or IDs');
      hunk.lines.forEach((line, i) => { if (line.content.display !== `line ${i} with unicode 界\n`) throw new Error('changed text'); });
      const after = process.memoryUsage();
      samples.push({ variant, count, round, pages: times.length, decodedBytes, decodedPieces, firstMs: times[0], totalMs: times.reduce((a,b) => a+b, 0), heapDelta: after.heapUsed-before.heapUsed, rssAfter: after.rss });
    }
  }
}
const output = process.argv[2] ?? 'docs/benchmarks/git-diff-render.json';
await mkdir('docs/benchmarks', { recursive: true });
await writeFile(output, JSON.stringify({ scope: 'Frontend assembly only; no RPC, response validation, or DOM', memory: 'After final page, not peak; heap delta includes GC', sourceHashes, samples }, null, 2)+'\n');
for (const row of samples.filter(s => s.round === 1)) console.log(row.variant, row.count, row.pages, row.decodedPieces, row.totalMs.toFixed(2));

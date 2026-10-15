// Run from the repository root. Executes the installed original xterm sources.
const fs = require('fs');
const vm = require('vm');
const ts = require('typescript');
function load(path, modules = {}) {
  const exports = {};
  const code = ts.transpileModule(fs.readFileSync(path, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  vm.runInNewContext(code, { exports, require: name => {
    if (!(name in modules)) throw new Error(`Unexpected oracle import ${name}`);
    return modules[name];
  } });
  return exports;
}
const color = load(require.resolve('@xterm/xterm/src/common/Color.ts'));
const types = load(require.resolve('@xterm/xterm/src/browser/Types.ts'), {'common/Color': color});
const palette = types.DEFAULT_ANSI_COLORS.map(c => c.rgba >>> 8);
const cases = [];
for (const bg of [0xffffff, 0x1e1f21, 0x777777, 0x000080]) {
  for (const fg of [...palette, 0xeeeeef, 0x25262a, bg]) {
    for (const ratio of [4.5, 2.25]) {
      const result = color.rgba.ensureContrastRatio(((bg << 8) | 255) >>> 0, ((fg << 8) | 255) >>> 0, ratio);
      cases.push({bg, fg, ratio, result: result === undefined ? null : result >>> 8});
    }
  }
}
fs.writeFileSync('crates/newport-gpui/tests/fixtures/terminal-colors.json', JSON.stringify({palette,cases})+'\n');

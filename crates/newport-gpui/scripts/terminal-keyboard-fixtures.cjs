// Compare native keydown dispatch with the original app's installed xterm.
// Run from the repository root; uses existing TypeScript, no network/browser.
// Pass --windows to generate the Windows reference instead of macOS.
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
assert.ok(process.argv.length === 2 || (process.argv.length === 3 && process.argv[2] === '--windows'));
const isWindows = process.argv[2] === '--windows';
const ts = require('typescript');
assert.equal(require('@xterm/xterm/package.json').version, '6.0.0');
const root = path.join(path.dirname(require.resolve('@xterm/xterm/package.json')), 'src');
const cache = new Map();
function load(name) {
  assert.ok(name.startsWith('common/') && !name.includes('..'));
  if (cache.has(name)) return cache.get(name);
  const filename = path.join(root, `${name}.ts`);
  const { outputText } = ts.transpileModule(fs.readFileSync(filename, 'utf8'), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
    fileName: filename,
  });
  const module = { exports: {} };
  cache.set(name, module.exports);
  vm.runInNewContext(outputText, { module, exports: module.exports, require: load }, { filename });
  return module.exports;
}
const { evaluateKeyboardEvent } = load('common/input/Keyboard');
const { KeyboardResultType } = load('common/Types');
// Execute the original keydown methods as well as the escape-sequence evaluator.
// Inert host stubs record output/scrolling without needing a DOM or real PTY.
const sourceText = fs.readFileSync(path.join(root, 'browser/CoreBrowserTerminal.ts'), 'utf8');
const source = ts.createSourceFile('CoreBrowserTerminal.ts', sourceText, ts.ScriptTarget.Latest, true);
const browserClass = source.statements.find(n => ts.isClassDeclaration(n) && n.name.text === 'CoreBrowserTerminal');
const methodNames = ['_keyDown', '_isThirdLevelShift'];
const methods = methodNames.map(name => {
  const method = browserClass.members.find(n => n.name?.getText(source) === name);
  assert.ok(method, name);
  return method.getText(source);
});
const wrapper = ts.transpileModule('class Reference { ' + methods.join('\n') + ' }\nmodule.exports = Reference;', {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText;
const referenceModule = { exports: {} };
vm.runInNewContext(wrapper, { module: referenceModule, evaluateKeyboardEvent, KeyboardResultType,
  C0: load('common/data/EscapeSequences').C0 });
function dispatch(event, applicationCursor) {
  let output = null, scroll = null;
  const reference = Object.assign(new referenceModule.exports(), {
    browser: { isMac: !isWindows, isWindows },
    options: { macOptionIsMeta: false, scrollOnUserInput: true },
    optionsService: { rawOptions: { screenReaderMode: false } },
    buffer: { ybase: 0, ydisp: 0 }, rows: 24, textarea: { value: '' },
    _compositionHelper: { keydown: () => true },
    coreService: { decPrivateModes: { applicationCursorKeys: applicationCursor },
      triggerDataEvent: text => { output = text; } },
    _onKey: { fire: () => {} }, _showCursor: () => {}, updateCursorStyle: () => {},
    cancel: () => false, scrollLines: lines => { scroll = -lines; },
  });
  reference._keyDown(event);
  return { output, scroll };
}
const keys = [
  ['backspace', 8], ['tab', 9], ['enter', 13], ['escape', 27],
  ['pageup', 33], ['pagedown', 34], ['end', 35], ['home', 36],
  ['left', 37], ['up', 38], ['right', 39], ['down', 40],
  ['insert', 45], ['delete', 46],
  ...Array.from({ length: 20 }, (_, i) => ['f' + (i + 1), 112 + i]),
];
const fixtures = [];
function add(key, keyCode, modifiers, applicationCursor, nativeKey = key, nativeModifiers = modifiers) {
  const event = { key, keyCode, code: '', type: 'keydown', shiftKey: !!(modifiers & 1),
    altKey: !!(modifiers & 2), ctrlKey: !!(modifiers & 4), metaKey: false,
    getModifierState: () => false };
  const result = dispatch(event, applicationCursor);
  fixtures.push({ key: nativeKey, modifiers: nativeModifiers, applicationCursor, ...result });
}
for (const applicationCursor of [false, true]) {
  for (const [key, keyCode] of keys) {
    for (let modifiers = 0; modifiers < 8; modifiers++) add(key, keyCode, modifiers, applicationCursor);
  }
  for (let code = 65; code <= 90; code++) {
    for (let modifiers = isWindows ? 2 : 4; modifiers < 8; modifiers++) {
      const lower = String.fromCharCode(code + 32);
      add(modifiers & 1 ? lower.toUpperCase() : lower, code, modifiers, applicationCursor, lower);
    }
  }
  // GPUI keeps Shift for letters, but folds shifted punctuation into key
  // and clears the modifier. These fixtures model that boundary on a US layout.
  for (const [normal, shifted, code] of [
    ['0', ')', 48], ['1', '!', 49], ['2', '@', 50], ['3', '#', 51], ['4', '$', 52],
    ['5', '%', 53], ['6', '^', 54], ['7', '&', 55], ['8', '*', 56], ['9', '(', 57],
    [';', ':', 186], ['=', '+', 187], [',', '<', 188], ['-', '_', 189], ['.', '>', 190],
    ['/', '?', 191], ['`', '~', 192], ['[', '{', 219], ['\\', '|', 220], [']', '}', 221], ["'", '"', 222],
  ]) {
    for (const modifiers of isWindows ? [2, 4, 6] : [4]) {
      add(normal, code, modifiers, applicationCursor);
      add(shifted, code, modifiers | 1, applicationCursor, shifted, modifiers);
    }
  }
  for (let modifiers = isWindows ? 2 : 4; modifiers < 8; modifiers++) add(' ', 32, modifiers, applicationCursor, 'space');
}
const filename = isWindows ? 'terminal-keyboard-windows.json' : 'terminal-keyboard.json';
fs.writeFileSync(`crates/newport-gpui/tests/fixtures/${filename}`, JSON.stringify(fixtures) + '\n');
console.log(fixtures.length + ' source keydown / modifier / cursor-mode comparisons');

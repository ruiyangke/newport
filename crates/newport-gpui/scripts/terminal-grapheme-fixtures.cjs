// Regenerate the grid oracle using the original Newport terminal dependencies.
// Run from the repository root: node crates/newport-gpui/scripts/terminal-grapheme-fixtures.cjs
const fs = require("fs");
const { createRequire } = require("module");
const req = createRequire(__filename);
const { Terminal } = req("@xterm/xterm");
// Execute the browser addon without Node's pooled Buffer; its trie decoder
// reads ArrayBuffer directly, so a Buffer byteOffset corrupts the fixture.
const vm = require("vm");
const mod = { exports: {} };
vm.runInNewContext(
  fs.readFileSync(req.resolve("@xterm/addon-unicode-graphemes"), "utf8"),
  { module: mod, exports: mod.exports, atob },
);
const { UnicodeGraphemesAddon } = mod.exports;
const cases = [
  ["unicode-provider-wifi", "a\u{1f6dc}!"],
  ["unicode-provider-harp", "a\u{1fa89}!"],
  ["unicode-provider-tired-face", "a\u{1fae9}!"],
  ["unicode-provider-new-mark", "a\u0897!"],
  ["unicode-provider-nag-mundari", "a\u{1e4ec}!"],
  ["unicode-provider-indic", "क्‍ष!"],
  ["unicode-provider-zwj", "a‍💻!"],
  ["unicode-provider-isolated-selector", "\u{fe0f}!"],
  ["unicode-provider-mark-after-control", "a\x1b[31m\u0897!"],
  ["unicode-provider-right-margin", "123456789\u{1fa89}!"],
  ["unicode-provider-wide-right-margin", "123456789\u{fe0f}!"],
  ["unicode-provider-repeated-new-character", "\u{1fae9}\x1b[3b!"],
  ["unicode-provider-promoted-new-character", "\u{1fae9}\u{fe0f}!"],
  ["unicode-provider-resized-new-character", "123456789\u{1fae9}", [6, 3], "!"],
  ["resize-prompt-grow", "123456789abcdef", [14, 3], "!"],
  ["resize-prompt-shrink", "123456789abcdef", [6, 3], "!"],
  ["resize-history-grow", "123456789abcdef\r\nprompt", [14, 3], "!"],
  ["resize-history-shrink", "123456789abcdef\r\nprompt", [6, 3], "!"],
  ["resize-middle-grow", "123456789abcdefghijklmnop\x1b[2;2H", [14, 3], "!"],
  ["resize-middle-shrink", "123456789abcdefghijklmnop\x1b[2;2H", [6, 3], "!"],
  ["resize-wide-clipped", "12345中x", [6, 3], "!"],
  ["resize-margin-shrink", "1234567890", [6, 3], "!"],
  ["resize-scrolled-grow", "long history entry\r\nmore history here\r\n123456789abcdef", [14, 3], "!"],
  ["resize-scrolled-shrink", "long history entry\r\nmore history here\r\n123456789abcdef", [6, 3], "!"],
  ["resize-bottom-output-grow", "prompt\r\n123456789abcdef\x1b[1;2H", [14, 3], "!"],
  ["resize-bottom-output-shrink", "prompt\r\n123456789abcdef\x1b[1;2H", [6, 3], "!"],
  ["resize-short-output-below", "prompt\r\n123456789\x1b[1;2H", [6, 3], "!"],
  ["resize-short-output-saved", "prompt\r\n123456789\x1b[1;2H\x1b7", [6, 3], "\x1b8!"],
  ["resize-short-output-alt", "prompt\r\n123456789\x1b[1;2H\x1b[?1049hALT", [6, 3], "\x1b[?1049l!"],
  ["resize-short-output-extra-space", "prompt\r\n123456789\x1b[1;2H", [10, 5], "!", [6, 5]],
  ["resize-history-output-below", "history\r\nprompt\r\n123456789abcdef\x1b[1;2H", [6, 3], "!"],
  ["resize-grow-height-top", "a\r\nb\r\nc\x1b[1;2H", [10, 5], "!"],
  ["resize-grow-height-history-top", "history\r\na\r\nb\r\nc\x1b[1;2H", [10, 5], "!"],
  ["resize-grow-height-history-bottom", "history\r\na\r\nb\r\nc", [10, 5], "!"],
  ["resize-grow-height-history-middle", "history\r\na\r\nb\r\nc\x1b[2;2H", [10, 5], "!"],
  ["resize-grow-height-alternate", "history\r\na\r\nb\r\nc\x1b[1;2H\x1b[?1049hALT", [10, 5], "\x1b[?1049l!"],
  ["resize-grow-height-saved-cursor", "history\r\na\r\nb\r\nc\x1b[1;2H\x1b7", [10, 5], "\x1b8!"],
  ["resize-shrink-height-top", "a\r\nb\r\nc\x1b[1;2H", [10, 2], "!"],
  ["resize-shrink-height-middle", "a\r\nb\r\nc\x1b[2;2H", [10, 2], "!"],
  ["resize-shrink-both-top", "prompt\r\n123456789abcdef\x1b[1;2H", [6, 2], "!"],
  ["resize-grow-both-history", "history\r\n123456789abcdef\r\nprompt", [14, 5], "!"],
  ["resize-grow-shrink-prompt", "123456789abcdef", [14, 3], "!", [6, 3]],
  ["resize-shrink-grow-prompt", "123456789abcdef", [6, 3], "!", [14, 3]],
  ["resize-grow-shrink-history", "123456789abcdef\r\nprompt", [14, 3], "!", [6, 3]],
  ["resize-same", "❤", [10, 3], "️!"],
  ["resize-grow", "a👩‍", [14, 3], "💻!"],
  ["resize-shrink", "a❤", [6, 3], "️!"],
  ["resize-rows", "a\r\nb\r\nc❤", [10, 2], "️!"],
  ["resize-wrap-grow", "12345678👩‍", [14, 3], "💻!"],
  ["resize-reflow-grow", "123456789ab👩‍", [14, 3], "💻!"],
  ["resize-reflow-shrink", "12345678❤", [6, 3], "️!"],
  ["resize-alternate", "\x1b[?1049ha❤", [14, 3], "️!"],
  ["resize-repeat", "👩‍💻", [14, 3], "\x1b[2b!"],
  ["repeat-zwj", "👩‍💻\u001b[2b!"],
  ["repeat-flag", "🇺🇸\u001b[2b!"],
  ["repeat-regional", "🇺\u001b[2b!"],
  ["repeat-combining", "é\u001b[3b!"],
  ["repeat-default", "❤️\u001b[b!"],
  ["repeat-zero", "❤️\u001b[0b!"],
  ["repeat-wrap", "👨‍👩‍👧‍👦\u001b[15b!"],
  ["repeat-after-control", "❤️\u001b[31m\u001b[2b!"],
  ["repeat-after-repeat", "🇺🇸\u001b[2b\u001b[2b!"],
  ["repeat-before-text", "\u001b[2b!"],
  ["del-join", "👩‍💻!"],
  ["del-repeat", "❤️\u001b[2b!"],
  ["unknown-osc", "👩‍\u001b]999;ignored\u0007💻!"],
  ["unknown-dcs", "👩‍\u001bPignored\u001b\\💻!"],
  ["cancel-control", "👩‍\u0018💻!"],
  ["sync-ignored-control", "\u001b[?2026h👩‍\u001b[999z💻!\u001b[?2026l"],
  ["sync-null", "\u001b[?2026ha\u0000́!\u001b[?2026l"],
  ["sync-repeat", "\u001b[?2026h👩‍💻\u001b[2b!\u001b[?2026l"],
  ["sync-title", "\u001b[?2026h👩‍\u001b]2;hello\u0007💻!\u001b[?2026l"],
  ["sync-del", "\u001b[?2026h👩‍💻!\u001b[?2026l"],
  ["unknown-control", "👩‍\x1b[999z💻!"],
  ["null-boundary", "a\0\u0301!"],
  ["escape-title", "a\x1b]2;title\x07\u0301!"],
  ["ascii", "hello"],
  ["combining", "e\u0301a\u0308!"],
  ["zwj", "👩‍💻!"],
  ["family", "👨‍👩‍👧‍👦!"],
  ["modifier", "👍🏽!"],
  ["flag", "🇺🇸!"],
  ["flags", "🇺🇸🇨🇦🇬!"],
  ["keycap", "1️⃣!"],
  ["variation", "❤️!"],
  ["text-selector", "😀︎!"],
  ["hangul", "각!"],
  ["isolated", "\u0301!"],
  ["spacing-mark", "का!"],
  ["prepend", "\u0600a!"],
  ["cjk", "中文!"],
  ["promote-right-margin", "123456789❤️!"],
  ["wide-right-margin", "123456789👩‍💻!"],
  ["join-at-margin", "12345678👩‍💻!"],
  ["scroll-promotion", "a\r\nb\r\n123456789❤️!"],
  ["control-boundary", "a\x1b[31m\u0301!"],
  ["cursor-rewrite", "👩‍💻!\rOK"],
  ["insert", "abcdef\r\x1b[4h❤️!"],
  ["no-wrap", "\x1b[?7l123456789❤️!"],
  ["no-wrap-wide", "\x1b[?7l123456789👩‍💻!"],
  ["alternate", "abc\x1b[?1049h👩‍💻!\x1b[?1049l"],
  ["line-drawing", "\x1b(0lqk\x1b(B👩‍💻!"],
  ["regional-margin", "123456789🇺🇸!"],
  ["combining-cr", "abc\r\u0301!"],
  ["variation-cursor", "❤️!\x1b[6n"],
];
(async () => {
  const fixtures = [];
  for (const [name, input, resize, after, resizeAgain] of cases) {
    const t = new Terminal({ cols: 10, rows: 3, allowProposedApi: true });
    t.loadAddon(new UnicodeGraphemesAddon());
    await new Promise((r) => t.write(input, r));
    if (resize) {
      t.resize(...resize);
      if (resizeAgain) t.resize(...resizeAgain);
      await new Promise((r) => t.write(after, r));
    }
    const [cols, rows] = resizeAgain || resize || [10, 3];
    const b = t.buffer.active;
    fixtures.push({
      name,
      input,
      ...(resize ? { resize, after } : {}),
      ...(resizeAgain ? { resizeAgain } : {}),
      cursor: [b.cursorY, b.cursorX],
      lines: Array.from({ length: rows }, (_, i) =>
        b.getLine(b.baseY + i).translateToString(true),
      ),
      cells: Array.from({ length: rows }, (_, i) =>
        Array.from({ length: cols }, (_, j) => {
          const c = b.getLine(b.baseY + i).getCell(j);
          return [c.getChars(), c.getWidth()];
        }),
      ),
    });
    t.dispose();
  }
  fs.mkdirSync("crates/newport-gpui/tests/fixtures", { recursive: true });
  fs.writeFileSync(
    "crates/newport-gpui/tests/fixtures/terminal-graphemes.json",
    "[\n" + fixtures.map((f) => "  " + JSON.stringify(f)).join(",\n") + "\n]\n",
  );
})();

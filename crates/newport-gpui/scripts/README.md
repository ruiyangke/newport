# Optional native fixture and icon generators

The native build consumes committed SVG assets and terminal JSON fixtures.
It does not install npm dependencies. These optional scripts use pinned npm
packages solely to reproduce that data. React renders the
file-icon SVGs offline; xterm supplies terminal compatibility fixtures. Neither
is bundled with Newport.

Run from the repository root:

```sh
npm ci --prefix crates/newport-gpui/scripts
node crates/newport-gpui/scripts/file-icons.mjs
node crates/newport-gpui/scripts/terminal-color-fixtures.cjs
node crates/newport-gpui/scripts/terminal-keyboard-fixtures.cjs
node crates/newport-gpui/scripts/terminal-keyboard-fixtures.cjs --windows
node crates/newport-gpui/scripts/terminal-grapheme-fixtures.cjs
```

Review generated changes before accepting them. The website maintains its own
independent npm package; root npm commands are native build/release wrappers.

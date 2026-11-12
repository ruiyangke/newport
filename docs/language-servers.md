---
title: "Language tools"
description: "Get completion, diagnostics, formatting and refactoring in the project editor from language servers on your server."
slug: language-tools
---

# Language tools

The [project editor](projects.md) offers completion, hover information, go to definition, error and warning marks, formatting and refactoring through a language server: a program that understands your project's language. It runs on your Linux server, in the project's folder, with your login shell's environment and the project's direnv settings.

## Before you start

Install the language server on the server, for the account Newport signs in with. Installing it on your own computer doesn't help.

- Rust: `rust-analyzer`.
- JavaScript, TypeScript, JSX and TSX: `typescript-language-server`.
- Python: `basedpyright`, or `pyright` if basedpyright isn't installed. **Install in terminal** offers `npm install -g pyright`.
- Go: `gopls`.

## Start a language server

Open a file in one of these languages. Newport starts the server, and the file header shows its name with a dot: green when ready, and a word such as **Starting** or **Indexing** while it works.

If the server isn't installed, a notice says so. Choose **Install in terminal**: a terminal tab opens in the editor with the install command typed in but not run, such as `rustup component add rust-analyzer`. Check it, press **Return**, and when it finishes choose **Try again**.

### If the project uses direnv

direnv loads settings for a project from its `.envrc` file. That file can run commands, so allow it only if you trust it.

If the notice says direnv hasn't allowed `.envrc`:

1. Choose **Allow in terminal**. A terminal tab opens with `direnv allow` typed in for this folder.
2. Check the command and press **Return**.
3. Choose **Try again**.

Choose **Start without .envrc** to start the server without those settings. It may then not find your project's dependencies. If direnv isn't installed on the server, this is the only choice.

When `.envrc` changes, the notice offers **Restart** to load the new settings.

## Use the tools

Completion, hover information and error marks appear as you type. Hold **Cmd** (**Ctrl** on Windows) and click a symbol to go to its definition.

Choose **Code** in the file header for:

- **Code actions…**: fixes and refactorings at the cursor or selection.
- **Organize imports**.
- **Find references**: a list of places you can open.
- **Rename symbol…**: rename it across the project.
- **Format document**.
- **Signature help**: the signature of the function at the cursor.
- **Restart language server**.

Items the server doesn't support are unavailable.

Formatting, renaming and code actions change your open tabs but don't save them. A rename can open several tabs; save each one. Undo works in each file.

## Use your editor settings

Newport reads language server settings from the project folder's `.vscode/settings.json` and `.zed/settings.json`. Comments and trailing commas are allowed. For example, `.vscode/settings.json`:

```jsonc
{
  "rust-analyzer.check.command": "clippy",
  "python.analysis.typeCheckingMode": "strict",
  "gopls": { "gofumpt": true },
  "typescript.format.semicolons": "remove",
}
```

From `.vscode/settings.json`, Newport reads `rust-analyzer.*`, the `gopls` object, `python.*`, `pyright.*`, `basedpyright.*`, `typescript.format.*` and `javascript.format.*`. From `.zed/settings.json`, it reads `lsp.<server>.settings` and `lsp.<server>.initialization_options`, where `<server>` is a name from the list above:

```jsonc
{
  "lsp": {
    "rust-analyzer": {
      "initialization_options": { "check": { "command": "clippy" } }
    }
  }
}
```

When both files set something, the Zed setting wins. The server's own files, such as `pyproject.toml` or `tsconfig.json`, also apply and may take precedence. Newport doesn't read other editor settings, extensions or workspace files.

After you change settings, choose **Code** > **Restart language server**. If a settings file can't be used, Newport names the problem and starts the server with its default settings. Each file must be inside the project folder and at most 256 KiB.

## When a tool cannot finish

| Symptom | Fix |
| --- | --- |
| The header says **Not installed** | Choose **Install in terminal**, or install the server yourself, then **Try again**. |
| The header says **Not started** | Follow the notice: allow `.envrc`, or fix it if it failed to load, then choose **Try again**. Or choose **Start without .envrc**. |
| The header says **Stopped** | Read the reason in the notice and choose **Restart**. |
| **Code** is unavailable | Wait until the server is ready. |
| "The language server took too long" | Wait for indexing to finish, then try again. |
| "The files changed" and no edits were applied | Run the tool again. |
| A definition "is outside the worktree" | Newport opens only files inside the project folder. |
| "File watching failed" | Choose **Code** > **Restart language server**. |
| Settings are ignored | Check that the file is at the top of the project folder and uses the settings listed above. |

Language tools can't create, rename or delete files, and **Find references** shows at most 500 places.

[Back to the user guide](README.md)

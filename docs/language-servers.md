---
title: "Language tools"
description: "Enable completion, diagnostics, formatting, and refactoring in the remote project editor."
slug: language-tools
---

# Language tools

The [project editor](projects.md) can offer completion, hover information, definitions, diagnostics, and code changes through a language server: a program that understands your project's language. That program runs on the Linux server alongside your project.

## Enable language tools

1. Open a project and a supported source file.
2. Check the file header for language-server status. Newport starts the server automatically when it is available.
3. If the server is missing, follow the installation action or guidance shown in the header.
4. After installing or changing settings, choose **Code → Restart language server**.

| Language | Command Newport looks for on the remote server |
| --- | --- |
| Rust | `rust-analyzer` |
| JavaScript, JSX, TypeScript, TSX | `typescript-language-server --stdio` |
| Python | `basedpyright-langserver --stdio`, or `pyright-langserver --stdio` if basedpyright is unavailable |
| Go | `gopls` |

Newport uses the project's login-shell environment and any approved direnv environment. Installing a language server only on your own computer will not make it available to the remote editor.

### If the project uses direnv

direnv loads project-specific environment settings from an `.envrc` file on the server. That file can execute shell commands, so inspect it and approve it only if you trust it.

If the language-server notice says `.envrc` has not been allowed, choose **Allow in terminal**. Newport opens a remote terminal with a command to change to this worktree and run `direnv allow`. Review and run the command, return to the editor, and choose **Try again**. This uses your remote account; it does not normally require administrator privileges.

Choose **Start without .envrc** if you want to try the language tools without loading that environment. They may then be unable to find project-specific dependencies. If direnv is missing, install it on the server using your normal package-management process, or use that same skip action.

## Use the Code menu

Completion, hover information, definitions, and diagnostics are available as you edit. The **Code** menu shows the additional tools supported by the active language server.

| Action | What happens |
| --- | --- |
| Code actions | Offers fixes and refactorings for the caret or selection |
| Organize imports | Offers the server's import-organizing actions |
| Find references | Lists locations you can open |
| Rename symbol | Prompts for a new name |
| Format document | Formats the open file |
| Signature help | Shows the function signature at the caret |

Choose these actions from the Code menu. Signature help opens when requested; it does not appear automatically as you type. You can cancel a tool from its result panel.

**Review and save the result.** Formatting, rename, and code actions change editor buffers without saving them. A rename may open several tabs; save each affected file. Each file has its own undo history. Completion edits, including added imports, can be undone together within that file.

## Reuse project settings

Newport reads these files from the **remote worktree root**—the folder containing the project's checked-out files:

| Configuration | Settings Newport reads |
| --- | --- |
| `.vscode/settings.json` | `rust-analyzer.*`; the `gopls` object; `python.*`, `pyright.*`, and `basedpyright.*`; `typescript.format.*` and `javascript.format.*` |
| `.zed/settings.json` | The supported server's `lsp.<server>.settings` and `lsp.<server>.initialization_options` objects |
| Language-specific project files | The language server discovers its usual files, such as `pyrightconfig.json`, `pyproject.toml`, `tsconfig.json`, `jsconfig.json`, and Cargo or Go workspace files |

Both editor settings files allow comments and trailing commas. For example, `.vscode/settings.json` can contain:

```jsonc
{
  "rust-analyzer.check.command": "clippy",
  "python.analysis.typeCheckingMode": "strict",
  "gopls": { "gofumpt": true },
  "typescript.format.semicolons": "remove",
}
```

For `.zed/settings.json`, use the server name from the table above without its command-line arguments. Python uses the name of the installed server, `basedpyright` or `pyright`:

```jsonc
{
  "lsp": {
    "rust-analyzer": {
      "initialization_options": {
        "check": { "command": "clippy" }
      }
    }
  }
}
```

If both editor files exist, Newport starts with the VS Code settings and applies matching Zed settings over them. For Rust and Go, Zed initialization options override the VS Code values, then Zed settings take precedence. When basedpyright is installed, explicit `basedpyright.*` settings override compatible Python and Pyright defaults. A language server's own project files may take precedence over editor settings.

Newport imports only the language-server settings listed above. It does not import editor extensions, appearance, tasks, user-wide settings, `.code-workspace` files, language override blocks such as `[python]`, or custom server commands. The older `.newport/lsp.json` format is no longer read.

After editing settings, choose **Code → Restart language server**. Each settings file must stay inside the worktree and be no larger than 256 KiB. A configuration error names the file that needs attention.

## When a tool cannot finish

- **Server missing or environment unavailable:** check the file header and install the tool on the remote server. For a blocked `.envrc`, follow [the direnv steps above](#if-the-project-uses-direnv).
- **Settings are ignored:** check that the file is at the worktree root and uses one of the supported settings. Update Needle in **Integrations** if Newport reports an older helper.
- **An edit is refused:** the file may have changed since the request began. Review the current file and run the action again.
- **A result is outside the project:** definitions and references outside the current worktree cannot be opened here.
- **A watch error appears:** restart the language server to rescan the project.
- **A request times out:** requests stop after 30 seconds. Wait for indexing to finish, then retry or restart the language server.

The editor allows 20 open tabs, 16 MiB per editable file, and 64 MiB of open file content per worktree. Reference results show at most 500 locations. Language tools cannot create, rename, or delete files. Snippet tab stops, semantic highlighting, and inlay hints are not currently available.

[Back to the user guide](README.md)

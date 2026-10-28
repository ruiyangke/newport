# Release notes

Each release has one Markdown file named `<version>.md`, such as `0.5.2.md`.
The website imports them with the docs and shows them at
[newport.app/releases](https://newport.app/releases). They are not guides.

Start each file with YAML frontmatter:

```yaml
---
version: "0.5.2"
title: "A short headline."
date: "2026-10-25"
summary: "One sentence about the release."
---
```

`version` matches the file name and `date` is the release day (`YYYY-MM-DD`).
Set `draft: true` to keep a file off the website. Write the body with `###`
sections such as **New**, **Improved**, **Fixed** and **Before you install**.

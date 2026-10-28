# Documentation authoring rules

These instructions apply to user documentation under `docs/`. Start a new guide
from [TEMPLATE.md](TEMPLATE.md). Existing guides can adopt these conventions when
they are refactored; do not rewrite unrelated pages just to add metadata.

## Ownership and publication

- This repository owns the documentation content. Update affected guides alongside
  product changes, and verify that published instructions match the selected release.
- `newport-website` owns rendering, navigation, search, analytics, and deployment.
  Keep these docs independent of Astro and other website implementation details.
- The planned publication flow imports a reviewed snapshot from a selected tag or
  commit into the website repository and records the source revision there. Normal
  website builds use that local snapshot. Newport's build and release CI must not
  depend on the website repository, its tools, or its availability.
- `releases/` holds the release notes, one `<version>.md` per release. Its
  format is in [releases/README.md](releases/README.md); these files are not guides.
- `AGENTS.md` and `TEMPLATE.md` are authoring resources, not public guide pages.
  Exclude them, other nested `AGENTS.md` files, and draft pages from publication,
  navigation, search indexes, and sitemaps. Do not link to them from public guides.
- These are authoring and importer requirements. The website importer and automated
  validation are not implemented by adding these files.

## Markdown and metadata

- Use portable Markdown: paragraphs, headings, lists, tables, images, links, and
  fenced code blocks. Do not add MDX, component imports, executable expressions,
  scripts, or raw HTML to public guides.
- Begin each new or refactored public guide with YAML frontmatter containing
  `title`, `description`, and `slug`. Quote prose values so punctuation remains
  valid YAML. The description should be one concise sentence explaining the task
  or question the guide addresses.
- Use a unique, stable, lowercase hyphenated slug, such as `port-forwarding`.
  Use `index` only for `docs/README.md`, which maps to `/docs`; other slugs map to
  `/docs/<slug>`. Slugs contain no file extension, slash, query, or fragment.
- Set `draft: true` while content is incomplete or describes unreleased behavior.
  Remove it or set it to `false` only after review. The template starts as a draft.
- Use exactly one H1 in the body, matching `title`, so the file reads well on
  GitHub. The website must render that heading without adding a duplicate H1.
  Follow it with logically nested H2 and H3 sections.
- Keep sidebar groups, ordering, CSS, and website component settings out of the
  frontmatter. Record the source release and commit once for the imported snapshot,
  rather than repeating a version number on every page. State feature-specific
  version requirements in the prose when they affect the reader.
- Preserve published slugs. A necessary slug change requires a redirect in the
  website; changing a file name also requires updating source links.

## Links, images, and examples

- Link to other guides using relative Markdown paths, such as
  `[Troubleshooting](troubleshooting.md#cannot-connect-over-ssh)`. Keep `.md` in
  source links so they work in the repository; the website importer must rewrite
  them to the target page's published slug and preserve valid section anchors.
- Public guides must not depend on unpublished files or pages outside the docs
  snapshot. Use absolute HTTPS links for external resources.
- Store new guide images under `docs/assets/`, reference them with relative paths,
  and write descriptive alt text. Existing `docs/images/` references may remain;
  any imported snapshot must include the assets its published pages use.
- Label fenced code blocks with their language. Explain whether a command runs on
  the user's computer or the Linux server, and identify any required privileges.
- Use clearly explained placeholders and synthetic examples. Never include real
  credentials, tokens, private hostnames, or personal data in commands or images.
- Explain material effects before commands that change configuration, expose a
  service, overwrite data, or interrupt sessions. Use the smallest example needed
  to complete the task.

## Content and accuracy

- Write for Newport users. Lead with what they can accomplish, then give relevant
  prerequisites, steps, expected results, and troubleshooting. Adapt the template
  to the subject; omit sections that add no useful information.
- Use the exact UI labels and command names from the applicable product version.
  Distinguish desktop operating system support, remote Linux requirements, helper
  requirements, and trial or Pro access where relevant.
- Verify behavior against the implementation, release, or a working product
  session. Do not infer support from another platform or describe a planned
  feature as available. Keep roadmap content explicitly labeled as planned.
- Keep the product name, helper terminology, and explanations consistent across
  guides. Explain technical details when they help users make a decision or solve
  a problem; keep developer architecture and contributor workflows in the
  relevant crate or tooling documentation.
- Prefer short, descriptive headings and direct answers. Avoid promotional filler,
  repeated keywords, invented review dates, and unsupported security claims.
- Link to the authoritative guide for shared behavior instead of duplicating long
  explanations. Add new public guides to [README.md](README.md), and update its
  links when guides are renamed or removed.

## Review before publication

1. Check required metadata, unique slugs, one H1, and logical heading levels.
2. Resolve every internal file link, section anchor, and image path. Published
   pages must not link to drafts or excluded authoring resources.
3. Remove template placeholders and review platform, edition, permission, and
   version requirements. Check instructions against the release being documented.
4. Review command examples and screenshots for accuracy and unintended data.
5. Check that Markdown remains readable in the repository. When integrating into
   the website, verify rendered headings, rewritten links, images, code blocks,
   mobile layout, and metadata without requiring JavaScript to read the guide.
6. Report what was checked and any steps that could not be verified. Do not claim
   that a command or workflow was tested unless it was actually exercised.

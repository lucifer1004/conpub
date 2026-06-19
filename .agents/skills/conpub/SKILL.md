---
name: conpub
description: Use conpub to help users publish, sync, preview, search, and read local Markdown/Typst knowledge files through Confluence Cloud. Use when a user asks to configure a local knowledge base, bind a project to a Confluence space/parent page, inspect local KB content, build page hierarchy, run dry-run publish/sync, publish or sync with explicit confirmation, include images/assets, or use Confluence as a sharing/result-presentation surface while keeping local files as the source of truth.
---

# conpub

## Overview

Use `conpub` when the user wants an agent-friendly workflow for local knowledge
files and Confluence Cloud. The local filesystem is authoritative; Confluence is
only the viewing and sharing surface.

## Core Rules

- Treat local files as the source of truth. Do not infer missing knowledge by
  reading Confluence.
- Prefer JSON output and parse it directly. Use `--pretty` only for human
  inspection.
- Run `publish --dry-run` or `sync --dry-run` before remote writes.
- Use `publish --yes` or `sync --yes` only when the user explicitly wants
  Confluence writes.
- Use `sync --archive-deleted --yes` only when the user explicitly wants known
  deleted pages archived remotely.
- Do not print or persist credentials.

## Configuration

`conpub` reads shared defaults from:

```text
CONPUB_KB_ROOT
CONPUB_BASE_URL
CONPUB_SPACE
CONPUB_PARENT_ID
```

Publishing credentials are consumed by typub:

```text
CONFLUENCE_EMAIL
CONFLUENCE_API_KEY
```

Use `CONPUB_HOME` only for isolated tests or temporary agent runs.

For first-time setup:

```bash
conpub root "$CONPUB_KB_ROOT" --base-url "$CONPUB_BASE_URL"
conpub bind <source-relative-to-root> --space "$CONPUB_SPACE" --parent "$CONPUB_PARENT_ID"
conpub resolve
```

If defaults are already in the environment, this is enough:

```bash
conpub root
conpub bind <source-relative-to-root>
conpub resolve
```

Run `conpub resolve` after configuration changes and inspect the returned
`target`, `root`, and `source_abs`.

## Local Query Workflow

Use this path when the user wants to inspect local knowledge before publishing:

```bash
conpub index
conpub search "<query>"
conpub read <path>[:line]
conpub plan
```

`search` uses the index when fresh and falls back to scanning local files when
needed. Use `read` for exact local source context.

## Page Hierarchy

Root-level documents publish under the configured Confluence parent page.

For non-root directories, require `_index.md` or `index.md`. That index file is
the parent page for documents in the directory. Nested directories need an index
for every ancestor directory.

Example:

```text
project/
  notes.md
  perf/
    _index.md
    occupancy.md
```

`notes.md` publishes under the configured parent. `perf/occupancy.md` publishes
under `perf/_index.md`.

When syncing a child path, conpub includes required parent index pages in the
plan.

## Publishing Workflow

Preview first:

```bash
conpub publish --dry-run
conpub sync --dry-run
```

Use `publish` for a full publish set. Use `sync` for create/update/unchanged/
deleted classification and incremental publishing:

```bash
conpub sync --dry-run <file-or-directory>
conpub sync --yes <file-or-directory>
```

Use subset sync when the user wants to publish only a focused result page or
directory. In subset mode, conpub omits global deleted entries.

## Images And Assets

Keep shared publish assets under the configured KB root:

```text
<root>/_assets/
```

Reference them from Markdown or Typst as `assets/<name>`, for example
`![Diagram](assets/diagram.png)` or `#image("assets/diagram.png")`.
conpub maps root `_assets` into each staged typub post's `assets/` directory.
conpub stages only safe asset extensions such as images and PDFs, and skips
dotfiles and common key or credential filenames.

conpub fingerprints the document plus the safe shared `_assets` set. Changing
`_assets` can mark documents changed because typub owns exact asset reference
parsing. Other Markdown and Typst files are separate documents, not assets.

After adding or changing images:

```bash
conpub sync --dry-run <document>
conpub sync --yes <document>
```

## Deleted Pages

Default `sync` reports deleted local files but does not archive or delete remote
pages.

If the user explicitly wants remote archive for deleted pages whose IDs are
already known locally:

```bash
conpub sync --dry-run --archive-deleted
conpub sync --yes --archive-deleted
```

conpub does not search Confluence for pages to archive.

## Information Boundaries

Treat raw JSON output as local agent data, not share-ready content. It can
include local paths such as `root`, `source_abs`, `stage_root`, `state_file`,
config paths, and Confluence target IDs. Redact or summarize it before posting
to shared Confluence pages or chats.

`conpub index` writes a persistent search index under the generated stage root.
The index stores full document lines. Do not commit or share generated stage
roots, search indexes, sync state, typub status DBs, `.env` files, or
credentials.

Sync state and typub status are shared by KB root and Confluence target across
project source bindings. A bound source limits the current scan and deleted-file
detection; it does not create a separate publish state.

Precise title extraction uses the `typst` CLI. If `typst` is unavailable or
evaluation fails, conpub falls back to a filename-derived title.

## Troubleshooting

- Missing root: run `conpub root <dir>` or set `CONPUB_KB_ROOT`.
- Missing target defaults: bind with `--space`, `--parent`, and optionally
  `--base-url`, or set `CONPUB_SPACE`, `CONPUB_PARENT_ID`, and
  `CONPUB_BASE_URL`.
- Missing non-root hierarchy index: add `_index.md` or `index.md` to the
  directory.
- Remote write rejected: rerun with `--yes` only after user confirmation.
- Credential failure: verify `CONFLUENCE_EMAIL` and `CONFLUENCE_API_KEY`
  without printing their values.

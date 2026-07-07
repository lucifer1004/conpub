---
name: conpub
description: Use conpub to help users publish, sync, preview, search, and read local Markdown/Typst knowledge files through Confluence Cloud. Use when a user asks to configure a local knowledge base or Atlassian credentials, bind a project to a Confluence space/parent page, inspect local KB content, build page hierarchy, run dry-run publish/sync, publish or sync with explicit confirmation, include images/assets, or use Confluence as a sharing/result-presentation surface while keeping local files as the source of truth.
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
- Never ask the user to paste an API token into chat. Ask them to configure it
  locally and report only whether it is present.
- Do not print, read back, log, or persist credentials. Persist a credential
  only when the user explicitly requests it and the destination is private,
  access-restricted, and excluded from version control.

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

Despite its name, `CONFLUENCE_API_KEY` must contain an Atlassian personal API
token, not an Atlassian organization API key.

Use `CONPUB_HOME` only for isolated tests or temporary agent runs.

## First-Run Intake

Run `conpub resolve` first. Reuse valid existing configuration and ask only for
values that are missing or that the user wants to change. Never make the user
re-enter values available from the environment or conpub configuration. Ask for
all missing non-secret values in one concise message rather than one at a time.

For an unconfigured project, collect the minimum inputs:

1. Ask for the absolute local knowledge-base directory.
2. Derive the project source as the current directory relative to that root. If
   it cannot be derived, ask for the source-relative path; use `.` only when the
   whole root should be bound.
3. Ask for the Confluence parent-page URL. Prefer this single URL over asking
   separately for the base URL, space key, and parent page ID.
4. Ask for the Atlassian account email and whether a personal API token is
   already configured locally. Never ask for the token value.

Derive configuration from a normal Confluence Cloud page URL such as:

```text
https://example.atlassian.net/wiki/spaces/GPU/pages/123456789/Page+Title
```

- Derive the base URL as `https://example.atlassian.net/wiki`.
- Derive the space key from the segment after `/spaces/`.
- Derive the parent ID from the digits after `/pages/`. Also accept `pageId`
  from a `viewpage.action` URL; ask for the space key separately when that URL
  does not contain it.
- Preserve a personal-space key's leading `~`. Quote values such as
  `CONPUB_SPACE='~account-id'` in shell configuration to prevent expansion.

Before writing configuration, summarize the derived non-secret values for the
user. Do not include credential values in the summary.

## Atlassian API Token

Guide a user without a token to Atlassian's
[API token page](https://id.atlassian.com/manage-profile/security/api-tokens).
Use Atlassian's official
[token management guide](https://support.atlassian.com/atlassian-account/docs/manage-api-tokens-for-your-atlassian-account/)
when the UI or organization policy needs explanation:

1. Create a personal API token **without scopes**, give it a purpose-specific
   name and expiry, and copy it when shown.
2. Confirm that the token's account can view the parent page and create and edit
   pages in the target space. For documents with images or files, also confirm
   the space's **Add attachments** permission.
3. Ask the user to expose it locally as `CONFLUENCE_API_KEY` and the owning
   account email as `CONFLUENCE_EMAIL`.

Current conpub authentication uses email/token Basic Auth against the site's
`*.atlassian.net/wiki` URL. Scoped API tokens require Atlassian's
`api.atlassian.com/ex/confluence/{cloudId}` endpoint and are not supported.
Organization API keys and service-account Bearer tokens are also not supported.
If organization policy prevents creating an unscoped personal token, report the
compatibility limitation instead of requesting a different secret.

Choose the credential handoff based on who will run conpub.

When the user will run conpub in the same interactive shell, let them enter the
token without echoing it or placing it in shell history:

```bash
export CONFLUENCE_EMAIL='you@example.com'
read -rsp 'Atlassian API token: ' CONFLUENCE_API_KEY
printf '\n'
export CONFLUENCE_API_KEY
```

An export in the user's shell does not propagate into a separate agent command
process. When the agent will run conpub, ask the user to choose one of these
local handoffs without revealing the token:

- Put `[confluence]` credentials in `~/.config/conpub/conpub.toml` and restrict
  it with `chmod 600`. Ask the user to write the values; do not read them back.
- Prepare a git-ignored, mode-`600` environment file and provide only its path.
  Source that file in the same command process that runs conpub, without
  printing its contents.

Treat the user's confirmation that credentials are configured as sufficient
until the first explicitly authorized Confluence write. Never make a separate
remote request merely to expose or test the secret.

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

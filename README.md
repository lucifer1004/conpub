# conpub

`conpub` is an agent-first CLI for publishing local knowledge files to Confluence Cloud.

The local filesystem is the source of truth. Confluence is the viewing and sharing surface.

## Commands

All commands emit JSON by default. Use `--pretty` to format JSON for humans.

```bash
conpub root ~/nv-kb --base-url https://example.atlassian.net
conpub bind projects/cuda-agent --space GPU --parent 123456789

# Install or manage the conpub Agent Skill plugin:
conpub agent doctor
conpub agent install all --from-checkout <checkout-or-unpacked-plugin-archive>
conpub agent update all
conpub agent uninstall all

# Or use CONPUB_* defaults from the shell environment:
conpub root
conpub bind projects/cuda-agent

conpub resolve
conpub index
conpub search "warp occupancy"
conpub read projects/cuda-agent/perf/occupancy.md:80
conpub plan
conpub publish --dry-run
conpub publish --yes
conpub sync --dry-run
conpub sync --yes
conpub sync --dry-run perf/occupancy.md
conpub sync --yes perf/occupancy.md docs/
conpub sync --dry-run --archive-deleted
conpub sync --yes --archive-deleted
conpub prune
conpub prune --yes
conpub prune --yes --archive
conpub prune --yes --delete
conpub status
```

## Agent plugin

conpub ships one operator Agent Skill for Codex and Claude. Install the conpub binary first, then use either a source checkout or the matching release's unpacked `conpub-plugin.tar.gz`:

```bash
conpub agent doctor
conpub agent install codex --from-checkout .
conpub agent install claude --from-checkout .
```

Use `all` instead of a runtime name to operate on both. The lifecycle commands delegate to each runtime's native plugin manager and emit JSON like every other conpub command:

```bash
conpub agent update all
conpub agent uninstall all
```

The plugin installs only the Agent Skill; it does not install the conpub binary or configure Confluence credentials. Start a new agent thread after installing or updating so the runtime loads the new skill version.

`publish --dry-run` stages the local publish set and returns the documents that would be published without calling Confluence.

`publish --yes` performs Confluence writes through the public `typub` crates from crates.io.

`sync --dry-run` compares the current local documents with `conpub`'s local publish state and reports `create`, `update`, `unchanged`, and `deleted` classifications without calling Confluence.

`sync --yes` publishes only `create` and `update` documents, then records successful publishes in the local sync state. It does not read from Confluence and does not delete remote pages for locally deleted files.

`sync` accepts optional file or directory paths relative to the bound source or configured root. In subset mode it reports only selected documents and does not report global `deleted` entries.

A `deleted` entry whose Confluence page id is owned by a live document classifies as `superseded`: after a local move, provision adopts the remote page by title under the new path, so the old entry points at a page that is still alive. Superseded entries are never archived or deleted remotely — `sync` and `prune` drop them from local state without any remote action.

`prune` reconciles `deleted` state entries explicitly. Without flags it only drops them from local state, leaving remote pages in place; `prune --yes --archive` archives the pages first; `prune --yes --delete` deletes them permanently (a 404 counts as already gone, so reruns are idempotent). `--archive` and `--delete` are mutually exclusive. Without `--yes`, `prune` reports what it would do and changes nothing.

`index` writes a persistent local search index. `search` uses the index only when it is fresh for the current source and document fingerprints; otherwise it falls back to scanning local files.

## Page hierarchy

Root-level documents in the bound source are published directly under the configured Confluence parent page.

For non-root directories, add `_index.md` or `index.md`. That index document becomes the Confluence parent page for documents in the directory. Nested directories must also have index documents for every ancestor directory.

Example:

```text
projects/cuda-agent/
  notes.md
  perf/
    _index.md
    occupancy.md
    memory/
      _index.md
      bandwidth.md
```

`notes.md` publishes under the configured parent. `perf/_index.md` also publishes under the configured parent, `perf/occupancy.md` publishes under `perf/_index.md`, and `perf/memory/bandwidth.md` publishes under `perf/memory/_index.md`.

`publish` and `sync` publish parent index pages before child pages. If you run `sync perf/occupancy.md`, `conpub` includes `perf/_index.md` automatically so the child page has a local parent in the same plan.

`publish` and `sync` run serially by default. Use `--delay-ms <n>` to pause between Confluence publish calls. `--concurrency` is present for forward compatibility, but values above `1` are rejected until typub publish state can be safely shared across concurrent writes.

Shared publish assets live under the configured KB root:

```text
<root>/_assets/
```

Reference these files from Markdown or Typst as `assets/<name>`, for example `![Diagram](assets/diagram.png)` or `#image("assets/diagram.png")`. During staging, `conpub` maps the root `_assets` directory into each typub post's `assets/` directory. `conpub` stages only safe asset extensions such as images and PDFs, and skips dotfiles and common key or credential filenames.

Sync fingerprints include the document and the safe shared `_assets` set. Changing `_assets` can mark documents as changed because `conpub` intentionally leaves exact reference parsing to typub. Other Markdown and Typst files are treated as separate documents, not assets. `conpub` rejects slug collisions before planning, syncing, or publishing.

`plan` additionally scans each document for `assets/<name>` references (markdown images/links, typst `#image("assets/...")`, html `src="assets/..."`) and marks documents whose references are not present in `_assets/` as `blocked`, naming each missing reference. `publish` and `sync` refuse such a set locally, before staging or any remote write — an asset placed next to the document instead of under `<root>/_assets/` is caught here instead of failing half-way through a remote publish.

`plan` consults the local publish state (the same view `status` reports): unblocked documents carry the state-derived action — `create`, `update`, `unchanged`, or `deleted` — with the reason, the last published Confluence URL when typub status knows the page, and a `publishable` count plus per-action `summary`. A clean plan is `publishable: 0`. `plan` is entirely local; it takes no lock and writes nothing. Note that `publish` does not consult this state — it republishes the whole set unconditionally; `sync` is the incremental verb.

The local sync state is written under a per-target file lock with atomic replacement and is bound to the configured root, base URL, space, and parent page. Multiple project bindings that share the same root and Confluence target share this state. The bound source only limits the current scan and deleted-file detection scope. The state stores only local KB metadata such as fingerprints, titles, slugs, parent paths, and sync timestamps.

Remote Confluence IDs, URLs, and publish status are owned by typub's status database under the generated stage root:

```text
<stage-root>/.typub/status.db
```

For a real Confluence smoke test, bind a disposable source directory to a disposable parent page and run `conpub sync --yes <smoke-file>`. `conpub` intentionally has no default remote delete/archive behavior; deleted local files are reported as `deleted` so a human can decide with `conpub prune` (state-only, `--archive`, or `--delete`) what should happen remotely.

Use `sync --archive-deleted --yes` to archive deleted pages whose Confluence page IDs are already known in typub status. This calls Confluence Cloud's `POST /wiki/rest/api/content/archive` endpoint and removes accepted archived entries from local sync state. It does not search Confluence for pages to archive.

## Authentication

`CONFLUENCE_API_KEY` is the historical configuration name for an Atlassian **personal API token**. It is not an Atlassian organization API key.

To create a compatible token:

1. Open Atlassian's [API token page](https://id.atlassian.com/manage-profile/security/api-tokens) and sign in with the account that will publish pages. Atlassian's [token management guide](https://support.atlassian.com/atlassian-account/docs/manage-api-tokens-for-your-atlassian-account/) covers the available token types and organization-policy restrictions.
2. Create an API token **without scopes**, choose a purpose-specific name and expiration date, and copy the token when it is shown. Atlassian does not show it again.
3. Ensure that the account can view the configured parent page and create and edit pages in the target space. Publishing images or files additionally requires the space's **Add attachments** permission. Archive and permanent-delete commands require the corresponding Confluence permissions.
4. Set the account email as `CONFLUENCE_EMAIL` and the token as `CONFLUENCE_API_KEY`.

conpub currently authenticates with email/token Basic Auth against the site's `https://<site>.atlassian.net/wiki` URL. Scoped API tokens use Atlassian's `https://api.atlassian.com/ex/confluence/{cloudId}` endpoint and are not supported by this authentication path. Atlassian organization API keys and service-account Bearer tokens are not compatible either.

Do not paste a token into chat, an issue, a committed project file, or a shell command that will be retained in history.

When you will run conpub in the same interactive shell, enter the token without echo:

```bash
export CONFLUENCE_EMAIL='you@example.com'
read -rsp 'Atlassian API token: ' CONFLUENCE_API_KEY
printf '\n'
export CONFLUENCE_API_KEY
```

Those exports apply only to that shell; they do not propagate into a separately launched agent command. When an agent will run conpub, prepare one of these local credential sources yourself and tell the agent only which source to use:

- Put `[confluence]` credentials in `~/.config/conpub/conpub.toml`, then run `chmod 600 ~/.config/conpub/conpub.toml`. conpub loads this file directly for every project.
- Put the environment variables in a git-ignored file with mode `600`. The agent must source that file in the same command process that runs conpub and must never print its contents.

Confluence credentials resolve per field with this precedence:

```text
CONFLUENCE_API_KEY / CONFLUENCE_EMAIL      (environment, wins)
[confluence] api_key / email in .conpub.toml      (project)
[confluence] api_key / email in ~/.config/conpub/conpub.toml   (user)
```

Config-file credentials are stored as plaintext. Never place credentials in a project `.conpub.toml` that may be committed. The following example is intended for the mode-`600` user configuration at `~/.config/conpub/conpub.toml`:

```toml
[confluence]
api_key = "<atlassian-api-token>"
email = "you@example.com"
```

Environment variables remain useful for ephemeral or secret-manager-backed sessions. Project-level credentials are supported for compatibility but are not recommended.

Resolved credentials travel only inside the in-memory typub platform config; they are never written to the stage, and never appear in `resolve`, `plan`, `status`, or `bind` output. Re-running `conpub bind` or `conpub root` preserves an existing `[confluence]` section.

The `--base-url` value may include `/wiki`; `conpub` normalizes it before passing it to typub.

## Configuration

User configuration is stored at:

```text
~/.config/conpub/conpub.toml
```

Set `CONPUB_HOME` to override this location, primarily for tests and isolated agent runs.

These environment variables are used as fallbacks when the matching config value or CLI flag is missing:

```text
CONPUB_KB_ROOT
CONPUB_BASE_URL
CONPUB_SPACE
CONPUB_PARENT_ID
```

Config files and explicit CLI flags take precedence over these environment variables. `CONPUB_KB_ROOT` can also be used without a user config file, so a project can be bound or resolved in a temporary agent environment after sourcing a shared `.env`.

Project binding is stored in the current project as:

```text
.conpub.toml
```

The effective source directory is:

```text
<user root>/<project source>
```

## Information Boundaries

Default JSON output is agent-friendly, not share-ready. It can include local paths such as `root`, `source_abs`, `stage_root`, `state_file`, config paths, and Confluence target IDs. Redact or summarize raw JSON before pasting it into shared pages or chats.

`index` stores a persistent local search index under the generated stage root. The index includes full document lines so `search` can return local context quickly. Do not commit or share generated stage roots, search indexes, sync state, typub status databases, or `.env` files.

Precise title extraction for Typst and Markdown uses the `typst` CLI. When `typst` is unavailable or title evaluation fails, `conpub` falls back to the filename-derived title.

## Release Builds

GitHub Actions builds release artifacts with `cargo-zigbuild` for:

- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-musl`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-gnu`

Tag pushes matching `v*` create a GitHub Release with bare binaries,
per-target archives, and `.sha256` checksums. Releases also include a
version-matched `conpub-plugin.tar.gz` with its checksum for Codex and Claude.

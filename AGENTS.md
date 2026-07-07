# Agent Development Guide for conpub

This guide defines the default working rules for coding agents and contributors
working on conpub.

## Scope

Audience: agents and developers changing conpub source, tests, release
automation, or repository documentation.

This file is not the user guide for publishing content. User-facing usage lives
in `README.md`.

The local `conpub` skill is for agents helping users operate conpub. Keep this
development guide focused on repository work.

## Read First

- `README.md` for CLI behavior, configuration, page hierarchy, and release
  targets.
- `Cargo.toml` for Rust version, crate metadata, dependency surface, and the
  crates.io package whitelist.
- `.github/workflows/ci.yml` and `.github/workflows/release.yml` before
  changing validation, packaging, or release behavior.
- `plugins/conpub/skills/conpub/SKILL.md` before changing the user-facing conpub
  skill.
- `scripts/check-plugin-package.py` before changing plugin manifests or moving
  skill files.

## Project Shape

conpub is an agent-first Rust CLI. The local filesystem is the knowledge source;
Confluence is only the publishing and sharing surface.

The source tree follows a small DDD-style split:

- `src/domain/`: local concepts and rules such as documents, hierarchy, sync
  planning, and model types.
- `src/application/`: command orchestration and JSON response assembly.
- `src/infrastructure/`: filesystem, config, title extraction, typub,
  Confluence archive, search index, and state adapters.
- `src/support/`: shared error, JSON, runtime, and validation helpers.
- `tests/cli.rs`: black-box CLI behavior coverage.

Keep new behavior in the narrowest layer that owns it. Do not move domain rules
into CLI parsing or infrastructure adapters unless the rule is genuinely tied to
that boundary.

## CLI Contract

- JSON is the default output contract. Human formatting should be opt-in.
- Remote writes require explicit confirmation flags such as `--yes`.
- Dry-run commands must avoid Confluence writes.
- The local filesystem remains authoritative. Do not add Confluence reads as a
  knowledge-source fallback.
- `sync` state is local metadata only. Remote Confluence IDs, URLs, and publish
  status belong to typub status under the generated stage root.
- Sync state and typub status are scoped by KB root and Confluence target, not
  by project source. Source bindings limit scans and deleted-file detection.

## Configuration And Secrets

conpub supports these shared defaults:

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

Do not commit `.env`, credentials, personal Confluence page IDs, or local smoke
test artifacts. Use disposable Confluence pages for remote smoke tests.

Shared publish assets belong under `<root>/_assets` and are staged as typub
`assets/` entries. Keep staging whitelist behavior conservative: do not restore
generic sibling-file publishing or broad asset discovery in document folders.

## Development Commands

Run the focused command first, then broaden when the change touches shared
behavior.

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo publish --dry-run --allow-dirty --locked
python3 scripts/check-plugin-package.py .
scripts/pack-plugin.sh /tmp/conpub-plugin.tar.gz
```

For release-path validation, exercise at least one zigbuild target when the
local toolchain has `cargo-zigbuild` and `zig`:

```bash
rustup target add x86_64-unknown-linux-musl
cargo zigbuild --locked --release --target x86_64-unknown-linux-musl
```

Use `cargo publish --dry-run --allow-dirty --locked` before the initial commit
or while the nested repository has untracked files. Use the stricter CI command
without `--allow-dirty` once the package files are committed.

## Coding Rules

- Prefer existing module patterns over new abstractions.
- Keep errors structured through the existing `AppError` and JSON response
  helpers.
- Preserve deterministic output for agent workflows.
- Avoid global current-directory changes. If a dependency requires one, guard it
  with a lock and restore the previous directory.
- Treat typub as the owner of rendering, Confluence publish mechanics, and
  remote publish status.
- Keep comments short and only where they clarify non-obvious constraints.

## Testing Expectations

Add or adjust tests when changing:

- CLI arguments, JSON response shape, or error messages.
- hierarchy, sync planning, title extraction, fingerprinting, or archive
  behavior.
- state layout, status DB interactions, file locking, or atomic writes.
- release or CI workflow behavior.

Prefer CLI integration tests for user-visible behavior and unit tests for small
domain or infrastructure invariants.

## Governance

Use `govctl` for governed work items and loop state.

Basic flow:

```bash
govctl status
govctl work show <WI-ID>
govctl check --has-active
govctl loop start <WI-ID>
govctl loop run <LOOP-ID>
```

Fill the loop round summary before closing a loop. Do not hand-edit work items;
use `govctl work ...` commands.

## Release Automation

GitHub Actions use floating major tags for actions. Check upstream release pages
before changing action versions, then keep workflow `uses:` values on the
current auto-updating major tag.

The cargo-zigbuild container is pinned by digest. Treat that separately from
GitHub Action major tags.

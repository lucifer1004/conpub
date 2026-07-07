#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT=${1:?usage: pack-plugin.sh <out.tar.gz>}
case "$OUT" in
    /*) ;;
    *) OUT="$(pwd)/$OUT" ;;
esac

python3 "$ROOT/scripts/check-plugin-package.py" "$ROOT"

cd "$ROOT"
tar --sort=name \
    --owner=root --group=root --numeric-owner \
    --mtime='2026-01-01 00:00:00 UTC' \
    -cf - LICENSE .agents/plugins .claude-plugin plugins/conpub \
    | gzip -n > "$OUT"

listing=$(tar -tzf "$OUT")
for required in \
    LICENSE \
    .agents/plugins/marketplace.json \
    .claude-plugin/marketplace.json \
    plugins/conpub/.codex-plugin/plugin.json \
    plugins/conpub/.claude-plugin/plugin.json \
    plugins/conpub/skills/conpub/SKILL.md
do
    printf '%s\n' "$listing" | grep -qx "$required"
done

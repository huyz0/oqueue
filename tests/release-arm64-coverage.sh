#!/usr/bin/env bash
# M13.6: the native ARM release job must exercise the store crate, and the
# musl decision must be explicit rather than inferred from a missing target.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workflow="$ROOT/.github/workflows/release.yml"
artifacts="$ROOT/docs/release/artifacts.md"
decision="$ROOT/docs/internal/product/decisions/0071-musl-release-target.md"

job="$(sed -n '/^  linux-aarch64:/,/^  [a-z][a-z-]*:/p' "$workflow")"
grep -Fq 'runs-on: ubuntu-24.04-arm' <<<"$job"
grep -Fq 'cargo test --locked -p oqueue-store --all-targets' <<<"$job"
grep -Fq 'bash tests/release-arm64-coverage.sh' <<<"$job"

grep -Fq 'aarch64-unknown-linux-musl' "$artifacts"
grep -Fq 'not a shipped release artifact' "$artifacts"
grep -Fq 'Status: accepted' "$decision"
grep -Fq 'Requirements: NFR-40, NFR-42' "$decision"
grep -Fq 'default allocator' "$decision"

printf '%s\n' 'release-arm64-coverage: ok'

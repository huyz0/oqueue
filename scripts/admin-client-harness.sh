#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
exec python3 scripts/harness/admin_client_harness.py

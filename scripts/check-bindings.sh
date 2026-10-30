#!/usr/bin/env bash
# Fails when the committed generated files (apps/desktop/src/bindings, and the JSON in apps/desktop/src/lib) differ from
# what the crates write now: scripts/bindings.sh writes them again, then
#   - in a git checkout: `git diff --exit-code` and no untracked or deleted files under those folders;
#   - in a copy without .git (an rsync to a server): a recursive diff against <before>, the folders as they were before
#     anything wrote them (scripts/ci-linux.sh copies them first, as its test steps write the bindings too).
#   scripts/check-bindings.sh [before]
set -uo pipefail
cd "$(dirname "$0")/.."
paths=(apps/desktop/src/bindings apps/desktop/src/lib)
stale() {
  echo "the bindings changed: run pnpm bindings and commit them" >&2
  exit 1
}
scripts/bindings.sh || exit 1
if git rev-parse --is-inside-work-tree > /dev/null 2>&1; then
  git diff --exit-code --stat -- "${paths[@]}" || stale
  changed=$(git status --porcelain --untracked-files=all -- "${paths[@]}")
  [ -z "$changed" ] || { echo "$changed"; stale; }
  exit 0
fi
before=${1:?"not a git checkout: pass the folders as they were before (scripts/ci-linux.sh does)"}
diff -r "$before/bindings" apps/desktop/src/bindings && diff -r "$before/lib" apps/desktop/src/lib || stale

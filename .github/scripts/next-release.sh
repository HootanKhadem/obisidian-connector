#!/usr/bin/env bash
# Works out the next release from the Conventional Commits since the latest v* tag.
#
#   feat:                         minor bump
#   fix: / perf:                  patch bump
#   type!: or "BREAKING CHANGE:"  major bump (minor while the version is 0.x)
#   anything else (docs, chore, ci, test, refactor, style, build)  no release
#
# Prints `version=X.Y.Z` (empty when nothing needs releasing) and writes the
# release notes to the file named by $NOTES_FILE (default release-notes.md).
set -euo pipefail

notes_file="${NOTES_FILE:-release-notes.md}"
last_tag="$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)"
if [[ -n "$last_tag" ]]; then
  range="$last_tag..HEAD"
  current="${last_tag#v}"
else
  range="HEAD"
  current="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
fi

IFS=. read -r major minor patch <<<"$current"
bump=0 # 0 none, 1 patch, 2 minor, 3 major
breaking="" features="" fixes=""
header='^([a-z]+)(\([^)]*\))?(!)?: (.+)$'

while IFS= read -r -d $'\x1e' commit; do
  commit="${commit#$'\n'}"
  hash="${commit%%$'\x1f'*}"
  rest="${commit#*$'\x1f'}"
  subject="${rest%%$'\x1f'*}"
  body="${rest#*$'\x1f'}"
  [[ "$subject" =~ $header ]] || continue
  type="${BASH_REMATCH[1]}"
  scope="${BASH_REMATCH[2]}"
  description="${BASH_REMATCH[4]}"
  entry="- ${scope:+**${scope:1:-1}:** }$description (${hash:0:7})"$'\n'
  if [[ -n "${BASH_REMATCH[3]}" ]] || grep -qE '^BREAKING[ -]CHANGE: ' <<<"$body"; then
    breaking+="$entry"
    bump=3
    continue
  fi
  case "$type" in
    feat) features+="$entry"; ((bump < 2)) && bump=2 ;;
    fix | perf) fixes+="$entry"; ((bump < 1)) && bump=1 ;;
  esac
done < <(git log --no-merges --format='%H%x1f%s%x1f%b%x1e' "$range")

if ((bump == 3 && major == 0)); then
  bump=2 # Before 1.0, a breaking change bumps the minor version (SemVer item 4).
fi
case "$bump" in
  3) next="$((major + 1)).0.0" ;;
  2) next="$major.$((minor + 1)).0" ;;
  1) next="$major.$minor.$((patch + 1))" ;;
  *) next="" ;;
esac

{
  [[ -n "$breaking" ]] && printf '### Breaking changes\n\n%s\n' "$breaking"
  [[ -n "$features" ]] && printf '### Features\n\n%s\n' "$features"
  [[ -n "$fixes" ]] && printf '### Bug fixes\n\n%s\n' "$fixes"
  true
} >"$notes_file"

echo "version=$next"

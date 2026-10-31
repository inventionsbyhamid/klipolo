# Shared by the release scripts: sourced, never run. Nothing here prints a signing identity or a secret.

# The Developer ID Application identity's SHA-1, never its name, and never printed: KLIPOLO_SIGNING_IDENTITY when the
# caller sets it, else the one such identity in the keychains (the login keychain on the Mac that signs releases),
# APPLE_TEAM_ID when there are several.
developer_id() {
  if [ -n "${KLIPOLO_SIGNING_IDENTITY:-}" ]; then
    [[ "$KLIPOLO_SIGNING_IDENTITY" =~ ^[0-9A-F]{40}$ ]] || {
      echo "refused: KLIPOLO_SIGNING_IDENTITY is an identity's SHA-1 (40 hex digits)" >&2
      return 1
    }
    printf '%s\n' "$KLIPOLO_SIGNING_IDENTITY"
    return
  fi
  local found
  found=$(security find-identity -v -p codesigning 2> /dev/null | grep '"Developer ID Application: ' \
    | { if [ -n "${APPLE_TEAM_ID:-}" ]; then grep -F "($APPLE_TEAM_ID)\""; else cat; fi; } \
    | sed -n 's/^ *[0-9][0-9]*) \([0-9A-F]\{40\}\) .*/\1/p' | sort -u || true)
  case "$(printf '%s' "$found" | grep -c .)" in
    1) printf '%s\n' "$found" ;;
    0) echo "refused: no Developer ID Application identity in the keychain" >&2; return 1 ;;
    *) echo "refused: several Developer ID Application identities; set APPLE_TEAM_ID to choose one" >&2; return 1 ;;
  esac
}

# A tool's output with the identity taken out (`... | redact "$id"`, or `... | redact` for its name only).
redact() {
  if [ -n "${1:-}" ]; then
    sed -e "s/$1/[Developer ID]/g" -e 's/Developer ID Application: [^"]*/Developer ID Application: [redacted]/g'
  else
    sed -e 's/Developer ID Application: [^"]*/Developer ID Application: [redacted]/g'
  fi
}

# A Mach-O file, by its magic (32 or 64 bit, either byte order, or universal).
is_macho() {
  case "$(head -c 4 "$1" | od -An -tx1 | tr -d ' \n')" in
    cffaedfe | cefaedfe | feedfacf | feedface | cafebabe | cafebabf | bebafeca | bfbafeca) return 0 ;;
    *) return 1 ;;
  esac
}

# The files under <path>... that name the home folder <home> (anywhere) or the user <user> (only as a whole path part,
# /<user>/, so an account named test or build doesn't match "testing" or "builder"):
#   naming_files <home> <user> <path>...
# A user name shorter than 4 characters isn't looked for: /lib/ and /src/ are in every binary. Prints the files found
# and returns grep's status: 0 some found, 1 none, 2 a file couldn't be read (the files found are printed all the same).
naming_files() {
  local home=$1 user=$2
  shift 2
  local patterns=(-e "$home")
  [ "${#user}" -lt 4 ] || patterns+=(-e "/$user/")
  LC_ALL=C grep -rlF "${patterns[@]}" "$@"
}

# The checkout <repo>'s source files vendored from Cap's MIT crates, as paths from <repo>, one a line, sorted: each .rs
# file under crates/<crate>/src whose first line is `// SPDX-License-Identifier: MIT` and whose first four lines name
# Cap Software, Inc. (crates/klipolo-capture/NOTICE lists them). Returns 1 when a folder or a file couldn't be read
# (the files found are printed all the same).
cap_mit_files() {
  local repo=$1 list f found="" rc=0
  [ -d "$repo/crates" ] || return 0
  list=$(mktemp)
  (cd "$repo" && find crates -path 'crates/*/src/*' -name '*.rs' -type f -print0) > "$list" || rc=1
  while IFS= read -r -d '' f; do
    if [ ! -r "$repo/$f" ]; then rc=1; continue; fi
    [ "$(head -n 1 "$repo/$f")" = '// SPDX-License-Identifier: MIT' ] || continue
    case "$(head -n 4 "$repo/$f")" in *'Cap Software, Inc.'*) found+="$f"$'\n' ;; esac
  done < "$list"
  rm -f "$list"
  printf '%s' "$found" | LC_ALL=C sort
  return "$rc"
}

# The public address of Klipolo's source code, as NOTICE.txt gives it: https://, a host, and a path of plain URL
# characters (no spaces, quotes, & or \).
is_source_url() {
  local re='^https://[A-Za-z0-9.-]+(/[A-Za-z0-9._~/%+=@:#?-]*)?$'
  [[ "$1" =~ $re ]]
}

# A full git commit id: 40 lowercase hex digits, as `git rev-parse HEAD` prints it.
is_commit() {
  [[ "$1" =~ ^[0-9a-f]{40}$ ]]
}

# <notice>'s source code line filled in, in place: {{SOURCE_URL}} becomes <url> and {{SOURCE_COMMIT}} <commit>, the
# commit the app is built from: fill_source <notice> <url> <commit>. Nothing changes unless both are good. Each
# replacement is quoted in an assignment, so bash 5.2's patsub_replacement never reads a & in it, and bash 3.2 (which
# keeps quotes in a replacement inside "${...}") takes it as plain text too.
fill_source() {
  local url_placeholder='{{SOURCE_URL}}' commit_placeholder='{{SOURCE_COMMIT}}' text
  is_source_url "$2" || { echo "refused: not an https:// source code address: $2" >&2; return 1; }
  is_commit "$3" || { echo "refused: not a full commit id: $3" >&2; return 1; }
  text=$(< "$1")
  text=${text//"$url_placeholder"/"$2"}
  text=${text//"$commit_placeholder"/"$3"}
  printf '%s\n' "$text" > "$1"
}

# A release version: X.Y.Z, digits and dots only, as CFBundleShortVersionString and CFBundleVersion require.
is_version() {
  [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
}

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

# A release version: X.Y.Z, with an optional pre-release part.
is_version() {
  [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]
}

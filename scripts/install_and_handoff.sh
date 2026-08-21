#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
usage: scripts/install_and_handoff.sh [--install-root <directory>]

Install the current Herdr checkout, then hand a running server to the new
binary without stopping its panes. If no server is running, install only.

Options:
  --install-root <directory>  Cargo install root. Defaults to
                              HERDR_INSTALL_ROOT, CARGO_INSTALL_ROOT,
                              CARGO_HOME, or $HOME/.cargo.
  -h, --help                  Show this help.

Environment:
  CARGO   Cargo executable to use. Defaults to cargo.
  PYTHON  Python executable used to parse status JSON. Defaults to python3.
USAGE
}

fail() {
  echo "error: $*" >&2
  exit 1
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd -- "$script_dir/.." && pwd)"
cargo_bin="${CARGO:-cargo}"
python_bin="${PYTHON:-python3}"
install_root="${HERDR_INSTALL_ROOT:-${CARGO_INSTALL_ROOT:-${CARGO_HOME:-${HOME:?HOME is not set}/.cargo}}}"

while (($#)); do
  case "$1" in
    --install-root)
      (($# >= 2)) || fail "--install-root requires a directory"
      install_root="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$(uname -s)" in
  Darwin|Linux) ;;
  *) fail "live handoff is supported only on macOS and Linux" ;;
esac

command -v "$cargo_bin" >/dev/null 2>&1 || fail "cargo executable not found: $cargo_bin"
command -v "$python_bin" >/dev/null 2>&1 || fail "python executable not found: $python_bin"
[[ -n "$install_root" ]] || fail "install root must not be empty"

installed_bin="$install_root/bin/herdr"

parse_status() {
  local kind="$1"
  "$python_bin" -c '
import json
import sys

kind = sys.argv[1]
data = json.load(sys.stdin)
if kind == "client":
    version = data.get("version") or "-"
    protocol = data.get("protocol")
    print(f"{version}|{protocol if protocol is not None else chr(45)}")
else:
    running = bool(data.get("running"))
    version = data.get("version") or "-"
    protocol = data.get("protocol")
    capabilities = data.get("capabilities") or {}
    handoff = bool(capabilities.get("live_handoff"))
    print(
        f"{int(running)}|{version}|"
        f"{protocol if protocol is not None else chr(45)}|{int(handoff)}"
    )
' "$kind"
}

echo "installing current checkout from $repo_dir"
"$cargo_bin" install \
  --path "$repo_dir" \
  --locked \
  --force \
  --root "$install_root"

[[ -x "$installed_bin" ]] || fail "cargo did not install an executable at $installed_bin"

client_json="$($installed_bin status client --json)" \
  || fail "failed to read installed client status from $installed_bin"
client_fields="$(printf '%s' "$client_json" | parse_status client)" \
  || fail "installed client returned invalid status JSON"
IFS='|' read -r target_version target_protocol <<< "$client_fields"
[[ "$target_version" != "-" && "$target_protocol" != "-" ]] \
  || fail "installed client status is missing version or protocol"

server_json="$($installed_bin status server --json)" \
  || fail "failed to inspect the running server"
server_fields="$(printf '%s' "$server_json" | parse_status server)" \
  || fail "server returned invalid status JSON"
IFS='|' read -r running current_version current_protocol live_handoff <<< "$server_fields"

echo "installed Herdr $target_version (protocol $target_protocol) at $installed_bin"

if [[ "$running" != 1 ]]; then
  echo "no running server; the next Herdr launch will use the installed binary"
  exit 0
fi

[[ "$live_handoff" == 1 ]] \
  || fail "running server $current_version (protocol $current_protocol) does not support live handoff"

echo "handing off server $current_version (protocol $current_protocol)"
"$installed_bin" server live-handoff \
  --import-exe "$installed_bin" \
  --expected-protocol "$target_protocol" \
  --expected-version "$target_version"

for _ in {1..100}; do
  if updated_json="$($installed_bin status server --json 2>/dev/null)"; then
    if updated_fields="$(printf '%s' "$updated_json" | parse_status server 2>/dev/null)"; then
      IFS='|' read -r updated_running updated_version updated_protocol _ <<< "$updated_fields"
      if [[ "$updated_running" == 1 \
        && "$updated_version" == "$target_version" \
        && "$updated_protocol" == "$target_protocol" ]]; then
        echo "live handoff complete: server $updated_version (protocol $updated_protocol)"
        exit 0
      fi
    fi
  fi
  sleep 0.1
done

fail "live handoff returned, but the replacement server did not become ready"

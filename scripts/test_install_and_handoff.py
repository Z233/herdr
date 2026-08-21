from __future__ import annotations

import os
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "install_and_handoff.sh"


class InstallAndHandoffTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.base = Path(self.temp_dir.name)
        self.install_root = self.base / "cargo"
        self.cargo_log = self.base / "cargo.log"
        self.herdr_log = self.base / "herdr.log"
        self.server_running = self.base / "server-running"
        self.handoff_done = self.base / "handoff-done"

        self.fake_cargo = self.base / "fake-cargo"
        self.fake_herdr = self.base / "fake-herdr"
        self.fake_cargo.write_text(
            """#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >> "$FAKE_CARGO_LOG"
root=""
while (($#)); do
  if [[ "$1" == "--root" ]]; then
    root="$2"
    shift 2
  else
    shift
  fi
done
[[ -n "$root" ]]
mkdir -p "$root/bin"
cp "$FAKE_HERDR_SOURCE" "$root/bin/herdr"
chmod +x "$root/bin/herdr"
"""
        )
        self.fake_herdr.write_text(
            """#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >> "$FAKE_HERDR_LOG"
if [[ "${1:-}" == "status" && "${2:-}" == "client" && "${3:-}" == "--json" ]]; then
  printf '{"version":"0.9.0","protocol":20,"binary":"%s"}\\n' "$0"
  exit 0
fi
if [[ "${1:-}" == "status" && "${2:-}" == "server" && "${3:-}" == "--json" ]]; then
  if [[ -e "$FAKE_SERVER_RUNNING" ]]; then
    if [[ -e "$FAKE_HANDOFF_DONE" ]]; then
      printf '{"status":"running","running":true,"version":"0.9.0","protocol":20,"capabilities":{"live_handoff":true}}\\n'
    else
      printf '{"status":"running","running":true,"version":"0.8.0","protocol":19,"capabilities":{"live_handoff":%s}}\\n' "${FAKE_LIVE_HANDOFF:-true}"
    fi
  else
    printf '{"status":"stopped","running":false}\\n'
  fi
  exit 0
fi
if [[ "${1:-}" == "server" && "${2:-}" == "live-handoff" ]]; then
  if [[ "${FAKE_HANDOFF_FAIL:-0}" == "1" ]]; then
    echo "simulated handoff failure" >&2
    exit 1
  fi
  touch "$FAKE_HANDOFF_DONE"
  exit 0
fi
echo "unexpected fake herdr invocation: $*" >&2
exit 2
"""
        )
        self.fake_cargo.chmod(0o755)
        self.fake_herdr.chmod(0o755)

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def run_script(self, **overrides: str) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env.update(
            {
                "CARGO": str(self.fake_cargo),
                "FAKE_CARGO_LOG": str(self.cargo_log),
                "FAKE_HANDOFF_DONE": str(self.handoff_done),
                "FAKE_HERDR_LOG": str(self.herdr_log),
                "FAKE_HERDR_SOURCE": str(self.fake_herdr),
                "FAKE_SERVER_RUNNING": str(self.server_running),
                "HERDR_INSTALL_ROOT": str(self.install_root),
            }
        )
        env.update(overrides)
        return subprocess.run(
            [str(SCRIPT)],
            cwd=self.base,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_installs_checkout_and_hands_running_server_to_new_binary(self) -> None:
        self.server_running.touch()

        result = self.run_script()

        self.assertEqual(result.returncode, 0, result.stderr)
        installed_bin = self.install_root / "bin" / "herdr"
        self.assertTrue(installed_bin.is_file())
        cargo_args = self.cargo_log.read_text().strip().split()
        self.assertEqual(cargo_args[:2], ["install", "--path"])
        self.assertEqual(Path(cargo_args[2]), ROOT)
        self.assertIn("--locked", cargo_args)
        self.assertIn("--force", cargo_args)
        self.assertEqual(cargo_args[-2:], ["--root", str(self.install_root)])

        invocations = self.herdr_log.read_text().splitlines()
        handoff = next(line for line in invocations if line.startswith("server live-handoff"))
        self.assertIn(f"--import-exe {installed_bin}", handoff)
        self.assertIn("--expected-protocol 20", handoff)
        self.assertIn("--expected-version 0.9.0", handoff)
        self.assertTrue(self.handoff_done.exists())
        self.assertIn("live handoff complete", result.stdout)

    def test_installs_without_handoff_when_server_is_stopped(self) -> None:
        result = self.run_script()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.install_root / "bin" / "herdr").is_file())
        self.assertFalse(self.handoff_done.exists())
        self.assertNotIn("server live-handoff", self.herdr_log.read_text())
        self.assertIn("no running server", result.stdout)

    def test_refuses_to_replace_server_without_live_handoff_capability(self) -> None:
        self.server_running.touch()

        result = self.run_script(FAKE_LIVE_HANDOFF="false")

        self.assertEqual(result.returncode, 1)
        self.assertFalse(self.handoff_done.exists())
        self.assertIn("does not support live handoff", result.stderr)

    def test_handoff_failure_is_reported(self) -> None:
        self.server_running.touch()

        result = self.run_script(FAKE_HANDOFF_FAIL="1")

        self.assertEqual(result.returncode, 1)
        self.assertFalse(self.handoff_done.exists())
        self.assertIn("simulated handoff failure", result.stderr)


if __name__ == "__main__":
    unittest.main()

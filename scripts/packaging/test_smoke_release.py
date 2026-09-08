#!/usr/bin/env python3
"""Real subprocess regressions for the release startup gate; no Qt/build required."""

import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("smoke_release", Path(__file__).with_name("smoke_release.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)

STARTUP = 'print("QbzCore initialized\\n" + "normal startup log\\n" * 10, flush=True)\n'


@unittest.skipUnless(os.name == "posix", "release helper targets Linux/macOS")
class ProcessTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="qbz-release-smoke-test-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.log = self.root / "smoke.log"

    def run_fake(self, code, seconds=.25):
        return smoke.observe([sys.executable, "-u", "-c", code], dict(os.environ),
                             seconds, self.log, grace=.15)

    def assert_reaped(self, pid):
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)
        with self.assertRaises(ChildProcessError):
            os.waitpid(pid, os.WNOHANG)

    def test_core_then_exit_zero_is_rejected_and_reaped(self):
        with self.assertRaisesRegex(RuntimeError, "exited before"):
            self.run_fake(STARTUP)
        result = json.loads(Path(str(self.log) + ".json").read_text())
        self.assertEqual(result["returncode"], 0)
        self.assert_reaped(result["pid"])

    def test_core_then_signal_crash_is_rejected_without_core_dump(self):
        with self.assertRaisesRegex(RuntimeError, "exited before"):
            self.run_fake(STARTUP + 'import os, resource, signal\n'
                          'assert resource.getrlimit(resource.RLIMIT_CORE) == (0, 0)\n'
                          'os.kill(os.getpid(), signal.SIGSEGV)\n')
        result = json.loads(Path(str(self.log) + ".json").read_text())
        self.assertEqual(result["returncode"], -signal.SIGSEGV)
        self.assert_reaped(result["pid"])

    def test_wrapper_cannot_hide_early_child_exit(self):
        # AppRun/flatpak may propagate a child's status via a waiting wrapper.
        # Even a wrapper that maps that status to 0 must not pass early.
        for status in ("0", "7"):
            with self.subTest(status=status), self.assertRaisesRegex(RuntimeError, "exited before"):
                child = STARTUP + "raise SystemExit(" + status + ")\n"
                self.run_fake('import subprocess, sys\n'
                              f'subprocess.run([sys.executable, "-u", "-c", {child!r}])\n')

    def test_live_process_is_accepted_and_reaped(self):
        result = self.run_fake(STARTUP + "import time; time.sleep(60)\n")
        self.assertTrue(result["survived"])
        self.assertGreaterEqual(result["observed_seconds"], .25)
        self.assertEqual(result["returncode"], -signal.SIGTERM)
        self.assert_reaped(result["pid"])

    def test_live_process_ignoring_term_is_killed_and_reaped(self):
        result = self.run_fake('import signal, time\n'
                              'signal.signal(signal.SIGTERM, signal.SIG_IGN)\n'
                              + STARTUP + 'time.sleep(60)\n')
        self.assertEqual(result["returncode"], -signal.SIGKILL)
        self.assert_reaped(result["pid"])

    def test_waiting_wrapper_and_its_child_are_stopped(self):
        code = ('import subprocess, sys, signal, time\n'
                'child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])\n'
                'def stop(sig, frame):\n'
                '    child.wait(timeout=2)\n'
                '    raise SystemExit(0)\n'
                'signal.signal(signal.SIGTERM, stop)\n'
                'print("child=" + str(child.pid), flush=True)\n' + STARTUP + 'time.sleep(60)\n')
        result = self.run_fake(code)
        child = int(self.log.read_text().splitlines()[0].split("=", 1)[1])
        self.assert_reaped(result["pid"])
        with self.assertRaises(ProcessLookupError):
            os.kill(child, 0)

    def test_alive_without_core_or_with_qml_errors_still_fails(self):
        for logs in ('print("log\\n" * 12, flush=True)\n',
                     STARTUP + 'print("ReferenceError: missing", flush=True)\n'):
            with self.subTest(logs=logs), self.assertRaises(RuntimeError):
                self.run_fake(logs + 'import time; time.sleep(60)\n')
            result = json.loads(Path(str(self.log) + ".json").read_text())
            self.assertTrue(result["survived"])
            self.assert_reaped(result["pid"])

    def test_sdk_is_hidden_and_restored_after_gate_failure(self):
        sdk = self.root / "Qt SDK"
        sdk.mkdir()
        (sdk / "marker").write_text("original")
        with self.assertRaisesRegex(RuntimeError, "exited before"):
            with smoke.hidden_sdk(sdk):
                self.assertFalse(sdk.exists())
                self.run_fake(STARTUP)
        self.assertEqual((sdk / "marker").read_text(), "original")
        self.assertEqual(list(self.root.glob("*.qbz-smoke-hidden-*")), [])

    def test_spawn_failure_also_restores_sdk(self):
        sdk = self.root / "Qt"
        sdk.mkdir()
        with self.assertRaises(FileNotFoundError):
            with smoke.hidden_sdk(sdk):
                smoke.observe([str(self.root / "missing")], {}, .1, self.log)
        self.assertTrue(sdk.is_dir())

    @unittest.skipUnless(sys.platform.startswith("linux"), "Linux private D-Bus fixture")
    def test_complete_gate_has_private_bus_profile_and_reaps_after_failure(self):
        fake = self.root / "qbz"
        fake.write_text('#!' + sys.executable + '\nimport json, os\n' + STARTUP +
                        'print(json.dumps({name: os.environ[name] for name in '
                        '["HOME", "XDG_DATA_HOME", "DBUS_SESSION_BUS_ADDRESS"]}), flush=True)\n')
        fake.chmod(0o755)
        original_home = os.environ.get("HOME")
        with self.assertRaisesRegex(RuntimeError, "exited before"):
            smoke.smoke(fake, .25, self.log)
        snapshot = json.loads(self.log.read_text().splitlines()[-1])
        self.assertNotEqual(snapshot["HOME"], original_home)
        self.assertIn("qbz-release-smoke-", snapshot["HOME"])
        root = Path(snapshot["HOME"]).parent
        self.assertIn(str(root), snapshot["DBUS_SESSION_BUS_ADDRESS"])
        self.assertFalse(root.exists())
        self.assertEqual(os.environ.get("HOME"), original_home)

    def test_cancellation_restores_sdk_and_cleans_observed_process(self):
        # Exercise the CLI's SIGTERM handler, not a mocked finally block.
        sdk = self.root / "Qt"
        sdk.mkdir()
        fake = self.root / "qbz"
        fake.write_text('#!' + sys.executable + '\nimport os, time\n' + STARTUP +
                        'print("product_pid=" + str(os.getpid()), flush=True)\ntime.sleep(60)\n')
        fake.chmod(0o755)
        gate = subprocess.Popen([sys.executable, str(Path(smoke.__file__)), str(fake),
                                 "--hide-qt", str(sdk), "--seconds", "60", "--log", str(self.log)],
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        try:
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                if self.log.exists() and "product_pid=" in self.log.read_text():
                    break
                if gate.poll() is not None:
                    self.fail(gate.communicate(timeout=2)[0])
                time.sleep(.025)
            else:
                self.fail("fake product did not start")
            self.assertFalse(sdk.exists())
            pid = int(self.log.read_text().split("product_pid=", 1)[1].strip())
            gate.terminate()
            output, _ = gate.communicate(timeout=10)
            self.assertEqual(gate.returncode, 1, output)
            self.assertIn("interrupted by signal", output)
            self.assertTrue(sdk.is_dir())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
        finally:
            if gate.poll() is None:
                gate.kill()
            gate.communicate(timeout=5)


class EnvironmentTests(unittest.TestCase):
    def test_native_macos_home_and_sdk_overrides_are_isolated(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original = {"HOME": "/personal", "PATH": "/SDK/bin:/usr/bin",
                        "CFFIXED_USER_HOME": "/personal", "QT_ROOT_DIR": "/SDK",
                        "DYLD_FRAMEWORK_PATH": "/SDK/lib", "QML_IMPORT_PATH": "/SDK/qml",
                        "DBUS_SESSION_BUS_ADDRESS": "unix:path=/personal/bus",
                        "QBZ_RENDERER": "software", "XDG_DATA_HOME": "/personal/data"}
            env = smoke.isolated_environment(root, original, "darwin")
            self.assertEqual(original["HOME"], "/personal")
            self.assertEqual(env["HOME"], str(root / "home"))
            self.assertEqual(env["CFFIXED_USER_HOME"], env["HOME"])
            self.assertTrue((root / "home/Library/Application Support").is_dir())
            self.assertTrue((root / "home/Library/Caches").is_dir())
            self.assertEqual(env["QT_QPA_PLATFORM"], "offscreen")
            self.assertNotIn("SDK", env["PATH"])
            for name in ("QT_ROOT_DIR", "DYLD_FRAMEWORK_PATH", "QML_IMPORT_PATH",
                         "DBUS_SESSION_BUS_ADDRESS", "QBZ_RENDERER"):
                self.assertNotIn(name, env)

    def test_flatpak_finds_user_install_but_product_uses_sandbox_profile(self):
        with tempfile.TemporaryDirectory(prefix="qbz-release-smoke-") as directory:
            root = Path(directory)
            original = {"HOME": "/runner", "XDG_DATA_HOME": "/runner/data",
                        "XDG_CONFIG_HOME": "/runner/config"}
            env = smoke.isolated_environment(root, original, "linux")
            env["DBUS_SESSION_BUS_ADDRESS"] = "unix:path=" + str(root / "bus")
            with patch.object(smoke.shutil, "which", return_value="/usr/bin/flatpak"):
                command, host_env = smoke.flatpak_command("com.blitzfc.qbz", env, original, root)
            self.assertEqual(host_env["HOME"], "/runner")
            self.assertEqual(host_env["XDG_DATA_HOME"], "/runner/data")
            self.assertEqual(host_env["DBUS_SESSION_BUS_ADDRESS"], env["DBUS_SESSION_BUS_ADDRESS"])
            self.assertIn("--die-with-parent", command)
            self.assertIn("--env=HOME=/tmp/" + root.name + "/home", command)
            self.assertIn("--env=XDG_DATA_HOME=/tmp/" + root.name + "/data", command)
            self.assertIn("--unset-env=QML_IMPORT_PATH", command)
            self.assertFalse(any(arg.startswith("--filesystem=") for arg in command))
            self.assertTrue(command[-1].endswith("exec /app/bin/qbz"))


if __name__ == "__main__":
    unittest.main()

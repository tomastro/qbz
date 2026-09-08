#!/usr/bin/env python3
"""SDK-cache and crash-aware smoke regressions; no Qt or Cargo build needed."""

import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
import sys
from unittest.mock import patch

sys.dont_write_bytecode = True


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


qt = load("qt_cargo", "qt-cargo.py")
smoke = load("qt_smoke", "qt-smoke.py")


class SdkTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.headers = self.root / "include" / "QtCore"
        self.headers.mkdir(parents=True)
        self.header = self.headers / "qarraydataops.h"
        self.header.write_text("old-layout")
        self.answers = {"QT_VERSION": "6.11.2", "QT_INSTALL_HEADERS": str(self.headers.parent),
                        "QT_INSTALL_LIBS": str(self.root / "lib")}
        mock = patch.object(qt, "query", side_effect=lambda _, key: self.answers[key])
        mock.start()
        self.addCleanup(mock.stop)

    def fingerprint(self):
        return qt.sdk_fingerprint("/sdk/bin/qmake", self.answers["QT_VERSION"])

    def test_identical_headers_are_stable_after_touch(self):
        before = self.fingerprint()
        os.utime(self.header, None)
        self.assertEqual(before, self.fingerprint())

    def test_header_change_invalidates_even_with_same_version_size_and_mtime(self):
        before = self.fingerprint()
        stat = self.header.stat()
        self.header.write_text("new-layout")
        os.utime(self.header, ns=(stat.st_atime_ns, stat.st_mtime_ns))
        self.assertNotEqual(before, self.fingerprint())

    def test_patch_version_invalidates(self):
        before = self.fingerprint()
        self.answers["QT_VERSION"] = "6.11.3"
        self.assertNotEqual(before, self.fingerprint())

    def test_library_root_invalidates(self):
        before = self.fingerprint()
        self.answers["QT_INSTALL_LIBS"] = "/other-sdk/lib"
        self.assertNotEqual(before, self.fingerprint())

    def test_added_and_removed_headers_invalidate(self):
        before = self.fingerprint()
        extra = self.headers / "qnew.h"
        extra.write_text("new type")
        self.assertNotEqual(before, self.fingerprint())
        extra.unlink()
        self.assertEqual(before, self.fingerprint())

    @unittest.skipIf(os.name == "nt", "unprivileged Windows symlinks may be unavailable")
    def test_framework_symlink_and_cycle_are_bounded(self):
        framework = self.root / "framework"
        framework.mkdir()
        header = framework / "qframework.h"
        header.write_text("old")
        (self.headers / "framework").symlink_to(framework, target_is_directory=True)
        (framework / "cycle").symlink_to(self.headers, target_is_directory=True)
        before = self.fingerprint()
        header.write_text("new")
        self.assertNotEqual(before, self.fingerprint())

    def test_missing_sdk_fails_closed(self):
        self.answers["QT_INSTALL_HEADERS"] = str(self.root / "missing")
        with self.assertRaisesRegex(RuntimeError, "missing"):
            self.fingerprint()

    def test_empty_sdk_fails_closed(self):
        self.header.unlink()
        with self.assertRaisesRegex(RuntimeError, "empty"):
            self.fingerprint()

    def test_homebrew_framework_only_headers_are_hashed(self):
        self.header.unlink()
        headers = self.root / "lib" / "QtCore.framework" / "Headers"
        headers.mkdir(parents=True)
        header = headers / "qarraydataops.h"
        header.write_text("old")
        before = self.fingerprint()
        header.write_text("new")
        self.assertNotEqual(before, self.fingerprint())

    def test_shared_include_root_ignores_non_qt_packages(self):
        before = self.fingerprint()
        unrelated = self.headers.parent / "other-package"
        unrelated.mkdir()
        (unrelated / "other.h").write_text("not part of Qt")
        self.assertEqual(before, self.fingerprint())

    def test_existing_flags_survive_and_nested_calls_are_stable(self):
        original = {"CXXFLAGS": "/Zc:__cplusplus /permissive- -include arm_acle.h",
                    "RUSTFLAGS": "-C link-arg=-fuse-ld=lld", "CARGO_TARGET_DIR": "/cache"}
        with patch.object(qt, "find_qmake", return_value=("/sdk/bin/qmake", "6.11.2")):
            env, _, _ = qt.cargo_environment(original)
            again, _, _ = qt.cargo_environment(env)
        self.assertEqual(env, again)
        self.assertTrue(env["CXXFLAGS"].startswith(original["CXXFLAGS"] + " "))
        self.assertEqual(env["RUSTFLAGS"], original["RUSTFLAGS"])
        self.assertEqual(env["CARGO_TARGET_DIR"], "/cache")
        self.assertNotIn("QMAKE", original)

    def test_broken_explicit_qmake_does_not_fall_back(self):
        with patch.object(qt.shutil, "which", return_value=None) as which:
            with self.assertRaises(RuntimeError):
                qt.find_qmake({"QMAKE": "/missing/sdk/qmake"})
        which.assert_called_once_with("/missing/sdk/qmake", path=None)


class SmokeTests(unittest.TestCase):
    output = "QbzCore initialized\n" + "normal startup log\n" * 10

    def test_success_requires_liveness(self):
        smoke.check_result(self.output, True, -15)

    def test_early_exit_is_never_green_even_after_success_marker(self):
        for status in (0, 1, 139, -11, -6):
            with self.subTest(status=status), self.assertRaisesRegex(RuntimeError, "exited"):
                smoke.check_result(self.output, False, status)

    def test_crash_at_deadline_is_not_our_intentional_shutdown(self):
        for status in (139, -11, 134, -6, 1):
            with self.subTest(status=status), self.assertRaisesRegex(RuntimeError, "status"):
                smoke.check_result(self.output, True, status)

    def test_silent_hang_is_not_startup(self):
        with self.assertRaises(RuntimeError):
            smoke.check_result("", True, -15)

    def test_missing_core_marker_fails(self):
        with self.assertRaisesRegex(RuntimeError, "initialization"):
            smoke.check_result("log\n" * 12, True, -15)

    def test_qml_error_after_init_fails(self):
        with self.assertRaisesRegex(RuntimeError, "QML complaints"):
            smoke.check_result(self.output + "ReferenceError: missing binding", True, -15)

    def test_known_property_cache_warning_does_not_hide_real_error(self):
        warning = "qt.qml.propertyCache: has no property\n"
        smoke.check_result(self.output + warning, True, -15)
        with self.assertRaises(RuntimeError):
            smoke.check_result(self.output + warning + "TypeError: broken", True, -15)


if __name__ == "__main__":
    unittest.main()

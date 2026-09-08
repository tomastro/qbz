#!/usr/bin/env python3
"""Bounded, isolated startup gate; surviving until the deadline is required.

The process must both initialize QbzCore and remain alive for the observation
window. Never turn a SIGSEGV (or a single-instance early exit) into a green
gate just because the log contains a success marker.
"""

import argparse
import contextlib
import os
from pathlib import Path
import re
import shutil
import signal
import selectors
import socket
import subprocess
import sys
import tempfile
import threading

QML_ERROR = re.compile(
    r"is not a type|unavailable|ReferenceError|TypeError|Cannot read|"
    r"Unable to assign|Cannot open|no such method|non-existent property|"
    r"failed to load component|is not installed", re.IGNORECASE
)


def check_result(output, survived, returncode):
    if not survived:
        raise RuntimeError(f"process exited before the smoke deadline (status {returncode})")
    # A child may fault just as its dbus-run-session parent reaches the
    # timeout. Do not accept a reported crash as our intentional shutdown.
    if returncode not in (0, -signal.SIGTERM, 128 + signal.SIGTERM,
                          -signal.SIGKILL, 128 + signal.SIGKILL):
        raise RuntimeError(f"unexpected process exit status {returncode}")
    lines = output.splitlines()
    if len(lines) < 10:
        raise RuntimeError(f"app produced only {len(lines)} log lines")
    complaints = [line for line in lines if "propertyCache" not in line and QML_ERROR.search(line)]
    if complaints:
        raise RuntimeError("QML complaints:\n" + "\n".join(complaints[:20]))
    if "QbzCore initialized" not in output:
        raise RuntimeError("app never reached QbzCore initialization")


@contextlib.contextmanager
def stalled_session_bus(path):
    """Accept connections but never answer AUTH; no real session is touched."""
    peers = []
    stopped = threading.Event()
    with socket.socket(socket.AF_UNIX) as listener:
        listener.bind(str(path))
        listener.listen(64)
        listener.settimeout(.1)

        def accept():
            while not stopped.is_set():
                try:
                    peer, _ = listener.accept()
                    peers.append(peer)
                except socket.timeout:
                    continue

        worker = threading.Thread(target=accept)
        worker.start()
        try:
            yield "unix:path=" + str(path)
        finally:
            stopped.set()
            worker.join(timeout=2)
            for peer in peers:
                peer.close()
            if worker.is_alive():
                raise RuntimeError("silent D-Bus fixture failed to stop")


@contextlib.contextmanager
def private_x_display(log):
    """Xcb exercises QDesktopUnixServices; offscreen misses its D-Bus wait."""
    with open(str(log) + ".xvfb", "wb") as output:
        process = subprocess.Popen(
            ["Xvfb", "-displayfd", "1", "-screen", "0", "1280x900x24", "-nolisten", "tcp"],
            stdout=subprocess.PIPE, stderr=output)
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                if not selector.select(5):
                    raise RuntimeError("private Xvfb did not publish a display")
                display = process.stdout.readline().strip().decode()
                if not display.isdigit():
                    raise RuntimeError("private Xvfb exited without a display")
            yield ":" + display
        finally:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)
            process.stdout.close()


def smoke(binary, seconds, log, silent_bus=False):
    if seconds <= 0:
        raise RuntimeError("observation window must be positive")
    command = [str(Path(binary).resolve())]
    if sys.platform.startswith("linux") and not silent_bus:
        bus = shutil.which("dbus-run-session")
        if not bus:
            raise RuntimeError("dbus-run-session is required for an isolated Linux smoke")
        command = [bus, "--", *command]
    with contextlib.ExitStack() as stack:
        directory = stack.enter_context(tempfile.TemporaryDirectory(prefix="qbz-smoke-"))
        root = Path(directory)
        env = dict(os.environ)
        # Do not inherit the developer's renderer overrides or session bus.
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS",
                     "DBUS_SESSION_BUS_PID", "QBZ_RENDERER", "QT_QUICK_BACKEND",
                     "QSG_RHI_BACKEND"):
            env.pop(name, None)
        for name, folder in (("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data"),
                             ("XDG_CACHE_HOME", "cache"), ("XDG_STATE_HOME", "state"),
                             ("XDG_RUNTIME_DIR", "run")):
            path = root / folder
            path.mkdir(mode=0o700)
            env[name] = str(path)
        env.update(QT_QPA_PLATFORM="offscreen", RUST_LOG="info")
        # dirs::data_dir ignores XDG on macOS/Windows. This helper deliberately
        # refuses there until native profile isolation is implemented.
        if not sys.platform.startswith("linux"):
            raise RuntimeError("isolated qt-smoke.py currently supports Linux only")
        if silent_bus:
            env.update(
                DBUS_SESSION_BUS_ADDRESS=stack.enter_context(stalled_session_bus(root / "bus")),
                DISPLAY=stack.enter_context(private_x_display(log)),
                QT_QPA_PLATFORM="xcb", LIBGL_ALWAYS_SOFTWARE="1")
        # A deliberately crashing regression binary must not leave a huge
        # core dump in the checkout. This limit is private to the helper and
        # its children, not a system setting.
        import resource
        resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        with open(log, "wb") as output:
            process = subprocess.Popen(command, env=env, stdout=output,
                                       stderr=subprocess.STDOUT, start_new_session=True)
            survived = False
            try:
                process.wait(timeout=seconds)
            except subprocess.TimeoutExpired:
                survived = process.poll() is None
            finally:
                # Kill/reap the entire *private* session, including D-Bus.
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
        output = Path(log).read_text(errors="replace")
        check_result(output, survived, process.returncode)
        if silent_bus:
            for marker in ("total deadline of 2000 ms expired",
                           "D-Bus disabled for this launch before Qt startup", "resolved="):
                if marker not in output:
                    raise RuntimeError(f"silent D-Bus regression missed marker: {marker}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary")
    # Keep the existing CI's 75s budget: first-run bundle discovery is async
    # network work and can legitimately outlast 30s before core init logs.
    parser.add_argument("--seconds", type=float, default=75)
    parser.add_argument("--log", required=True)
    parser.add_argument("--silent-bus", action="store_true",
                        help="exercise Qt/xcb startup against a private bus that stalls AUTH (requires Xvfb)")
    args = parser.parse_args()
    try:
        smoke(args.binary, args.seconds, args.log, args.silent_bus)
    except (OSError, RuntimeError, subprocess.SubprocessError):
        if Path(args.log).is_file():
            print(f"Last startup messages — {args.log}:", file=sys.stderr)
            print("\n".join(Path(args.log).read_text(errors="replace").splitlines()[-30:]),
                  file=sys.stderr)
        raise
    print(f"smoke OK: core initialized, process survived {args.seconds:g}s — {args.log}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"smoke FAILED: {error}", file=sys.stderr)
        sys.exit(1)

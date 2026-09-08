#!/usr/bin/env python3
"""Observe a deployed Linux/macOS product, including early exit and cleanup.

AppRun and Flatpak remain the launchers under test. macOS runs the deployed
bundle executable. These are startup checks, without a session or playback;
offscreen rendering does not certify a physical display or audio device.
"""

import argparse
import contextlib
import importlib.util
import json
import os
from pathlib import Path
import re
import resource
import selectors
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid
from xml.sax.saxutils import escape

sys.dont_write_bytecode = True
_spec = importlib.util.spec_from_file_location(
    "qt_smoke", Path(__file__).resolve().parents[1] / "qt-smoke.py")
_startup = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_startup)

# Keep the existing startup/log contract, including its propertyCache exception.
LOADER_ERROR = re.compile(r"Library not loaded|dyld:|This application failed to start", re.I)
UNSET = (
    "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "DYLD_FRAMEWORK_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH", "DYLD_FALLBACK_FRAMEWORK_PATH",
    "QT_PLUGIN_PATH", "QML_IMPORT_PATH", "QML2_IMPORT_PATH", "QT_ROOT_DIR",
    "QTDIR", "QMAKE", "QT_QPA_PLATFORM_PLUGIN_PATH", "QT_QPA_PLATFORM",
    "DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "DBUS_SESSION_BUS_PID",
    "QBZ_RENDERER", "QT_QUICK_BACKEND", "QSG_RHI_BACKEND", "QT_SCALE_FACTOR",
    "QBZ_PROFILE",
)
PROFILE_DIRS = {
    "HOME": "home", "XDG_CONFIG_HOME": "config", "XDG_DATA_HOME": "data",
    "XDG_CACHE_HOME": "cache", "XDG_STATE_HOME": "state", "XDG_RUNTIME_DIR": "run",
}


def isolated_environment(root, original, platform):
    env = {name: value for name, value in original.items() if name not in UNSET}
    for name, folder in PROFILE_DIRS.items():
        path = root / folder
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
        env[name] = str(path)
    if platform == "darwin":
        # dirs::{data,config,cache}_dir use HOME/Library on macOS, not XDG.
        # CoreFoundation/Qt also get the private home; the shell's home is unchanged.
        env["CFFIXED_USER_HOME"] = env["HOME"]
        for folder in ("Application Support", "Caches", "Preferences"):
            (Path(env["HOME"]) / "Library" / folder).mkdir(parents=True, mode=0o700)
    env.update(PATH="/usr/bin:/bin:/usr/sbin:/sbin", QT_QPA_PLATFORM="offscreen", RUST_LOG="info")
    return env


def no_core_dump():
    # Child-only limit: injected crashes must not write cores in the checkout.
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


def signal_group(process, sig):
    try:
        os.killpg(process.pid, sig)
    except ProcessLookupError:
        pass


def stop_group(process, grace=3):
    """Always reap the launcher; also kill descendants if it exits first."""
    signal_group(process, signal.SIGTERM)
    try:
        process.wait(timeout=grace)
    except subprocess.TimeoutExpired:
        pass
    finally:
        # A wrapper may have exited while a child ignored SIGTERM. Checking
        # only process.poll() here would leak that child.
        signal_group(process, signal.SIGKILL)
        process.wait(timeout=3)


def observe(command, env, seconds, log, grace=3):
    if seconds <= 0:
        raise RuntimeError("observation window must be positive")
    started = time.monotonic()
    survived = False
    with Path(log).open("wb") as output:
        process = subprocess.Popen(command, env=env, stdout=output,
                                   stderr=subprocess.STDOUT, start_new_session=True,
                                   preexec_fn=no_core_dump)
        try:
            try:
                process.wait(timeout=seconds)
            except subprocess.TimeoutExpired:
                survived = process.poll() is None
        finally:
            # An exit immediately after the deadline is still an application
            # exit, not a shutdown we initiated. Preserve its actual status.
            if process.poll() is not None:
                survived = False
            observed_seconds = time.monotonic() - started
            stop_group(process, grace)
    result = {"command": command, "pid": process.pid, "survived": survived,
              "observed_seconds": observed_seconds, "returncode": process.returncode}
    Path(str(log) + ".json").write_text(json.dumps(result, indent=2) + "\n")
    output = Path(log).read_text(errors="replace")
    _startup.check_result(output, survived, process.returncode)
    if LOADER_ERROR.search(output):
        raise RuntimeError("loader complaints in deployed product log")
    return result


@contextlib.contextmanager
def private_bus(root, env, log):
    daemon = shutil.which("dbus-daemon", path=env["PATH"])
    if daemon is None:
        raise RuntimeError("dbus-daemon is required for an isolated Linux smoke")
    # No service directories: a private bus must not activate the runner's
    # portals/keyring/desktop helpers outside the process group we own.
    config = root / "bus.conf"
    config.write_text('<busconfig><type>session</type><auth>EXTERNAL</auth>'
                      '<listen>' + escape("unix:path=" + str(root / "bus")) + '</listen>'
                      '<policy context="default"><allow user="*"/>'
                      '<allow own="*"/><allow send_destination="*"/>'
                      '<allow receive_sender="*"/></policy></busconfig>')
    with Path(str(log) + ".dbus").open("wb") as output:
        process = subprocess.Popen(
            [daemon, "--config-file=" + str(config), "--nofork", "--nopidfile", "--print-address=1"],
            env=env, stdout=subprocess.PIPE, stderr=output, start_new_session=True)
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                if not selector.select(5):
                    raise RuntimeError("private D-Bus did not publish its address")
                address = process.stdout.readline().decode().strip()
            if not address.startswith("unix:") or process.poll() is not None:
                raise RuntimeError("private D-Bus failed to start")
            yield address
        finally:
            stop_group(process)
            process.stdout.close()


@contextlib.contextmanager
def hidden_sdk(path):
    if path is None:
        yield
        return
    source = Path(path).resolve(strict=True)
    if not source.is_dir() or source == Path(source.anchor):
        raise RuntimeError("Qt SDK path must name its installation directory")
    hidden = source.with_name(source.name + ".qbz-smoke-hidden-" + uuid.uuid4().hex)
    source.rename(hidden)
    try:
        yield
    finally:
        hidden.rename(source)


@contextlib.contextmanager
def cleanup_on_signal():
    def interrupt(signum, _frame):
        # Allow finally blocks to finish even if cancellation sends another signal.
        for sig in (signal.SIGINT, signal.SIGTERM):
            signal.signal(sig, signal.SIG_IGN)
        raise RuntimeError(f"smoke interrupted by signal {signum}")

    previous = {sig: signal.signal(sig, interrupt) for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        yield
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def flatpak_command(app, env, original, root):
    flatpak = shutil.which("flatpak", path=env["PATH"])
    if not flatpak:
        raise RuntimeError("flatpak is required")
    host_env = dict(env)
    # The CLI must find the installation made by `flatpak install --user`.
    # Only the sandboxed product gets a fresh profile; the bus remains private.
    for name in ("HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME"):
        if name in original:
            host_env[name] = original[name]
        else:
            host_env.pop(name, None)
    # Flatpak has its own /tmp. No extra filesystem permission is granted.
    sandbox_root = Path("/tmp") / root.name
    command = [flatpak, "run", "--user", "--die-with-parent", "--no-a11y-bus"]
    command.extend("--unset-env=" + name for name in UNSET
                   if name not in ("DBUS_SESSION_BUS_ADDRESS", "DBUS_SESSION_BUS_PID"))
    command.extend("--env=" + name + "=" + str(sandbox_root / folder)
                   for name, folder in PROFILE_DIRS.items())
    command.extend(["--env=QT_QPA_PLATFORM=offscreen", "--env=RUST_LOG=info",
                    "--command=sh", app, "-c",
                    'set -eu; umask 077; mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" '
                    '"$XDG_CACHE_HOME" "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR"; '
                    'exec /app/bin/qbz'])
    return command, host_env


def smoke(target, seconds, log, *, flatpak=False, qt_sdk=None):
    if sys.platform != "darwin" and not sys.platform.startswith("linux"):
        raise RuntimeError("deployed smoke supports Linux and macOS only")
    if flatpak and not sys.platform.startswith("linux"):
        raise RuntimeError("Flatpak smoke requires Linux")
    log = Path(log).resolve()
    command = None if flatpak else [str(Path(target).absolute())]
    with cleanup_on_signal(), contextlib.ExitStack() as stack:
        directory = stack.enter_context(tempfile.TemporaryDirectory(prefix="qbz-release-smoke-"))
        root = Path(directory)
        original = dict(os.environ)
        env = isolated_environment(root, original, sys.platform)
        if sys.platform.startswith("linux"):
            env["DBUS_SESSION_BUS_ADDRESS"] = stack.enter_context(private_bus(root, env, log))
        if flatpak:
            command, env = flatpak_command(target, env, original, root)
        stack.enter_context(hidden_sdk(qt_sdk))
        return observe(command, env, seconds, log)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", help="deployed executable/AppRun, or Flatpak app id")
    parser.add_argument("--flatpak", action="store_true")
    parser.add_argument("--hide-qt", type=Path, help="temporarily hide this SDK; restored even on failure")
    parser.add_argument("--seconds", type=float, default=60)
    parser.add_argument("--log", required=True, type=Path)
    args = parser.parse_args()
    try:
        result = smoke(args.target, args.seconds, args.log, flatpak=args.flatpak, qt_sdk=args.hide_qt)
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        if args.log.is_file():
            print("\n".join(args.log.read_text(errors="replace").splitlines()[-30:]), file=sys.stderr)
        print(f"deployed smoke FAILED: {error}", file=sys.stderr)
        return 1
    print(f"deployed smoke OK: core initialized, survived {result['observed_seconds']:.1f}s; "
          f"launcher reaped (status {result['returncode']}) — {args.log}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

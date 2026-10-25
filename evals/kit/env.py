"""An isolated environment for a live run, and the macOS sandbox wrapper around it.

Copied from `evals/longform/__main__.py` (`isolated_env`, `sandbox_wrapper`) so the soak does not
depend on a suite that is due to be removed.
"""

import os
import shutil
import subprocess
import sys

# Only these reach the app and the agent's commands, besides the scratch HOME/TMP folders.
# SYSTEMROOT/SYSTEMDRIVE are needed on Windows; the SSL variables only name certificate files.
KEPT = ("PATH", "OPENCODE_KEY", "SYSTEMROOT", "SYSTEMDRIVE", "SSL_CERT_FILE", "SSL_CERT_DIR")


def isolated_env(workdir):
    """A minimal environment: scratch HOME/TMP (no real home or app data), PATH and the provider key."""
    home = os.path.join(workdir, "home")
    tmp = os.path.join(workdir, "tmp")
    os.makedirs(home, exist_ok=True)
    os.makedirs(tmp, exist_ok=True)
    env = {k: os.environ[k] for k in KEPT if k in os.environ}
    env.update(HOME=home, USERPROFILE=home, APPDATA=home, LOCALAPPDATA=home, XDG_DATA_HOME=home,
               XDG_CONFIG_HOME=home, TMPDIR=tmp, TEMP=tmp, TMP=tmp)
    return env


# The app's Keychain service (`KEYCHAIN_SERVICE` in crates/desktop/src/secrets.rs).
KEYCHAIN_SERVICE = "ai.themis.desktop"


def keychain_visible(env):
    """True when a Themis Keychain entry can be found under `env` (macOS only). The app reads the
    Keychain before `OPENCODE_KEY`, and only the scratch HOME hides the real login keychain, so a
    run must not start when this is true. Only existence is checked, never the secret."""
    if sys.platform != "darwin" or not shutil.which("security"):
        return False
    found = subprocess.run(["security", "find-generic-password", "-s", KEYCHAIN_SERVICE],
                           env=env, capture_output=True, check=False)
    return found.returncode == 0


def sandbox_wrapper(runner, workdir, mode):
    """macOS `sandbox-exec` prefix that blocks reads/writes under /Users and /Volumes except the
    runner binary and the scratch directory. Network is NOT restricted. Returns [] when unavailable.

    macOS cannot start the app's own sandbox inside this one, so this profile also gives the
    project's `.themis/` the app's protection: only the runner itself (the app) writes there,
    never the commands the agent runs."""
    if mode == "off" or sys.platform != "darwin" or not shutil.which("sandbox-exec") or not os.path.isfile(runner[0]):
        return []
    real = os.path.realpath
    profile = os.path.join(workdir, "sandbox.sb")
    with open(profile, "w", encoding="utf-8") as handle:
        handle.write('(version 1)\n(allow default)\n'
                     '(deny file-read* (subpath "/Users") (subpath "/Volumes") (subpath "/private/etc/ssh"))\n'
                     '(deny file-write* (subpath "/Users") (subpath "/Volumes"))\n'
                     f'(allow file-read* (literal "{real(runner[0])}") (subpath "{real(workdir)}"))\n'
                     f'(deny file-write* (require-all (subpath "{real(workdir)}/project/.themis")'
                     f' (require-not (process-path "{real(runner[0])}"))))\n')
    return ["sandbox-exec", "-f", profile]

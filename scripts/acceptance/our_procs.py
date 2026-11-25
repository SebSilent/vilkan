#!/usr/bin/env python3
"""List PIDs of OUR runtime processes, scoped to our Proton prefix.

A foreign Wine/Proton app (another game) shares the generic process names
wineserver / winedevice.exe / xalia.exe. Matching those names alone would both
false-positive the ban check and risk killing a foreign app during cleanup. We
therefore scope to processes whose STEAM_COMPAT_DATA_PATH / WINEPREFIX is OUR
prefix (CSM_DATA_DIR/prefix, from the external config), plus our distinctive
command-line signatures as a fallback. Prints one PID per line; always exit 0.
No title-derived constant is stored here — the prefix comes from the config.
"""
import os
import sys
import glob


def our_prefix():
    # argv[1] / CSM_OUR_PREFIX is the FULL prefix directory (run.sh sets
    # STEAM_COMPAT_DATA_PATH=$VILKAN_DATA/prefix; VILKAN_DATA defaults to the
    # repo-sibling vilkan-data, i.e. dirname(CSM_DATA_DIR)).
    return os.environ.get("CSM_OUR_PREFIX") or (sys.argv[1] if len(sys.argv) > 1 else "")


def main():
    prefix = our_prefix().rstrip("/")
    me = os.getpid()
    sig = ("vilkan.exe", "victory_gpu", "Proton - Experimental")
    pids = []
    for ent in glob.glob("/proc/[0-9]*"):
        try:
            pid = int(ent.rsplit("/", 1)[1])
        except ValueError:
            continue
        if pid == me:
            continue
        try:
            with open(ent + "/environ", "rb") as f:
                env = f.read().split(b"\0")
        except OSError:
            env = []
        match = False
        if prefix:
            for kv in env:
                if kv.startswith((b"STEAM_COMPAT_DATA_PATH=", b"WINEPREFIX=")):
                    val = kv.split(b"=", 1)[1].decode("utf-8", "replace").rstrip("/")
                    if val == prefix or val.startswith(prefix + "/"):
                        match = True
                        break
        if not match:
            try:
                with open(ent + "/cmdline", "rb") as f:
                    cmd = f.read().replace(b"\0", b" ").decode("utf-8", "replace")
            except OSError:
                cmd = ""
            if any(s in cmd for s in sig):
                match = True
            elif prefix and (prefix + "/launch.bat") in cmd.replace("\\", "/"):
                match = True
        if match:
            pids.append(pid)
    for p in sorted(pids):
        print(p)


if __name__ == "__main__":
    main()
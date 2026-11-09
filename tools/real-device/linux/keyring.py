#!/usr/bin/env python3
"""keyring.py status|lock|unlock: the run's throwaway login collection, on the private bus.

Run through `harness.sh keyring ...`, which puts it on the session's bus. `unlock` uses
the random password the session made ($BWH_RUN/kcpass), so no prompt is raised.
"""
import os
import sys

if not os.environ.get("BWH_RUN"):
    sys.exit("run this through `harness.sh run`, never against your own session")

from gi.repository import Gio, GLib

SVC = "org.freedesktop.secrets"
COLLECTION = "/org/freedesktop/secrets/collection/login"
bus = Gio.bus_get_sync(Gio.BusType.SESSION)


def call(path, iface, method, args, reply):
    return bus.call_sync(SVC, path, iface, method, args, GLib.VariantType(reply), 0, 5000, None).unpack()


def locked():
    (v,) = call(COLLECTION, "org.freedesktop.DBus.Properties", "Get",
                GLib.Variant("(ss)", ("org.freedesktop.Secret.Collection", "Locked")), "(v)")
    return bool(v)


cmd = sys.argv[1] if len(sys.argv) > 1 else "status"
if cmd == "lock":
    done, prompt = call("/org/freedesktop/secrets", "org.freedesktop.Secret.Service", "Lock",
                        GLib.Variant("(ao)", ([COLLECTION],)), "(aoo)")
    print("locked", list(done), "prompt", prompt)
elif cmd == "unlock":
    with open(os.path.join(os.environ["BWH_RUN"], "kcpass"), "rb") as f:
        pw = f.read()
    _, session = call("/org/freedesktop/secrets", "org.freedesktop.Secret.Service", "OpenSession",
                      GLib.Variant("(sv)", ("plain", GLib.Variant("s", ""))), "(vo)")
    call("/org/freedesktop/secrets", "org.gnome.keyring.InternalUnsupportedGuiltRiddenInterface",
         "UnlockWithMasterPassword",
         GLib.Variant("(o(oayays))", (COLLECTION, (session, b"", pw, "text/plain"))), "()")
    print("unlocked")
elif cmd != "status":
    sys.exit(__doc__)
print("Locked =", locked())

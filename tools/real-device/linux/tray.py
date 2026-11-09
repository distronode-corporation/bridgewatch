#!/usr/bin/env python3
"""Drive the app's StatusNotifierItem menu over dbusmenu, as a tray host would.

tray.py layout            print the menu tree (id, label, enabled, type)
tray.py click <label>     AboutToShow + Event(id, "clicked") on the item with that label
"""
import os
import sys

if not os.environ.get("BWH_RUN"):
    sys.exit("run this through `harness.sh run`, never against your own session")

import json

from gi.repository import Gio, GLib

bus = Gio.bus_get_sync(Gio.BusType.SESSION)
watcher = Gio.DBusProxy.new_sync(bus, 0, None, "org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher",
                                 "org.kde.StatusNotifierWatcher", None)
items = watcher.get_cached_property("RegisteredStatusNotifierItems").unpack()
if not items:
    sys.exit("no StatusNotifierItem registered")
item = items[-1]
svc, _, path = item.partition("/")
path = "/" + path
props = Gio.DBusProxy.new_sync(bus, 0, None, svc, path, "org.freedesktop.DBus.Properties", None)
menu_path = props.call_sync("Get", GLib.Variant("(ss)", ("org.kde.StatusNotifierItem", "Menu")), 0, 5000, None).unpack()[0]
menu = Gio.DBusProxy.new_sync(bus, 0, None, svc, menu_path, "com.canonical.dbusmenu", None)


def layout():
    rev, tree = menu.call_sync("GetLayout", GLib.Variant("(iias)", (0, -1, [])), 0, 5000, None).unpack()
    return tree


def walk(node, depth=0, out=None):
    nid, p, kids = node
    out.append((depth, nid, p))
    for k in kids:
        walk(k, depth + 1, out)
    return out


def flat():
    return walk(layout(), 0, [])


cmd = sys.argv[1]
if cmd == "layout":
    for depth, nid, p in flat():
        if nid == 0:
            print(f"item {item}, menu {menu_path}")
            continue
        label = p.get("label", "")
        print("  " * depth + f"[{nid}] {label!r} type={p.get('type', 'standard')} enabled={p.get('enabled', True)}"
              + (f" toggle={p.get('toggle-type')}:{p.get('toggle-state')}" if p.get("toggle-type") else "")
              + (f" children={p.get('children-display')}" if p.get("children-display") else ""))
elif cmd == "json":
    print(json.dumps([{"depth": d, "id": i, **{k: v for k, v in p.items()}} for d, i, p in flat()], default=str))
elif cmd == "click":
    want = sys.argv[2]
    match = [(i, p) for d, i, p in flat() if p.get("label", "").replace("_", "") == want]
    if not match:
        sys.exit(f"no item labelled {want!r}")
    nid = match[0][0]
    try:
        menu.call_sync("AboutToShow", GLib.Variant("(i)", (0,)), 0, 5000, None)
    except GLib.Error:
        pass
    menu.call_sync("Event", GLib.Variant("(isvu)", (nid, "clicked", GLib.Variant("s", ""), 0)), 0, 5000, None)
    print(f"clicked [{nid}] {want!r}")

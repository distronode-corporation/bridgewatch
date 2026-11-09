#!/usr/bin/env python3
"""Stand-in desktop services on the private session bus, so the harness needs no
notification daemon or tray host installed:

- org.freedesktop.Notifications: records each Notify to <log>/notifications.jsonl,
  advertises "actions" and "body", and emits ActionInvoked/NotificationClosed on
  request through org.bwtest.Control.Invoke(id, action_key).
- org.kde.StatusNotifierWatcher: accepts RegisterStatusNotifierItem, says a host
  is registered, and records the items to <log>/sni.jsonl.
"""
import os
import sys

if not os.environ.get("BWH_RUN"):
    sys.exit("run this through `harness.sh start`, never against your own session")

import json
import time

from gi.repository import Gio, GLib

LOG = sys.argv[1]

NOTIF_XML = """
<node>
 <interface name="org.freedesktop.Notifications">
  <method name="GetCapabilities"><arg type="as" direction="out"/></method>
  <method name="Notify">
   <arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/>
   <arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/>
   <arg type="a{sv}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/>
  </method>
  <method name="CloseNotification"><arg type="u" direction="in"/></method>
  <method name="GetServerInformation">
   <arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/>
  </method>
  <signal name="NotificationClosed"><arg type="u"/><arg type="u"/></signal>
  <signal name="ActionInvoked"><arg type="u"/><arg type="s"/></signal>
 </interface>
 <interface name="org.bwtest.Control">
  <method name="Invoke"><arg type="u" direction="in"/><arg type="s" direction="in"/></method>
 </interface>
</node>"""

SNW_XML = """
<node>
 <interface name="org.kde.StatusNotifierWatcher">
  <method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method>
  <method name="RegisterStatusNotifierHost"><arg type="s" direction="in"/></method>
  <property name="RegisteredStatusNotifierItems" type="as" access="read"/>
  <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
  <property name="ProtocolVersion" type="i" access="read"/>
  <signal name="StatusNotifierItemRegistered"><arg type="s"/></signal>
  <signal name="StatusNotifierItemUnregistered"><arg type="s"/></signal>
  <signal name="StatusNotifierHostRegistered"/>
 </interface>
</node>"""

state = {"next": 1, "items": []}


def log(fname, rec):
    rec["t"] = time.strftime("%H:%M:%S", time.gmtime()) + f".{int(time.time()*1000)%1000:03d}Z"
    with open(f"{LOG}/{fname}", "a") as f:
        f.write(json.dumps(rec) + "\n")


def notif_call(conn, sender, path, iface, method, params, inv):
    if iface == "org.bwtest.Control" and method == "Invoke":
        nid, action = params.unpack()
        conn.emit_signal(None, "/org/freedesktop/Notifications", "org.freedesktop.Notifications",
                         "ActionInvoked", GLib.Variant("(us)", (nid, action)))
        conn.emit_signal(None, "/org/freedesktop/Notifications", "org.freedesktop.Notifications",
                         "NotificationClosed", GLib.Variant("(uu)", (nid, 2)))
        log("notifications.jsonl", {"event": "invoked", "id": nid, "action": action})
        inv.return_value(None)
        return
    if method == "GetCapabilities":
        inv.return_value(GLib.Variant("(as)", (["actions", "body", "body-markup"],)))
    elif method == "GetServerInformation":
        inv.return_value(GLib.Variant("(ssss)", ("bwtest-notifyd", "bwtest", "1", "1.2")))
    elif method == "Notify":
        app, replaces, icon, summary, body, actions, hints, timeout = params.unpack()
        nid = replaces or state["next"]
        state["next"] += 1
        log("notifications.jsonl", {"event": "notify", "id": nid, "app": app, "summary": summary,
                                    "body": body, "actions": actions, "sender": sender,
                                    "hints": sorted(hints.keys()), "timeout": timeout})
        inv.return_value(GLib.Variant("(u)", (nid,)))
    elif method == "CloseNotification":
        (nid,) = params.unpack()
        log("notifications.jsonl", {"event": "close", "id": nid})
        conn.emit_signal(None, "/org/freedesktop/Notifications", "org.freedesktop.Notifications",
                         "NotificationClosed", GLib.Variant("(uu)", (nid, 3)))
        inv.return_value(None)


def snw_call(conn, sender, path, iface, method, params, inv):
    (arg,) = params.unpack()
    if method == "RegisterStatusNotifierItem":
        # The argument is an object path on the caller's connection, or a bus name.
        if arg.startswith("/"):
            item = sender + arg
        elif "/" in arg:
            item = arg
        else:
            item = arg + "/StatusNotifierItem"
        state["items"].append({"service": item, "sender": sender})
        log("sni.jsonl", {"event": "register", "arg": arg, "sender": sender, "item": item})
        conn.emit_signal(None, "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher",
                         "StatusNotifierItemRegistered", GLib.Variant("(s)", (item,)))
    inv.return_value(None)


def snw_get(conn, sender, path, iface, prop):
    if prop == "RegisteredStatusNotifierItems":
        return GLib.Variant("as", [i["service"] for i in state["items"]])
    if prop == "IsStatusNotifierHostRegistered":
        return GLib.Variant("b", True)
    return GLib.Variant("i", 0)


def on_bus(conn, name):
    ni = Gio.DBusNodeInfo.new_for_xml(NOTIF_XML)
    for iface in ni.interfaces:
        conn.register_object("/org/freedesktop/Notifications", iface, notif_call, None, None)
    si = Gio.DBusNodeInfo.new_for_xml(SNW_XML)
    conn.register_object("/StatusNotifierWatcher", si.interfaces[0], snw_call, snw_get, None)


def on_name(conn, name):
    log("desktopd.jsonl", {"event": "owned", "name": name})


Gio.bus_own_name(Gio.BusType.SESSION, "org.freedesktop.Notifications", Gio.BusNameOwnerFlags.NONE,
                 on_bus, on_name, lambda c, n: log("desktopd.jsonl", {"event": "lost", "name": n}))
Gio.bus_own_name(Gio.BusType.SESSION, "org.kde.StatusNotifierWatcher", Gio.BusNameOwnerFlags.NONE,
                 None, on_name, lambda c, n: log("desktopd.jsonl", {"event": "lost", "name": n}))
GLib.MainLoop().run()

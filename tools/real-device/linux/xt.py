#!/usr/bin/env python3
"""Tiny X11 driver over ctypes (no xdotool here). Uses $DISPLAY.

xt.py list                       windows with name, geometry, map state
xt.py find <substr>              ids of mapped toplevels whose name contains substr
xt.py geom <id>                  x y w h of a window (root-relative)
xt.py move <x> <y>               pointer
xt.py click <x> <y> [button]
xt.py key <keysym> [...]         e.g. Tab, Return, ctrl+a
xt.py type <text>
xt.py focus <id>
xt.py resize <id> <w> <h>
xt.py moveresize <id> <x> <y> <w> <h>
"""
import os
import sys

if not os.environ.get("BWH_RUN"):
    sys.exit("run this through `harness.sh run`, never against your own session")

import ctypes
import ctypes.util
import time

x11 = ctypes.CDLL(ctypes.util.find_library("X11"))
xtst = ctypes.CDLL(ctypes.util.find_library("Xtst"))
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XDefaultRootWindow.restype = ctypes.c_ulong
x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
x11.XKeysymToKeycode.restype = ctypes.c_ubyte
x11.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
x11.XStringToKeysym.restype = ctypes.c_ulong
x11.XStringToKeysym.argtypes = [ctypes.c_char_p]
x11.XFlush.argtypes = [ctypes.c_void_p]
x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
xtst.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
x11.XQueryTree.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)), ctypes.POINTER(ctypes.c_uint)]
x11.XFetchName.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_char_p)]
x11.XSetInputFocus.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]
x11.XMoveResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint]
x11.XRaiseWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
x11.XTranslateCoordinates.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_ulong)]


class XWindowAttributes(ctypes.Structure):
    _fields_ = [("x", ctypes.c_int), ("y", ctypes.c_int), ("width", ctypes.c_int), ("height", ctypes.c_int),
                ("border_width", ctypes.c_int), ("depth", ctypes.c_int), ("visual", ctypes.c_void_p),
                ("root", ctypes.c_ulong), ("class", ctypes.c_int), ("bit_gravity", ctypes.c_int),
                ("win_gravity", ctypes.c_int), ("backing_store", ctypes.c_int), ("backing_planes", ctypes.c_ulong),
                ("backing_pixel", ctypes.c_ulong), ("save_under", ctypes.c_int), ("colormap", ctypes.c_ulong),
                ("map_installed", ctypes.c_int), ("map_state", ctypes.c_int), ("all_event_masks", ctypes.c_long),
                ("your_event_mask", ctypes.c_long), ("do_not_propagate_mask", ctypes.c_long),
                ("override_redirect", ctypes.c_int), ("screen", ctypes.c_void_p)]


x11.XGetWindowAttributes.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(XWindowAttributes)]

d = x11.XOpenDisplay(None)
if not d:
    sys.exit("cannot open display")
root = x11.XDefaultRootWindow(d)


def children(w):
    r, p = ctypes.c_ulong(), ctypes.c_ulong()
    ch = ctypes.POINTER(ctypes.c_ulong)()
    n = ctypes.c_uint()
    x11.XQueryTree(d, w, ctypes.byref(r), ctypes.byref(p), ctypes.byref(ch), ctypes.byref(n))
    return [ch[i] for i in range(n.value)]


def name(w):
    s = ctypes.c_char_p()
    if x11.XFetchName(d, w, ctypes.byref(s)) and s.value:
        return s.value.decode("utf-8", "replace")
    return ""


def attrs(w):
    a = XWindowAttributes()
    x11.XGetWindowAttributes(d, w, ctypes.byref(a))
    return a


def geom(w):
    a = attrs(w)
    rx, ry, c = ctypes.c_int(), ctypes.c_int(), ctypes.c_ulong()
    x11.XTranslateCoordinates(d, w, root, 0, 0, ctypes.byref(rx), ctypes.byref(ry), ctypes.byref(c))
    return rx.value, ry.value, a.width, a.height, a.map_state


def keycode(sym):
    ks = x11.XStringToKeysym(sym.encode())
    if not ks:
        sys.exit(f"unknown keysym {sym}")
    return x11.XKeysymToKeycode(d, ks)


MODS = {"ctrl": "Control_L", "shift": "Shift_L", "alt": "Alt_L"}


def key(combo):
    parts = combo.split("+")
    mods = [keycode(MODS[p]) for p in parts[:-1]]
    k = keycode(parts[-1])
    for m in mods:
        xtst.XTestFakeKeyEvent(d, m, 1, 0)
    xtst.XTestFakeKeyEvent(d, k, 1, 0)
    xtst.XTestFakeKeyEvent(d, k, 0, 0)
    for m in reversed(mods):
        xtst.XTestFakeKeyEvent(d, m, 0, 0)
    x11.XSync(d, 0)
    time.sleep(0.05)


def type_text(text):
    for ch in text:
        sym = {" ": "space", ":": "colon", "/": "slash", ".": "period", "-": "minus", "_": "underscore",
               "=": "equal", '"': "quotedbl", ",": "comma"}.get(ch, ch)
        if ch.isupper() or ch in ':_"':
            key("shift+" + (sym.lower() if ch.isupper() else {":": "semicolon", "_": "minus", '"': "apostrophe"}[ch]))
        else:
            key(sym)


cmd, args = sys.argv[1], sys.argv[2:]
if cmd == "list":
    for w in children(root):
        x, y, wd, h, m = geom(w)
        print(f"0x{w:x}\t{x},{y}\t{wd}x{h}\tmap={m}\t{name(w)!r}")
elif cmd == "find":
    for w in children(root):
        x, y, wd, h, m = geom(w)
        if m == 2 and args[0] in name(w):
            print(f"0x{w:x}")
elif cmd == "geom":
    print(*geom(int(args[0], 0))[:4])
elif cmd == "move":
    xtst.XTestFakeMotionEvent(d, -1, int(args[0]), int(args[1]), 0)
    x11.XSync(d, 0)
elif cmd == "click":
    b = int(args[2]) if len(args) > 2 else 1
    xtst.XTestFakeMotionEvent(d, -1, int(args[0]), int(args[1]), 0)
    x11.XSync(d, 0)
    time.sleep(0.1)
    xtst.XTestFakeButtonEvent(d, b, 1, 0)
    x11.XSync(d, 0)
    time.sleep(0.05)
    xtst.XTestFakeButtonEvent(d, b, 0, 0)
    x11.XSync(d, 0)
elif cmd == "key":
    for a in args:
        key(a)
elif cmd == "type":
    type_text(args[0])
elif cmd == "focus":
    w = int(args[0], 0)
    x11.XRaiseWindow(d, w)
    x11.XSetInputFocus(d, w, 1, 0)
    x11.XSync(d, 0)
elif cmd == "resize":
    x11.XResizeWindow(d, int(args[0], 0), int(args[1]), int(args[2]))
    x11.XSync(d, 0)
elif cmd == "moveresize":
    x11.XMoveResizeWindow(d, int(args[0], 0), int(args[1]), int(args[2]), int(args[3]), int(args[4]))
    x11.XSync(d, 0)
else:
    sys.exit(__doc__)

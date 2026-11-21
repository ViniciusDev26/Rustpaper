#!/usr/bin/env python3
"""xpointer.py <window title>

Prints the cursor position Wallpaper Engine sees, as "x,y" in 0..1 inside
the window with that title (clamped like WE does), or exits 1.
Do NOT move it with XWarpPointer: that changes what XQueryPointer reports
(the user's cursor stays put) but WE ignores warps (two captures warped to
0.5,0.5 and 0.1,0.2 were identical), so the reading would no longer be what
WE sees. Only real cursor motion over X windows reaches WE.

WE runs under Wine on XWayland and reads the X pointer, which is NOT the
live Wayland cursor: XWayland only learns the position while the cursor is
over an X window, so it keeps the last one (2026-11-09: X said 2003,39 while
Hyprland's cursor was at 3536,696). X coordinates are not Hyprland's layout
either (the WE window Hyprland put at 4480,0 is at 1920,0 for X), so the
window geometry is read from X too. Pure ctypes on libX11, no extra packages.
"""
import ctypes
import ctypes.util
import os
import sys

title = sys.argv[1].encode()
X = ctypes.cdll.LoadLibrary(ctypes.util.find_library("X11") or "libX11.so.6")
X.XOpenDisplay.restype = ctypes.c_void_p
X.XOpenDisplay.argtypes = [ctypes.c_char_p]
X.XDefaultRootWindow.restype = ctypes.c_ulong
X.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
X.XQueryPointer.argtypes = [ctypes.c_void_p, ctypes.c_ulong] + [ctypes.c_void_p] * 7
X.XQueryTree.argtypes = [ctypes.c_void_p, ctypes.c_ulong] + [ctypes.c_void_p] * 4
X.XFetchName.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_void_p]
X.XFree.argtypes = [ctypes.c_void_p]
X.XTranslateCoordinates.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong,
                                    ctypes.c_int, ctypes.c_int] + [ctypes.c_void_p] * 3


class Attrs(ctypes.Structure):  # XWindowAttributes, only the leading fields used
    _fields_ = [("x", ctypes.c_int), ("y", ctypes.c_int), ("width", ctypes.c_int),
                ("height", ctypes.c_int), ("pad", ctypes.c_byte * 256)]


X.XGetWindowAttributes.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(Attrs)]

d = X.XOpenDisplay(os.environ.get("DISPLAY", ":0").encode())
if not d:
    sys.exit(1)
root = X.XDefaultRootWindow(d)


def find(w, depth=0):
    name = ctypes.c_char_p()
    if X.XFetchName(d, w, ctypes.byref(name)) and name.value:
        hit = name.value == title
        X.XFree(name)
        if hit:
            return w
    if depth > 6:
        return None
    r, p, kids, n = ctypes.c_ulong(), ctypes.c_ulong(), ctypes.POINTER(ctypes.c_ulong)(), ctypes.c_uint()
    if not X.XQueryTree(d, w, ctypes.byref(r), ctypes.byref(p), ctypes.byref(kids), ctypes.byref(n)):
        return None
    found = None
    for i in range(n.value):
        found = find(kids[i], depth + 1)
        if found:
            break
    if kids:
        X.XFree(kids)
    return found


win = find(root)
if not win:
    sys.exit(1)
a = Attrs()
X.XGetWindowAttributes(d, win, ctypes.byref(a))
ox, oy, child = ctypes.c_int(), ctypes.c_int(), ctypes.c_ulong()
X.XTranslateCoordinates(d, win, root, 0, 0, ctypes.byref(ox), ctypes.byref(oy), ctypes.byref(child))
r, c = ctypes.c_ulong(), ctypes.c_ulong()
px, py, wx, wy, m = (ctypes.c_int() for _ in range(5))
X.XQueryPointer(d, root, ctypes.byref(r), ctypes.byref(c), ctypes.byref(px), ctypes.byref(py),
                ctypes.byref(wx), ctypes.byref(wy), ctypes.byref(m))
clamp = lambda v: min(max(v, 0.0), 1.0)
print(f"{clamp((px.value - ox.value) / max(a.width, 1)):.3f},{clamp((py.value - oy.value) / max(a.height, 1)):.3f}")

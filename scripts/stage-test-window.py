#!/usr/bin/env python3
"""stage-test-window.py <title> <width> <height> <seconds>

An X11 window that cycles solid colors and moves a white square, with
Wallpaper Engine's window class (steam_app_431960). Stands in for WE when
testing a stage without it:

    RUSTPAPER_NO_WE=1 rustpaper <any wallpaper dir> --stage private
    DISPLAY=<the stage's display> scripts/stage-test-window.py rustpaper-DP-1 1920 1080 60

The title must be the stage's window name for a monitor, `rustpaper-<output>`.
Pure ctypes on libX11, no extra packages.
"""
import ctypes, ctypes.util, sys, time
title, w, h, secs = sys.argv[1].encode(), int(sys.argv[2]), int(sys.argv[3]), float(sys.argv[4])
X = ctypes.cdll.LoadLibrary(ctypes.util.find_library("X11"))
X.XOpenDisplay.restype = ctypes.c_void_p
X.XOpenDisplay.argtypes = [ctypes.c_char_p]
X.XDefaultRootWindow.restype = ctypes.c_ulong
X.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
X.XCreateSimpleWindow.restype = ctypes.c_ulong
X.XCreateSimpleWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_ulong]
X.XStoreName.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_char_p]
X.XMapWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
X.XCreateGC.restype = ctypes.c_void_p
X.XCreateGC.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_void_p]
X.XSetForeground.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulong]
X.XFillRectangle.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint]
X.XFlush.argtypes = [ctypes.c_void_p]
X.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
d = X.XOpenDisplay(None)
if not d:
    sys.exit("cannot open display")
win = X.XCreateSimpleWindow(d, X.XDefaultRootWindow(d), 0, 0, w, h, 0, 0, 0x202020)
X.XStoreName(d, win, title)
class ClassHint(ctypes.Structure):
    _fields_ = [("res_name", ctypes.c_char_p), ("res_class", ctypes.c_char_p)]
X.XSetClassHint.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ClassHint)]
hint = ClassHint(b"steam_app_431960", b"steam_app_431960")  # like WE under Proton
X.XSetClassHint(d, win, ctypes.byref(hint))
X.XMapWindow(d, win)
gc = X.XCreateGC(d, win, 0, None)
X.XSync(d, 0)
colors = [0xd03030, 0x30a040, 0x3060d0]
end, i = time.time() + secs, 0
while time.time() < end:
    X.XSetForeground(d, gc, colors[i % 3])
    X.XFillRectangle(d, win, gc, 0, 0, w, h)
    X.XSetForeground(d, gc, 0xffffff)
    X.XFillRectangle(d, win, gc, (i * 40) % w, h // 2 - 50, 100, 100)
    X.XFlush(d)
    i += 1
    time.sleep(0.5)

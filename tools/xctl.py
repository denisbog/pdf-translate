import ctypes
import sys
import time

x11 = ctypes.CDLL("libX11.so.6")
xtst = ctypes.CDLL("libXtst.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XWarpPointer.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint, ctypes.c_int, ctypes.c_int]
x11.XFlush.argtypes = [ctypes.c_void_p]
xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]

display = x11.XOpenDisplay(b":99")
if not display:
    sys.exit("no display")
x11.XDefaultRootWindow.restype = ctypes.c_ulong
x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
root = x11.XDefaultRootWindow(display)


def move(x, y):
    x11.XWarpPointer(display, 0, root, 0, 0, 0, 0, int(x), int(y))
    x11.XFlush(display)


def click(button=1):
    xtst.XTestFakeButtonEvent(display, button, 1, 0)
    xtst.XTestFakeButtonEvent(display, button, 0, 0)
    x11.XFlush(display)


def wheel(button, times):
    for _ in range(times):
        xtst.XTestFakeButtonEvent(display, button, 1, 0)
        xtst.XTestFakeButtonEvent(display, button, 0, 0)
        x11.XFlush(display)
        time.sleep(0.02)


if __name__ == "__main__":
    commands = sys.argv[1:]
    for command in commands:
        name, _, value = command.partition(":")
        if name == "move":
            x, y = value.split(",")
            move(x, y)
        elif name == "click":
            click(int(value or 1))
        elif name == "down":
            wheel(5, int(value))
        elif name == "up":
            wheel(4, int(value))
        elif name == "sleep":
            time.sleep(float(value))
        print(f"did {command}")

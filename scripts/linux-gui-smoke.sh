#!/usr/bin/env bash
# Exercise the real X11 window lifetime using only synthetic image bytes.
set -euo pipefail

if [[ $# != 1 ]]; then
    echo "Usage: scripts/linux-gui-smoke.sh <viewr-binary>" >&2
    exit 2
fi
binary=$(realpath "$1")
workspace=$(mktemp -d -t viewr-x11-lifetime-XXXXXXXX)
trap 'rm -rf -- "$workspace"' EXIT

# A locally generated 200x120 solid-color JPEG, with no embedded rating.
python3 - "$workspace/rating.jpg" <<'PY'
import base64
import pathlib
import sys

pathlib.Path(sys.argv[1]).write_bytes(base64.b64decode(
    '/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/2wBDAQkJCQwLDBgNDRgyIRwhMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjL/wAARCAB4AMgDASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwDhaKKK7zywooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooA//Z'
))
PY
cp "$workspace/rating.jpg" "$workspace/original.jpg"

export WINIT_UNIX_BACKEND=x11
export LIBGL_ALWAYS_SOFTWARE=1
export WGPU_BACKEND=gl
export XDG_CONFIG_HOME="$workspace/config"
export XDG_CACHE_HOME="$workspace/cache"
mkdir -p "$XDG_CONFIG_HOME/viewr"
printf 'dark' > "$XDG_CONFIG_HOME/viewr/appearance"
unset WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS

for close in shortcut destroyed; do
    xvfb-run -a -s '-screen 0 1280x800x24 -nolisten tcp' \
        bash -s -- "$binary" "$workspace" "$close" <<'SH'
set -euo pipefail
binary=$1
workspace=$2
close=$3
"$binary" "$workspace/rating.jpg" > "$workspace/stdout" 2> "$workspace/stderr" &
viewer=$!
cleanup() {
    if kill -0 "$viewer" 2>/dev/null; then
        kill "$viewer"
        wait "$viewer" || true
    fi
}
trap cleanup EXIT
window=$(timeout 20 xdotool search --sync --onlyvisible --pid "$viewer" | head -1)
xdotool windowfocus --sync "$window"
python3 - "$window" <<'PY'
import ctypes
import subprocess
import sys
import time

x11 = ctypes.CDLL('libX11.so.6')
pointer, integer, unsigned, long = ctypes.c_void_p, ctypes.c_int, ctypes.c_uint, ctypes.c_ulong
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XOpenDisplay.restype = pointer
x11.XGetGeometry.argtypes = [pointer, long, ctypes.POINTER(long), ctypes.POINTER(integer), ctypes.POINTER(integer), ctypes.POINTER(unsigned), ctypes.POINTER(unsigned), ctypes.POINTER(unsigned), ctypes.POINTER(unsigned)]
x11.XGetImage.argtypes = [pointer, long, integer, integer, unsigned, unsigned, long, integer]
x11.XGetImage.restype = pointer
x11.XGetPixel.argtypes = [pointer, integer, integer]
x11.XGetPixel.restype = long
x11.XDestroyImage.argtypes = [pointer]
x11.XCloseDisplay.argtypes = [pointer]
display = x11.XOpenDisplay(None)
if not display:
    raise RuntimeError('could not connect to the private X display')
window = int(sys.argv[1])
root, x, y = long(), integer(), integer()
width, height, border, depth = unsigned(), unsigned(), unsigned(), unsigned()
if not x11.XGetGeometry(display, window, ctypes.byref(root), ctypes.byref(x), ctypes.byref(y), ctypes.byref(width), ctypes.byref(height), ctypes.byref(border), ctypes.byref(depth)):
    raise RuntimeError('could not read the viewer geometry')

def pixel():
    image = x11.XGetImage(display, window, width.value // 2, height.value // 2, 1, 1, long(-1).value, 2)
    if not image:
        raise RuntimeError('could not read the presented center pixel')
    try:
        value = x11.XGetPixel(image, 0, 0)
        return ((value >> 16) & 255, (value >> 8) & 255, value & 255)
    finally:
        x11.XDestroyImage(image)

def image_presented():
    red, green, blue = pixel()
    return 35 <= red <= 50 and 75 <= green <= 90 and 90 <= blue <= 110

def wait_for(predicate, failure):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise RuntimeError(failure)

try:
    wait_for(image_presented, 'synthetic JPEG was not presented')
    # A rating toast cannot cover this center pixel. The app-owned disclosure
    # must replace the presented image here before a close is exercised. Rating
    # observation can settle after the first pixels, so retry the request while
    # it is still blocked. An opened disclosure owns all later digit input.
    deadline = time.monotonic() + 20
    while image_presented():
        if time.monotonic() >= deadline:
            raise RuntimeError('rating disclosure did not appear')
        subprocess.run(['xdotool', 'key', '4'], check=True)
        time.sleep(0.15)
finally:
    x11.XCloseDisplay(display)
PY
if [[ "$close" == shortcut ]]; then
    xdotool key ctrl+q
else
    # This destroys the X11 drawable, rather than sending WM_DELETE_WINDOW.
    # egui must never get another frame against that invalid native handle.
    xdotool windowclose "$window"
fi
for _ in {1..100}; do
    if ! kill -0 "$viewer" 2>/dev/null; then
        break
    fi
    sleep 0.1
done
if kill -0 "$viewer" 2>/dev/null; then
    echo "viewr did not exit after $close" >&2
    exit 1
fi
if ! wait "$viewer"; then
    cat "$workspace/stderr" >&2
    exit 1
fi
cmp "$workspace/rating.jpg" "$workspace/original.jpg"
if [[ -s "$workspace/stderr" ]]; then
    cat "$workspace/stderr" >&2
    exit 1
fi
echo "X11 $close: clean exit, unconfirmed JPEG unchanged"
SH
done

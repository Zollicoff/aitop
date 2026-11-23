#!/usr/bin/env python3
"""Make the README's screenshots (docs/*.png) from demo data.

    python3 scripts/make-demo-home.py /tmp/aitop-demo
    cargo build --release
    python3 scripts/make-screenshots.py

aitop runs in a pseudo-terminal against the demo home, so nothing real is
shown; the terminal's cells are kept with pyte and drawn with Pillow
(`pip install pyte pillow`). The font is macOS's Menlo; set FONT for another.
"""
import fcntl
import os
import pty
import select
import signal
import struct
import termios
import time

import pyte
from PIL import Image, ImageDraw, ImageFont

signal.alarm(150)  # give up rather than hang

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEMO = '/tmp/aitop-demo'
COLS, ROWS = 150, 44
FONT = os.environ.get('FONT', '/System/Library/Fonts/Menlo.ttc')
fonts = {False: ImageFont.truetype(FONT, 24, index=0), True: ImageFont.truetype(FONT, 24, index=1)}
W, H, MARGIN = 14, 29, 8
PALETTE = {'default': '#101116', 'black': '#000000', 'white': '#d8dae2'}


def color(s):
    return PALETTE.get(s, '#' + s)


def render(screen, name):
    im = Image.new('RGB', (COLS * W + 2 * MARGIN, ROWS * H + 2 * MARGIN), color('default'))
    for y in range(ROWS):
        for x in range(COLS):
            c = screen.buffer[y][x]
            fg, bg = color(c.fg), color(c.bg)
            if c.reverse:
                fg, bg = bg, fg
            cell = Image.new('RGB', (W, H), bg)
            d = ImageDraw.Draw(cell)
            if len(c.data) == 1 and 0x2581 <= ord(c.data) <= 0x2588:
                n = ord(c.data) - 0x2580
                d.rectangle((0, round(H * (8 - n) / 8), W - 1, H - 1), fill=fg)
            else:
                d.text((0, 0), c.data, font=fonts[c.bold], fill=fg)
            if c.underscore:
                d.line((0, H - 2, W, H - 2), fill=fg)
            im.paste(cell, (MARGIN + x * W, MARGIN + y * H))
    path = f'{ROOT}/docs/{name}.png'
    im.save(path, optimize=True)
    print(name, im.size, os.path.getsize(path), flush=True)
    return '\n'.join(screen.display)


def session(args, steps):
    """Run aitop with `args`; `steps` are (keys, seconds, name-or-None)."""
    screen = pyte.Screen(COLS, ROWS)
    stream = pyte.ByteStream(screen)
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(DEMO)
        # Nothing of the real machine: its home, its T3 Code, its environment.
        os.execv('/usr/bin/env', ['env', '-i', f'HOME={DEMO}', f'T3CODE_HOME={DEMO}/.t3', 'PATH=/usr/bin:/bin',
                                  'TERM=xterm-256color', 'COLORTERM=truecolor', ROOT + '/target/release/aitop', *args])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', ROWS, COLS, 0, 0))
    os.set_blocking(fd, False)

    def read_for(seconds):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            ready, _, _ = select.select([fd], [], [], min(.1, max(0, end - time.monotonic())))
            if ready:
                try:
                    data = os.read(fd, 65536)
                except BlockingIOError:
                    continue
                except OSError:
                    break
                if not data:
                    break
                stream.feed(data)
    texts = {}
    try:
        for keys, seconds, name in steps:
            if os.waitpid(pid, os.WNOHANG) != (0, 0):
                raise SystemExit(f'aitop exited early, before {name or keys!r}')
            if keys:
                os.write(fd, keys)
            read_for(seconds)
            if name:
                texts[name] = render(screen, name)
    finally:
        # Close our end first: a program exiting with unread output on its
        # terminal waits for it to be read, and would never be reaped.
        os.close(fd)
        try:
            if os.waitpid(pid, os.WNOHANG) == (0, 0):
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
        except ChildProcessError:
            pass
    return texts


session(['--range', 'all'], [
    (b'', 4, 'screenshot'),            # list layout, Claude Code selected
    (b'v', 4, 'stacked'),              # stacked layout (the footer note clears in 3 s)
    (b'v', 3.5, None),                 # back to the list
    (b'M', .4, None), (b'a', 1.5, 'machines'),   # Machines tab, activity feed open
    (b'a', .3, None), (b'p', .3, None),
    (b'o', .4, None), (b'\t\t', 1.5, 'options'),  # Options on the Agents section
])
session(['--range', 'all', '--theme', 'tokyo-night'], [
    (b'', 4, None), (b'\t', .5, None), (b'm', 1.5, 'theme'),   # another theme: Codex, its Models tab
])

#!/usr/bin/env python3
"""Run the decisions binary in a pseudo-terminal (100x30), type steps, wait for it to exit.

usage: drive.py BIN STEP...   (DRIVE_ARGS: space-separated arguments for BIN)
A STEP is text to type (one key every 0.15 s), `sleep:SECONDS`, or `signal:NAME` (TERM, HUP).
Prints the exit code, or `timeout` (and exits 1) when the app is still running 15 s later.
"""
import fcntl
import os
import pty
import select
import signal
import struct
import sys
import termios
import time


def drain(fd, secs):
    end = time.time() + secs
    while (left := end - time.time()) > 0:
        ready, _, _ = select.select([fd], [], [], left)
        if ready:
            try:
                os.read(fd, 65536)
            except OSError:
                return


def main():
    binary, steps = sys.argv[1], sys.argv[2:]
    pid, fd = pty.fork()
    if pid == 0:
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
        os.execv(binary, [binary, *os.environ.get("DRIVE_ARGS", "").split()])
    drain(fd, 1.5)
    for step in steps:
        if step.startswith("sleep:"):
            drain(fd, float(step[6:]))
        elif step.startswith("signal:"):
            os.kill(pid, getattr(signal, "SIG" + step[7:]))
            drain(fd, 0.3)
        else:
            for ch in step:
                os.write(fd, ch.encode())
                drain(fd, 0.15)
    deadline = time.time() + 15
    while time.time() < deadline:
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            print(os.waitstatus_to_exitcode(status))
            return
        drain(fd, 0.1)
    os.kill(pid, signal.SIGKILL)
    print("timeout")
    sys.exit(1)


main()

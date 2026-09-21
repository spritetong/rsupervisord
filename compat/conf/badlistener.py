#!/usr/bin/env python3
"""Deliberately misbehaving event listener for the compat harness.

It never emits the ``READY`` handshake; it writes junk to stdout instead.  A
stock supervisord must detect the protocol violation, move the listener to the
``UNKNOWN`` listener state and stop delivering events to it
(``compat/docs/EVENTLISTENER_COMPAT.md`` requirement EL-7).

Usage:: listener.py
"""

from __future__ import annotations

import sys
import time


def main() -> None:
    sys.stdout.write("this-is-not-a-ready-token\n")
    sys.stdout.flush()
    while True:
        time.sleep(1)


if __name__ == "__main__":
    main()

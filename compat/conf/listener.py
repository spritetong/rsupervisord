#!/usr/bin/env python3
"""Minimal, protocol-correct supervisord event listener.

Implements the supervisor 3.0 length-prefixed event protocol (READY / RESULT)
so the ``[eventlistener:listener]`` section in the harness config consumes
events instead of overflowing the pool buffer.
"""

from __future__ import annotations

import sys


def _write(s: str) -> None:
    sys.stdout.write(s)
    sys.stdout.flush()


def main() -> None:
    while True:
        _write("READY\n")
        header_line = sys.stdin.readline()
        if not header_line:
            return
        headers = dict(
            item.split(":", 1) for item in header_line.split() if ":" in item
        )
        try:
            payload_len = int(headers.get("len", "0"))
        except ValueError:
            payload_len = 0
        if payload_len:
            sys.stdin.read(payload_len)
        _write("RESULT 2\nOK")


if __name__ == "__main__":
    main()

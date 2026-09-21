#!/usr/bin/env python3
"""Protocol-correct supervisord event listener used by the compat harness.

Speaks the supervisor 3.0 length-prefixed event protocol (READY / RESULT) so
the ``[eventlistener:listener]`` pool consumes events instead of overflowing
its buffer.  Every envelope it receives is appended -- one JSON object per
line -- to the path given as ``argv[1]`` (default ``events.log`` next to this
script) so that ``compat/tests/test_eventlistener.py`` can assert on exactly
what the pool delivered.

Usage::

    listener.py [event-log-path]
"""

from __future__ import annotations

import json
import os
import sys


def _read_exactly(stream, count: int) -> bytes:
    chunks = []
    remaining = count
    while remaining > 0:
        chunk = stream.read(remaining)
        if not chunk:
            break
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def _log_event(path: str, seq: int, header: str, payload: str) -> None:
    record = {"seq": seq, "header": header, "payload": payload}
    for item in header.split():
        if ":" in item:
            key, value = item.split(":", 1)
            record.setdefault(key, value)
    with open(path, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(record) + "\n")


def main() -> None:
    log_path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "events.log"
    )
    stdin = sys.stdin.buffer
    stdout = sys.stdout.buffer
    seq = 0

    while True:
        stdout.write(b"READY\n")
        stdout.flush()

        header_line = stdin.readline()
        if not header_line:
            return

        header = header_line.decode("utf-8", "replace").rstrip("\n")
        headers = {
            key: value
            for item in header.split()
            if ":" in item
            for key, value in (item.split(":", 1),)
        }
        try:
            payload_len = int(headers.get("len", "0"))
        except ValueError:
            payload_len = 0

        payload = _read_exactly(stdin, payload_len).decode("utf-8", "replace")
        seq += 1
        _log_event(log_path, seq, header, payload)

        stdout.write(b"RESULT 2\nOK")
        stdout.flush()


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Reads armo_proto's golden files the way a Python client would and prints what it read.

    read_golden.py ARMO_PROTO_DIR

Each JSON golden must pass schema/v1.json; it is printed as sorted compact JSON after its path.
golden/frames/hello.bin is a JSON frame; golden/frames/events.bin an event batch, printed one
positioned record per line. tests/golden.rs compares this output with what Rust reads.
"""

import json
import os
import struct
import sys

import jsonschema

DIRS = [
    ("client", "ClientRequest"),
    ("server", "ServerMessage"),
    ("armod_request", "ArmodRequest"),
    ("armod", "ArmodMessage"),
    ("window_line", "WindowLine"),
    ("meta", "RecordMeta"),
]
MAX_LENGTH = 16 << 20


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def frame(data):
    length, kind = struct.unpack_from("<IB", data)
    if not 1 <= length <= MAX_LENGTH or kind not in (1, 2) or len(data) != 4 + length:
        raise ValueError(f"bad frame: length {length} kind {kind} size {len(data)}")
    return kind, data[5:]


def records(batch):
    at = 0
    while at < len(batch):
        pid, tid, event, size, flags, t = struct.unpack_from("<IIHHIQ", batch, at)
        if size < 16 or size % 8 or at + 8 + size > len(batch):
            raise ValueError(f"bad record at {at}")
        body = batch[at + 24 : at + 8 + size].hex()
        yield f"{pid} {tid} {event} {size} {flags} {t} {body}"
        at += 8 + size


def main(root):
    with open(os.path.join(root, "schema", "v1.json")) as f:
        schema = json.load(f)
    for sub, name in DIRS:
        validator = jsonschema.Draft202012Validator(dict(schema, **{"$ref": f"#/$defs/{name}"}))
        for file in sorted(os.listdir(os.path.join(root, "golden", sub))):
            with open(os.path.join(root, "golden", sub, file)) as f:
                value = json.load(f)
            validator.validate(value)
            print(f"{sub}/{file}\t{canonical(value)}")
    with open(os.path.join(root, "golden", "frames", "hello.bin"), "rb") as f:
        kind, payload = frame(f.read())
    assert kind == 1
    print(f"frames/hello.bin\t{canonical(json.loads(payload))}")
    with open(os.path.join(root, "golden", "frames", "events.bin"), "rb") as f:
        kind, payload = frame(f.read())
    assert kind == 2
    for line in records(payload):
        print(line)


if __name__ == "__main__":
    main(sys.argv[1])

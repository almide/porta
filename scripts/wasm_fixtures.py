"""Small WASI attack probes assembled directly, independent of guest compiler APIs."""
import json
import struct


def uleb(value):
    result = bytearray()
    while True:
        byte = value & 127
        value >>= 7
        result.append(byte | (128 if value else 0))
        if not value:
            return bytes(result)


def string(value):
    encoded = value.encode()
    return uleb(len(encoded)) + encoded


def section(kind, payload):
    return bytes([kind]) + uleb(len(payload)) + payload


def no_environment_guest(message=None):
    # Trap if WASI exposes even one environment variable, otherwise emit done.
    output = json.dumps(message if message is not None else {'action': 'done', 'output': 'no-env'}).encode()
    types = b'\x03\x60\x02\x7f\x7f\x01\x7f\x60\x04\x7f\x7f\x7f\x7f\x01\x7f\x60\x00\x00'
    imports = b'\x02' + string('wasi_snapshot_preview1') + string('environ_sizes_get') + b'\x00\x00'
    imports += string('wasi_snapshot_preview1') + string('fd_write') + b'\x00\x01'
    exports = b'\x02' + string('memory') + b'\x02\x00' + string('_start') + b'\x00\x02'
    body = bytes.fromhex('00 4100 4104 1000 1a 4100 280200 0440 00 0b 4101 4120 4101 4108 1001 1a 0b')
    data = struct.pack('<II', 64, len(output)) + bytes(24) + output
    return (b'\x00asm\x01\x00\x00\x00' + section(1, types) + section(2, imports)
            + section(3, b'\x01\x02') + section(5, b'\x01\x00\x01') + section(7, exports)
            + section(10, b'\x01' + uleb(len(body)) + body)
            + section(11, b'\x01\x00\x41\x20\x0b' + uleb(len(data)) + data))


def fixed_stdout_guest(message):
    """A real WASI guest that emits a chosen JSON value, without reading input."""
    output = json.dumps(message).encode()
    types = b'\x02\x60\x04\x7f\x7f\x7f\x7f\x01\x7f\x60\x00\x00'
    imports = b'\x01' + string('wasi_snapshot_preview1') + string('fd_write') + b'\x00\x00'
    exports = b'\x02' + string('memory') + b'\x02\x00' + string('_start') + b'\x00\x01'
    body = bytes.fromhex('00 4101 4120 4101 4108 1000 1a 0b')
    data = struct.pack('<II', 64, len(output)) + bytes(24) + output
    return (b'\x00asm\x01\x00\x00\x00' + section(1, types) + section(2, imports)
            + section(3, b'\x01\x01') + section(5, b'\x01\x00\x01') + section(7, exports)
            + section(10, b'\x01' + uleb(len(body)) + body)
            + section(11, b'\x01\x00\x41\x20\x0b' + uleb(len(data)) + data))

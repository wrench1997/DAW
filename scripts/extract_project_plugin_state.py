"""Extract a channel's VST3 state without loading a plug-in or changing a project.

Current Citrus projects store PluginInstance.opaque_state as strict hexadecimal
(src/model.rs, opaque_state_hex), not base64. This diagnostic intentionally
requires the versioned VST3HOST_STATE envelope from vst3-host's plugin.rs; it
does not accept the host's legacy raw-component fallback or interpret vendor
payloads. A valid envelope is not evidence that NOIRE or audio has restored.

Example (channel IDs are stable; channel indices are explicitly zero-based):
    python scripts/extract_project_plugin_state.py project.citrus --channel-id 1 --output state.bin

Both state.bin and its default state.json receipt must be new files. Their
parent directories must already exist. --receipt can select a different JSON
receipt path. Only Python's standard library is required.
"""

import argparse
from contextlib import ExitStack
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import sys


STATE_MAGIC = b"VST3HOST_STATE\0\0"
STATE_VERSION = 1
STATE_HEADER_BYTES = 28
NO_CONTROLLER_STATE = 0xFFFFFFFF
MAX_STATE_PAYLOAD_BYTES = 64 * 1024 * 1024
_HEX = re.compile(r"[0-9a-fA-F]+\Z", re.ASCII)


class ExtractionError(ValueError):
    """Invalid or ambiguous input; no usable extraction should be published."""


def decode_project_hex(encoded):
    """Match the project's string encoding; never guess another encoding."""
    if not isinstance(encoded, str):
        raise ExtractionError("opaque_state must be a hexadecimal string")
    if not encoded:
        raise ExtractionError("opaque_state is empty")
    if len(encoded) % 2:
        raise ExtractionError("opaque_state must contain an even number of hex digits")
    if len(encoded) > 2 * (STATE_HEADER_BYTES + MAX_STATE_PAYLOAD_BYTES):
        raise ExtractionError("opaque_state exceeds the host's 64 MiB payload limit")
    if _HEX.fullmatch(encoded) is None:
        raise ExtractionError("opaque_state contains invalid hex (base64 and whitespace are not accepted)")
    return bytes.fromhex(encoded)


def inspect_state_envelope(state):
    """Validate the documented host envelope, leaving both payloads opaque."""
    if not state.startswith(STATE_MAGIC):
        raise ExtractionError("state has no exact VST3HOST_STATE magic; legacy/raw state is not accepted")
    if len(state) < STATE_HEADER_BYTES:
        raise ExtractionError("truncated VST3HOST_STATE header")
    version, component_bytes, encoded_controller_bytes = struct.unpack_from("<III", state, 16)
    if version != STATE_VERSION:
        raise ExtractionError(f"unsupported VST3HOST_STATE version {version}")
    controller_bytes = (
        None if encoded_controller_bytes == NO_CONTROLLER_STATE else encoded_controller_bytes
    )
    payload_bytes = component_bytes + (controller_bytes or 0)
    if payload_bytes > MAX_STATE_PAYLOAD_BYTES:
        raise ExtractionError("VST3HOST_STATE combined payload exceeds the host's 64 MiB limit")
    expected_bytes = STATE_HEADER_BYTES + payload_bytes
    if len(state) != expected_bytes:
        raise ExtractionError(
            f"invalid VST3HOST_STATE size: header describes {expected_bytes} bytes, got {len(state)}"
        )
    return {
        "magic": "VST3HOST_STATE",
        "version": version,
        "header_bytes": STATE_HEADER_BYTES,
        "component_bytes": component_bytes,
        "controller_present": controller_bytes is not None,
        "controller_bytes": controller_bytes,
        "payload_bytes": payload_bytes,
        "total_bytes": expected_bytes,
    }


def _unique_json_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ExtractionError(f"duplicate JSON field {key!r}")
        result[key] = value
    return result


def _reject_json_constant(value):
    raise ExtractionError(f"invalid JSON numeric constant {value}")


def _unsigned_id(value, bits, label):
    if type(value) is not int or not 0 <= value < (1 << bits):
        raise ExtractionError(f"{label} must be an unsigned {bits}-bit integer")
    return value


def _object_list(project, key):
    values = project.get(key)
    if not isinstance(values, list) or any(not isinstance(value, dict) for value in values):
        raise ExtractionError(f"project {key} must be a list of objects")
    return values


def select_channel_plugin(project, *, channel_id=None, channel_index=None):
    """Resolve exactly one persisted channel and its top-level plug-in reference."""
    if (channel_id is None) == (channel_index is None):
        raise ExtractionError("choose exactly one of channel_id or channel_index")
    channels = _object_list(project, "channels")
    ids = [_unsigned_id(channel.get("id"), 32, "channel id") for channel in channels]
    if channel_id is not None:
        _unsigned_id(channel_id, 32, "selected channel id")
        matches = [index for index, value in enumerate(ids) if value == channel_id]
        if len(matches) != 1:
            raise ExtractionError(f"channel id {channel_id} must match exactly one channel (got {len(matches)})")
        index = matches[0]
        selection = {"method": "channel_id", "value": channel_id, "channel_index": index}
    else:
        if type(channel_index) is not int or not 0 <= channel_index < len(channels):
            raise ExtractionError("channel_index must be an in-range, zero-based integer")
        index = channel_index
        selection = {"method": "channel_index", "value": index, "channel_index": index}
    channel = channels[index]
    if ids.count(channel["id"]) != 1:
        raise ExtractionError(f"selected channel id {channel['id']} is duplicated")
    instance_id = _unsigned_id(
        channel.get("instrument_plugin_instance_id"), 64, "channel instrument_plugin_instance_id"
    )
    plugins = _object_list(project, "plugin_instances")
    matches = [
        plugin for plugin in plugins
        if _unsigned_id(plugin.get("id"), 64, "plug-in instance id") == instance_id
    ]
    if len(matches) != 1:
        raise ExtractionError(
            f"plug-in instance id {instance_id} must match exactly one instance (got {len(matches)})"
        )
    plugin = matches[0]
    if plugin.get("format") != "vst3":
        raise ExtractionError("selected channel's plug-in format must be vst3")
    for owner, fields in ((channel, ("name",)), (plugin, ("name", "path", "uid", "vendor", "role"))):
        for field in fields:
            if field in owner and not isinstance(owner[field], str):
                raise ExtractionError(f"selected channel/plug-in {field} must be a string when present")
    if "opaque_state" not in plugin:
        raise ExtractionError("selected plug-in has no opaque_state field")
    return channel, plugin, selection


def _write_new_pair(output_path, receipt_path, state, receipt_bytes):
    # Exclusive opens also reject symlinks (including dangling ones). Reserve
    # both outputs before writing either; clean up only files we created.
    created = []
    try:
        with ExitStack() as stack:
            for path in (output_path, receipt_path):
                stream = stack.enter_context(path.open("xb"))
                identity = os.fstat(stream.fileno())
                created.append((path, stream, (identity.st_dev, identity.st_ino)))
            for (_, stream, _), payload in zip(created, (state, receipt_bytes)):
                stream.write(payload)
                stream.flush()
                os.fsync(stream.fileno())
    except OSError:
        for path, _, identity in reversed(created):
            try:
                current = path.lstat()
                if (current.st_dev, current.st_ino) == identity:
                    path.unlink()
            except OSError:
                pass
        raise


def extract_state(project_path, *, channel_id=None, channel_index=None, output_path, receipt_path=None):
    """Validate a read-only project snapshot and create a new state/receipt pair."""
    project_path = Path(project_path).absolute()
    output_path = Path(output_path).absolute()
    receipt_path = Path(receipt_path).absolute() if receipt_path is not None else output_path.with_suffix(".json")
    if output_path.suffix.lower() != ".bin" or receipt_path.suffix.lower() != ".json":
        raise ExtractionError("output must end in .bin and receipt must end in .json")
    if output_path.resolve() == receipt_path.resolve():
        raise ExtractionError("state output and receipt paths must be different")
    for path in (output_path, receipt_path):
        if os.path.lexists(path):
            raise ExtractionError(f"refusing to overwrite existing output: {path}")
        if not path.parent.is_dir():
            raise ExtractionError(f"output parent directory does not exist: {path.parent}")

    project_bytes = project_path.read_bytes()
    try:
        project = json.loads(
            project_bytes.decode("utf-8-sig"),
            object_pairs_hook=_unique_json_object,
            parse_constant=_reject_json_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ExtractionError(f"project is not valid UTF-8 JSON: {error}") from error
    if not isinstance(project, dict):
        raise ExtractionError("project JSON root must be an object")
    channel, plugin, selection = select_channel_plugin(
        project, channel_id=channel_id, channel_index=channel_index
    )
    state = decode_project_hex(plugin["opaque_state"])
    envelope = inspect_state_envelope(state)
    receipt = {
        "receipt_schema_version": 1,
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "source_project": {
            "path": str(project_path.resolve()),
            "sha256": hashlib.sha256(project_bytes).hexdigest(),
            "bytes": len(project_bytes),
            "format_version": project.get("format_version"),
            "name": project.get("name"),
        },
        "selection": selection,
        "channel": {
            "id": channel["id"],
            "name": channel.get("name"),
            "instrument_plugin_instance_id": channel["instrument_plugin_instance_id"],
        },
        "plugin": {
            "id": plugin["id"],
            "name": plugin.get("name"),
            "format": plugin["format"],
            "role": plugin.get("role"),
            "vendor": plugin.get("vendor"),
            "path": plugin.get("path"),
            "class_uid": plugin.get("uid"),
        },
        "state": {
            "project_encoding": "hex",
            "output_path": str(output_path.resolve()),
            "bytes": len(state),
            "sha256": hashlib.sha256(state).hexdigest(),
        },
        "envelope": envelope,
        "validation_scope": (
            "Project hex encoding and host envelope only; opaque vendor payload, "
            "instrument restoration and audio output are not verified."
        ),
    }
    receipt_bytes = (json.dumps(receipt, ensure_ascii=False, indent=2, allow_nan=False) + "\n").encode("utf-8")
    _write_new_pair(output_path, receipt_path, state, receipt_bytes)
    return receipt


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("project", type=Path, help="read-only .citrus JSON project")
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument("--channel-id", type=int, help="exact stable channel ID")
    selection.add_argument("--channel-index", type=int, help="explicit zero-based channel index")
    parser.add_argument("--output", type=Path, required=True, help="new .bin file; never overwritten")
    parser.add_argument("--receipt", type=Path, help="new .json receipt (default: output with .json suffix)")
    args = parser.parse_args(argv)
    try:
        receipt = extract_state(
            args.project, channel_id=args.channel_id, channel_index=args.channel_index,
            output_path=args.output, receipt_path=args.receipt,
        )
    except (ExtractionError, OSError) as error:
        print(f"state extraction failed: {error}", file=sys.stderr)
        return 1
    # ASCII-safe output also works with Windows consoles whose encoding is GBK.
    print(json.dumps({"state": receipt["state"], "envelope": receipt["envelope"]}, ensure_ascii=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

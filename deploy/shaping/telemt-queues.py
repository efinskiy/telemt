#!/usr/bin/env python3
"""Explicit host-side queue lifecycle; never grants NET_ADMIN to the Telemt process."""
import argparse
import fcntl
import json
import os
import shutil
import sys
from pathlib import Path
from queue_manager import QueueManager, read_json


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["check", "apply", "status", "remove"])
    parser.add_argument("--config", default="/etc/telemt/queues.json")
    parser.add_argument("--state", default="/var/lib/telemt-queues/state.json")
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("Linux with iproute2 is required")
    if os.geteuid() != 0:
        parser.error("run as root; queue mutations require CAP_NET_ADMIN")
    tc = shutil.which("tc")
    if not tc:
        parser.error("tc is unavailable; install iproute2")
    state = Path(args.state)
    state.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with (state.parent / (state.name + ".lock")).open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        # Removal remains possible after an operator edits or removes the deployment config.
        config = read_json(state)["config"] if args.action == "remove" and state.exists() else read_json(args.config)
        manager = QueueManager(config, state, tc)
        result = getattr(manager, args.action)()
        print(json.dumps(result if result is not None else {"ok": True}, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, KeyError) as error:
        print(json.dumps({"ok": False, "error": str(error)}), file=sys.stderr)
        sys.exit(1)

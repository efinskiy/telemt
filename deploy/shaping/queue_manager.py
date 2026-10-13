"""Own a bounded HTB/fq_codel egress tree without replacing administrator policies."""
import json
import os
import re
import selectors
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = "7e10:"
LEAF = "7e11:"
CLASS = "7e10:1"
MAX_OUTPUT = 1024 * 1024
COMMAND_TIMEOUT = 5


def command(args):
    """Bound external command time and output before decoding any kernel JSON."""
    with subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as process:
        output = bytearray()
        deadline = time.monotonic() + COMMAND_TIMEOUT
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                while True:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise RuntimeError("queue command timed out")
                    if not selector.select(remaining):
                        raise RuntimeError("queue command timed out")
                    chunk = os.read(process.stdout.fileno(), 65536)
                    if not chunk:
                        break
                    if len(output) + len(chunk) > MAX_OUTPUT:
                        raise RuntimeError("queue command output exceeds 1 MiB")
                    output.extend(chunk)
            process.wait(timeout=max(0.001, deadline - time.monotonic()))
        except BaseException:
            process.kill()
            process.wait()
            raise
        if process.returncode:
            raise RuntimeError(f"queue command failed ({process.returncode}): {output.decode(errors='replace')[:512]}")
        return output.decode()


def read_json(path):
    with Path(path).open("rb") as stream:
        raw = stream.read(65537)
    if len(raw) > 65536:
        raise ValueError("configuration/state exceeds 64 KiB")
    return json.loads(raw)


def validate(config):
    expected = {"interface", "egress_bps", "target_ms", "interval_ms", "packet_limit", "memory_limit_bytes", "flows", "quantum", "burst_bytes"}
    if not isinstance(config, dict) or set(config) != expected:
        raise ValueError("configuration must contain exactly: " + ", ".join(sorted(expected)))
    if not isinstance(config["interface"], str) or not re.fullmatch(r"[a-zA-Z0-9_.:-]{1,15}", config["interface"]):
        raise ValueError("invalid Linux interface name")
    limits = {"egress_bps": (8000, 100_000_000_000), "target_ms": (1, 100), "interval_ms": (10, 1000),
              "packet_limit": (16, 65536), "memory_limit_bytes": (65536, 64 * 1024 * 1024),
              "flows": (16, 65536), "quantum": (256, 65536), "burst_bytes": (1600, 16 * 1024 * 1024)}
    for key, (lower, upper) in limits.items():
        if type(config[key]) is not int or not lower <= config[key] <= upper:
            raise ValueError(f"{key} must be an integer in [{lower}, {upper}]")
    if config["target_ms"] >= config["interval_ms"]:
        raise ValueError("target_ms must be smaller than interval_ms")
    return config


def save(path, value):
    path = Path(path)
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=path.parent, prefix=".queues-")
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(value, stream)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def structure(snapshot):
    """Ignore counters; retain queue parameters to detect external configuration changes."""
    result = {}
    for kind, rows in snapshot.items():
        result[kind] = []
        for row in rows:
            record = {key: value for key, value in row.items() if key not in ("refcnt", "direct_packets_stat")}
            if isinstance(record.get("options"), dict):
                record["options"] = {key: value for key, value in record["options"].items() if key != "direct_packets_stat"}
            result[kind].append(record)
        result[kind].sort(key=lambda row: json.dumps(row, sort_keys=True))
    return result


class QueueManager:
    """Manage one interface under an external process lock and a durable ownership record."""
    def __init__(self, config, state_path, tc="/usr/sbin/tc", run=command):
        self.config = validate(config)
        self.state_path = Path(state_path)
        self.tc = tc
        self.run = run

    def snapshot(self, interface=None):
        interface = interface or self.config["interface"]
        result = {}
        for kind in ("qdisc", "class", "filter"):
            rows = json.loads(self.run([self.tc, "-j", "-d", kind, "show", "dev", interface]))
            if not isinstance(rows, list) or len(rows) > 4096 or any(not isinstance(row, dict) for row in rows):
                raise RuntimeError("invalid or oversized queue inspection")
            result[kind] = rows
        if any(row.get("handle") == CLASS for row in result["class"]):
            rows = json.loads(self.run([self.tc, "-j", "-d", "filter", "show", "dev", interface, "parent", CLASS]))
            if not isinstance(rows, list) or len(rows) > 4096 or any(not isinstance(row, dict) for row in rows):
                raise RuntimeError("invalid or oversized class filter inspection")
            result["filter"].extend(rows)
        return result

    @staticmethod
    def pristine(snapshot):
        roots = [row for row in snapshot["qdisc"] if row.get("root")]
        defaults = {"noqueue", "pfifo_fast", "fq_codel", "fq", "mq"}
        return (len(roots) == 1 and not snapshot["filter"]
                and all(row.get("class") == "mq" for row in snapshot["class"])
                and all(row.get("handle") == "0:" and row.get("kind") in defaults for row in snapshot["qdisc"]))

    @staticmethod
    def owned_partial(snapshot):
        roots = [row for row in snapshot["qdisc"] if row.get("root")]
        return (len(roots) == 1 and roots[0].get("handle") == ROOT and roots[0].get("kind") == "htb"
                and not snapshot["filter"]
                and len(snapshot["qdisc"]) <= 2 and len(snapshot["class"]) <= 1
                and all(row == roots[0] or (row.get("parent") == CLASS and
                    ((row.get("handle") == LEAF and row.get("kind") == "fq_codel") or
                     (row.get("handle") == "0:" and row.get("kind") in ("pfifo", "fq_codel", "fq", "pfifo_fast")))) for row in snapshot["qdisc"])
                and all(row.get("handle") == CLASS and row.get("class") == "htb" and row.get("root") is True for row in snapshot["class"]))

    def state(self):
        return read_json(self.state_path) if self.state_path.exists() else None

    def check(self):
        current = self.snapshot()
        state = self.state()
        if state:
            # Persistent state survives an unclean reboot; kernel-default queues do not.
            if state["config"] == self.config and self.pristine(current):
                return "ready"
            if state["config"] != self.config or state["status"] != "active" or structure(current) != state.get("tree"):
                raise RuntimeError("owned queue configuration differs; inspect status and remove before applying changes")
            return "active"
        if not self.pristine(current):
            raise RuntimeError("foreign queue hierarchy present; refusing to replace it")
        return "ready"

    def apply(self):
        if self.check() == "active":
            return
        cfg = self.config
        interface = cfg["interface"]
        state = {"version": 1, "status": "installing", "config": cfg}
        save(self.state_path, state)
        installed_root = False
        try:
            # Exclusive creation rejects an administrator tree installed after inspection.
            # Only a verified kernel-default tree may be replaced; no unconditional root deletion.
            self.run([self.tc, "qdisc", "add", "dev", interface, "root", "handle", ROOT, "htb", "default", "1"])
            installed_root = True
            self.run([self.tc, "class", "add", "dev", interface, "parent", ROOT, "classid", CLASS, "htb",
                      "rate", f"{cfg['egress_bps']}bit", "ceil", f"{cfg['egress_bps']}bit",
                      "burst", str(cfg["burst_bytes"]), "cburst", str(cfg["burst_bytes"]), "quantum", str(cfg["quantum"])])
            self.run([self.tc, "qdisc", "add", "dev", interface, "parent", CLASS, "handle", LEAF, "fq_codel",
                      "target", f"{cfg['target_ms']}ms", "interval", f"{cfg['interval_ms']}ms",
                      "limit", str(cfg["packet_limit"]), "memory_limit", str(cfg["memory_limit_bytes"]),
                      "flows", str(cfg["flows"]), "quantum", str(cfg["quantum"]), "ecn"])
            actual = self.snapshot()
            if not self.owned_partial(actual) or not any(row.get("handle") == LEAF for row in actual["qdisc"]):
                raise RuntimeError("installed queue topology did not verify")
            state.update(status="active", tree=structure(actual))
            save(self.state_path, state)
        except BaseException:
            # Keep the ownership record if cleanup fails or an administrator changed the tree.
            current = self.snapshot()
            if installed_root and self.owned_partial(current):
                self.run([self.tc, "qdisc", "del", "dev", interface, "root", "handle", ROOT])
                self.state_path.unlink()
            elif self.pristine(current):
                self.state_path.unlink()
            raise

    def remove(self):
        state = self.state()
        if not state:
            return
        interface = validate(state["config"])["interface"]
        current = self.snapshot(interface)
        if self.pristine(current):
            self.state_path.unlink()
            return
        if not self.owned_partial(current) or (state["status"] == "active" and structure(current) != state["tree"]):
            raise RuntimeError("queue ownership/configuration conflict; refusing to delete")
        self.run([self.tc, "qdisc", "del", "dev", interface, "root", "handle", ROOT])
        self.state_path.unlink()

    def status(self):
        state = self.state()
        current = self.snapshot(state["config"]["interface"] if state else None)
        active = bool(state and state["status"] == "active" and structure(current) == state.get("tree"))
        return {"active": active, "state": state, "observed": current,
                "reason": "verified" if active else "not_installed" if not state else "configuration_conflict_or_incomplete_install"}

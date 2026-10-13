"""Opt-in real Linux namespace smoke; never touches the host's physical interface."""
import json
import os
import shutil
import sys
import tempfile
import unittest
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from queue_manager import QueueManager, command


@unittest.skipUnless(sys.platform == "linux" and os.environ.get("TELEMT_QUEUE_NETNS_TEST") == "1", "requires explicit Linux NET_ADMIN namespace test")
class LinuxQueueTest(unittest.TestCase):
    def test_real_kernel_tree_and_packets(self):
        ip, tc, ping = (shutil.which(name) for name in ("ip", "tc", "ping"))
        self.assertTrue(ip and tc and ping)
        a, b = "tqa" + uuid.uuid4().hex[:8], "tqb" + uuid.uuid4().hex[:8]
        made = []
        try:
            for name in (a, b):
                command([ip, "netns", "add", name]); made.append(name)
            command([ip, "-n", a, "link", "add", "test0", "type", "veth", "peer", "name", "test1"])
            command([ip, "-n", a, "link", "set", "test1", "netns", b])
            for name, device, address in [(a, "test0", "198.18.0.1/24"), (b, "test1", "198.18.0.2/24")]:
                command([ip, "-n", name, "addr", "add", address, "dev", device])
                command([ip, "-n", name, "link", "set", device, "up"])
            config = json.loads((Path(__file__).resolve().parents[1] / "queues.json.example").read_text())
            config.update(interface="test0", egress_bps=10000000)
            with tempfile.TemporaryDirectory() as directory:
                manager = QueueManager(config, Path(directory) / "state.json", tc, lambda args: command([ip, "netns", "exec", a] + args))
                manager.apply()
                self.assertTrue(manager.status()["active"])
                command([ip, "netns", "exec", a, ping, "-c", "5", "-i", "0.05", "-W", "1", "198.18.0.2"])
                rows = json.loads(command([ip, "netns", "exec", a, tc, "-s", "-j", "qdisc", "show", "dev", "test0"]))
                self.assertTrue(any(row.get("handle") == "7e11:" and row.get("bytes", 0) > 0 for row in rows))
                manager.remove()
                self.assertFalse(manager.state_path.exists())
        finally:
            for name in reversed(made):
                command([ip, "netns", "del", name])

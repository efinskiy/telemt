"""Lifecycle invariants without changing the host's network configuration."""
import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from queue_manager import QueueManager, validate

CONFIG = json.loads((Path(__file__).resolve().parents[1] / "queues.json.example").read_text())


class FakeTc:
    def __init__(self):
        self.tree = {"qdisc": [{"kind": "fq_codel", "handle": "0:", "root": True}], "class": [], "filter": []}
        self.mutations = []
        self.fail_leaf = False

    def __call__(self, args):
        if args[1] == "-j":
            return json.dumps(self.tree[args[3]])
        self.mutations.append(args)
        if args[1:3] == ["qdisc", "add"] and "root" in args:
            self.tree["qdisc"] = [{"kind": "htb", "handle": "7e10:", "root": True, "options": {"default": "1", "direct_packets_stat": 0}}]
        elif args[1:3] == ["class", "add"]:
            self.tree["class"] = [{"class": "htb", "handle": "7e10:1", "root": True, "rate": CONFIG["egress_bps"] // 8, "ceil": CONFIG["egress_bps"] // 8, "quantum": 1514}]
        elif args[1:3] == ["qdisc", "add"]:
            if self.fail_leaf:
                raise RuntimeError("fq_codel unavailable")
            self.tree["qdisc"].append({"kind": "fq_codel", "handle": "7e11:", "parent": "7e10:1", "options": {"limit": 1024}})
        elif args[1:3] == ["qdisc", "del"]:
            self.tree = FakeTc().tree
        else:
            raise AssertionError(args)
        return ""


class QueueTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.tc = FakeTc()
        self.manager = QueueManager(copy.deepcopy(CONFIG), Path(self.directory.name) / "state.json", run=self.tc)

    def test_foreign_root_is_never_replaced(self):
        self.tc.tree["qdisc"][0]["handle"] = "1234:"
        with self.assertRaisesRegex(RuntimeError, "foreign"):
            self.manager.apply()
        self.assertEqual(self.tc.mutations, [])
        self.assertFalse(self.manager.state_path.exists())

    def test_apply_is_idempotent_and_remove_only_owns_its_tree(self):
        self.manager.apply()
        self.assertTrue(self.manager.status()["active"])
        commands = len(self.tc.mutations)
        self.tc.tree["qdisc"][0]["options"]["direct_packets_stat"] = 100
        self.manager.apply()
        self.assertEqual(len(self.tc.mutations), commands)
        self.assertIn("190000000bit", self.tc.mutations[1])
        self.assertIn("memory_limit", self.tc.mutations[2])
        self.manager.remove()
        self.assertFalse(self.manager.state_path.exists())
        self.assertTrue(self.manager.pristine(self.tc.tree))

    def test_failed_leaf_rolls_back_only_our_partial_tree(self):
        self.tc.fail_leaf = True
        with self.assertRaisesRegex(RuntimeError, "fq_codel"):
            self.manager.apply()
        self.assertTrue(self.manager.pristine(self.tc.tree))
        self.assertFalse(self.manager.state_path.exists())

    def test_external_replacement_or_parameter_change_blocks_removal(self):
        self.manager.apply()
        self.tc.tree["qdisc"][1]["options"]["limit"] = 999
        commands = len(self.tc.mutations)
        self.assertFalse(self.manager.status()["active"])
        with self.assertRaisesRegex(RuntimeError, "conflict"):
            self.manager.remove()
        self.assertEqual(len(self.tc.mutations), commands)
        self.assertTrue(self.manager.state_path.exists())

    def test_configuration_change_requires_explicit_remove(self):
        self.manager.apply()
        self.manager.config["egress_bps"] = 100000000
        with self.assertRaisesRegex(RuntimeError, "differs"):
            self.manager.apply()
        self.manager.remove()
        self.manager.apply()
        self.assertTrue(self.manager.status()["active"])

    def test_flat_class_parameters_and_class_filters_are_protected(self):
        self.manager.apply()
        self.tc.tree["class"][0]["rate"] = 1
        with self.assertRaisesRegex(RuntimeError, "conflict"):
            self.manager.remove()
        self.tc.tree["class"][0]["rate"] = CONFIG["egress_bps"] // 8
        original = self.manager.run
        self.manager.run = lambda args: json.dumps([{"kind": "u32", "parent": "7e10:1"}]) if "parent" in args and "show" in args else original(args)
        with self.assertRaisesRegex(RuntimeError, "conflict"):
            self.manager.remove()

    def test_reboot_reinstalls_only_over_kernel_defaults(self):
        self.manager.apply()
        self.tc.tree = FakeTc().tree
        self.manager.apply()
        self.assertTrue(self.manager.status()["active"])
        self.assertEqual(sum("root" in args and "add" in args for args in self.tc.mutations), 2)

    def test_foreign_root_racing_install_is_not_replaced_or_deleted(self):
        original = self.manager.run
        def racing(args):
            if args[1:3] == ["qdisc", "add"] and "root" in args:
                self.tc.tree["qdisc"][0]["handle"] = "1234:"
                raise RuntimeError("File exists")
            return original(args)
        self.manager.run = racing
        with self.assertRaisesRegex(RuntimeError, "File exists"):
            self.manager.apply()
        self.assertEqual(self.tc.tree["qdisc"][0]["handle"], "1234:")
        self.assertEqual(self.tc.mutations, [])

    def test_input_is_bounded_before_tc_runs(self):
        for key, value in [("interface", "eth0;reboot"), ("flows", 100000000), ("egress_bps", True), ("target_ms", 100)]:
            config = copy.deepcopy(CONFIG)
            config[key] = value
            with self.assertRaises(ValueError):
                validate(config)


if __name__ == "__main__":
    unittest.main()

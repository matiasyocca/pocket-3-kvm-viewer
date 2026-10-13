import unittest
from unittest.mock import patch
import sample_cpu
from sample_cpu import cpu_percent, parse_stat


class SampleCpuTests(unittest.TestCase):
    def test_reused_process_id_is_rejected(self):
        before = {"time": 1, "ticks": 100, "starttime": 10, "threads": {}, "status": {}}
        after = {"time": 2, "ticks": 200, "starttime": 20, "threads": {}, "status": {}}
        with self.assertRaisesRegex(RuntimeError, "identity"):
            sample_cpu.summarize(before, after, 42, "test")

    def test_snapshot_records_process_starttime(self):
        fields = ["S"] + ["0"] * 19
        fields[11], fields[12], fields[19] = "120", "30", "12345"
        stat = "42 (viewer (test)) " + " ".join(fields)
        with patch("sample_cpu.Path.read_text", autospec=True,
                   side_effect=lambda path: "Threads: 1\n" if path.name == "status" else stat), \
                patch("sample_cpu.thread_ticks", return_value={}):
            snapshot = sample_cpu.snapshot(42)
        self.assertEqual(snapshot.get("starttime"), 12345)
        self.assertEqual(snapshot["ticks"], 150)

    def test_thread_ticks_records_thread_starttime(self):
        fields = ["S"] + ["0"] * 19
        fields[11], fields[12], fields[19] = "120", "30", "12345"
        stat = "7 (worker (test)) " + " ".join(fields)
        root = sample_cpu.Path("/unused")
        path = root / "task" / "7" / "stat"
        with patch("sample_cpu.Path.glob", return_value=[path]), \
                patch("sample_cpu.Path.read_text", return_value=stat):
            ticks = sample_cpu.thread_ticks(root)
        self.assertEqual(ticks["7"], ("worker (test)", 150, 12345))

    def test_reused_and_partial_interval_threads_are_excluded(self):
        before = {"time": 1, "ticks": 100, "starttime": 10, "status": {},
                  "threads": {"7": ("old", 10, 1), "8": ("stable", 20, 2),
                              "9": ("exited", 5, 3)}}
        after = {"time": 2, "ticks": 200, "starttime": 10, "status": {},
                 "threads": {"7": ("new", 30, 4), "8": ("stable", 40, 2),
                             "10": ("started", 5, 5)}}
        with patch("sample_cpu.os.sysconf", return_value=100):
            result = sample_cpu.summarize(before, after, 42, "test")
        self.assertEqual(result["threads"], [
            {"tid": "8", "name": "stable", "cpu_one_core_pct": 20.0}])
        self.assertEqual(result["cpu_one_core_pct"], 100.0)

    def test_stat_comm_can_contain_spaces_and_parentheses(self):
        fields = ["S"] + ["0"] * 10 + ["120", "30"]
        self.assertEqual(parse_stat("42 (viewer (test)) " + " ".join(fields)), 150)

    def test_percent_is_one_core_not_entire_machine(self):
        self.assertEqual(cpu_percent(100, 175, 100, 2.5), 30.0)

    def test_zero_interval_is_rejected(self):
        with self.assertRaises(ValueError):
            cpu_percent(0, 1, 100, 0)

    def test_negative_clock_rate_is_rejected(self):
        with self.assertRaises(ValueError):
            cpu_percent(0, 1, -100, 1)


if __name__ == "__main__":
    unittest.main()

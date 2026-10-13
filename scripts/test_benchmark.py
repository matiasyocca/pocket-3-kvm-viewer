from contextlib import contextmanager, ExitStack
from copy import deepcopy
import json
from pathlib import Path
import signal
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import benchmark
from benchmark import select_new_viewer, hidden_workspace


class BenchmarkTests(unittest.TestCase):
    @contextmanager
    def run_fixture(self, state="visible", displayed=1):
        viewer = {"address": "owned", "class": benchmark.APP_ID, "pid": 30,
                  "workspace": {"id": 1}, "size": [960, 540]}
        fixture = SimpleNamespace(viewer=viewer, displayed=displayed, clients_calls=0)

        def query(command):
            if command == "clients":
                fixture.clients_calls += 1
                return [] if fixture.clients_calls == 1 else [deepcopy(viewer)]
            if command == "monitors":
                return [{"activeWorkspace": {"id": fixture.displayed}}]
            if command == "workspaces":
                return [{"id": 1}]
            if command == "activewindow":
                return {}
            raise AssertionError(command)

        def dispatch(command, argument):
            if command == "movetoworkspacesilent":
                viewer["workspace"] = {"id": int(argument.split(",", 1)[0])}

        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            fixture.output = Path(directory) / "samples.jsonl"
            fixture.args = SimpleNamespace(output=str(fixture.output), binary=sys.executable,
                                           label="test", argument=[], allow_no_capture=True,
                                           warmup=0, states=state, repeats=1, seconds=1,
                                           revision="test")
            fixture.proc = Mock(pid=10)
            stack.enter_context(patch("benchmark.subprocess.Popen", return_value=fixture.proc))
            stack.enter_context(patch("benchmark.os.killpg"))
            fixture.pgid = stack.enter_context(patch("benchmark.os.getpgid", return_value=10))
            stack.enter_context(patch("benchmark.process_group", return_value={}))
            fixture.starttime = stack.enter_context(patch("benchmark.process_starttime", return_value=100))
            fixture.query = stack.enter_context(patch("benchmark.hypr_json", side_effect=query))
            fixture.dispatch = stack.enter_context(patch("benchmark.dispatch", side_effect=dispatch))
            stack.enter_context(patch("benchmark.time.sleep"))
            stack.enter_context(patch("benchmark.Path.iterdir", return_value=iter([])))
            fixture.sample = stack.enter_context(patch("benchmark.sample", return_value={
                "cpu_one_core_pct": 0, "status": {}, "label": state}))
            stack.enter_context(patch("builtins.print"))
            yield fixture

    def test_visible_and_returned_require_a_displayed_workspace(self):
        for state in ("visible", "returned"):
            with self.subTest(state=state), self.run_fixture(state, displayed=2) as fixture:
                with self.assertRaisesRegex(RuntimeError, "workspace"):
                    benchmark.run(fixture.args)
                fixture.sample.assert_not_called()
                self.assertFalse(fixture.output.exists())

    def test_changed_monitor_workspace_after_sampling_is_not_recorded(self):
        for state, changed in (("visible", 2), ("returned", 2), ("hidden", 1000)):
            with self.subTest(state=state), self.run_fixture(state) as fixture:
                def sample(*args):
                    fixture.displayed = changed
                    return {"cpu_one_core_pct": 0, "status": {}, "label": state}
                fixture.sample.side_effect = sample
                with self.assertRaisesRegex(RuntimeError, "workspace"):
                    benchmark.run(fixture.args)
                fixture.sample.assert_called_once()
                self.assertFalse(fixture.output.exists())

    def test_all_three_valid_states_are_recorded(self):
        with self.run_fixture("visible,hidden,returned") as fixture:
            benchmark.run(fixture.args)
            rows = [json.loads(line) for line in fixture.output.read_text().splitlines()]
            self.assertEqual([row["workspace"]["id"] for row in rows], [1, 1000, 1])
            self.assertEqual(fixture.sample.call_count, 3)
            fixture.proc.poll.assert_not_called()

    def test_failed_workspace_move_is_not_sampled(self):
        with self.run_fixture("hidden") as fixture:
            fixture.dispatch.side_effect = None
            with self.assertRaisesRegex(RuntimeError, "requested workspace"):
                benchmark.run(fixture.args)
            fixture.sample.assert_not_called()

    def test_hidden_workspace_on_another_monitors_special_workspace_is_rejected(self):
        with self.run_fixture("hidden") as fixture:
            original_query = fixture.query.side_effect
            monitors_calls = 0
            def query(command):
                nonlocal monitors_calls
                if command == "monitors":
                    monitors_calls += 1
                    if monitors_calls > 1:
                        return [{"activeWorkspace": {"id": 1}},
                                {"activeWorkspace": {"id": 2}, "specialWorkspace": {"id": 1000}}]
                return original_query(command)
            fixture.query.side_effect = query
            with self.assertRaisesRegex(RuntimeError, "workspace"):
                benchmark.run(fixture.args)
            fixture.sample.assert_not_called()

    def test_replaced_viewer_is_not_moved_or_sampled(self):
        for changed in ("pid", "starttime", "group", "class"):
            with self.subTest(changed=changed), self.run_fixture() as fixture:
                original_dispatch = fixture.dispatch.side_effect
                def dispatch(command, argument):
                    original_dispatch(command, argument)
                    if command == "setfloating":
                        if changed == "pid":
                            fixture.viewer["pid"] = 99
                        elif changed == "class":
                            fixture.viewer["class"] = "unrelated"
                        elif changed == "group":
                            fixture.pgid.return_value = 99
                        else:
                            fixture.starttime.side_effect = lambda pid: 101 if pid == 30 else 100
                fixture.dispatch.side_effect = dispatch
                with self.assertRaisesRegex(RuntimeError, "identity"):
                    benchmark.run(fixture.args)
                fixture.sample.assert_not_called()
                self.assertFalse(any(call.args[0] in {"resizewindowpixel", "movewindowpixel"}
                                     for call in fixture.dispatch.call_args_list))
                self.assertFalse(any(call.args[0] == "movetoworkspacesilent"
                                     for call in fixture.dispatch.call_args_list))

    def test_cleanup_kills_survivor_even_when_launcher_exits_on_term(self):
        with self.run_fixture() as fixture:
            group = {10: ("S", 100), 30: ("S", 200)}

            def kill(pgid, sig):
                self.assertEqual(pgid, 10)
                if sig == signal.SIGTERM:
                    group[10] = ("Z", 100)
                elif sig == signal.SIGKILL:
                    group[30] = ("Z", 200)

            fixture.proc.wait.side_effect = lambda **kwargs: group.clear()
            with patch("benchmark.os.killpg", side_effect=kill) as signals, \
                    patch("benchmark.process_group", side_effect=lambda pgid: dict(group)), \
                    patch("benchmark.process_starttime", return_value=100), \
                    patch("benchmark.time.monotonic", side_effect=range(1000)):
                benchmark.run(fixture.args)
            self.assertEqual([call.args[1] for call in signals.call_args_list],
                             [signal.SIGTERM, signal.SIGKILL])
            fixture.proc.poll.assert_not_called()
            self.assertFalse(group)

    def test_cleanup_refuses_reused_launcher_identity(self):
        proc = Mock(pid=10)
        with patch("benchmark.process_group", return_value={10: ("S", 999)}), \
                patch("benchmark.os.killpg") as signals:
            with self.assertRaisesRegex(RuntimeError, "identity"):
                benchmark.stop_process_group(proc, 100)
        signals.assert_not_called()

    def test_cleanup_requires_entire_original_group_to_disappear(self):
        proc = Mock(pid=10)
        group = {10: ("Z", 100), 30: ("Z", 200)}
        proc.wait.side_effect = lambda **kwargs: group.pop(10)
        with patch("benchmark.process_group", side_effect=lambda pgid: dict(group)), \
                patch("benchmark.os.killpg") as signals, \
                patch("benchmark.time.monotonic", side_effect=range(100)), \
                patch("benchmark.time.sleep"):
            with self.assertRaisesRegex(RuntimeError, "did not disappear"):
                benchmark.stop_process_group(proc, 100)
        signals.assert_not_called()

    def test_cleanup_does_not_follow_reused_pids_after_reaping(self):
        proc = Mock(pid=10)
        group = {10: ("Z", 100), 30: ("Z", 200)}
        def reap(**kwargs):
            group.clear()
            group.update({10: ("S", 999), 30: ("S", 888)})
        proc.wait.side_effect = reap
        with patch("benchmark.process_group", side_effect=lambda pgid: dict(group)), \
                patch("benchmark.os.killpg") as signals:
            benchmark.stop_process_group(proc, 100)
        signals.assert_not_called()

    def test_process_group_requires_matching_group_and_session(self):
        paths = [Path(f"/proc/{pid}/stat") for pid in (10, 20, 30, 40)]
        def stat(path):
            if path.parent.name == "40":
                raise FileNotFoundError
            fields = ["S"] + ["0"] * 19
            fields[2] = "10" if path.parent.name != "20" else "20"
            fields[3] = "10" if path.parent.name != "30" else "30"
            fields[19] = "12345"
            return f"{path.parent.name} (launcher (test)) " + " ".join(fields)
        with patch("benchmark.Path.glob", return_value=paths), \
                patch("benchmark.Path.read_text", autospec=True, side_effect=stat):
            self.assertEqual(benchmark.process_group(10), {10: ("S", 12345)})
            self.assertEqual(benchmark.process_starttime(10), 12345)

    def test_new_viewer_must_belong_to_launched_process_group(self):
        clients = [{"address": "unrelated", "class": benchmark.APP_ID, "pid": 20},
                   {"address": "owned", "class": benchmark.APP_ID, "pid": 30}]
        with patch("benchmark.os.getpgid", side_effect=lambda pid: {20: 20, 30: 10}[pid]):
            selected = select_new_viewer(clients, set(), 10)
            self.assertEqual(selected["address"], "owned")

    def test_never_selects_preexisting_viewer(self):
        clients = [{"address": "old", "class": benchmark.APP_ID, "pid": 20},
                   {"address": "new", "class": benchmark.APP_ID, "pid": 30}]
        with patch("benchmark.os.getpgid", return_value=10):
            self.assertEqual(select_new_viewer(clients, {"old"}, 10)["address"], "new")

    def test_ambiguous_new_viewers_are_rejected(self):
        clients = [{"address": x, "class": benchmark.APP_ID, "pid": 30} for x in ("a", "b")]
        with patch("benchmark.os.getpgid", return_value=10), self.assertRaises(ValueError):
            select_new_viewer(clients, set(), 10)

    def test_absent_new_window_returns_none(self):
        self.assertIsNone(select_new_viewer([], set(), 10))

    def test_compositor_calls_have_finite_timeouts(self):
        with patch("benchmark.subprocess.check_output", return_value="{}") as query:
            benchmark.hypr_json("activewindow")
        self.assertGreater(query.call_args.kwargs.get("timeout", 0), 0)
        with patch("benchmark.subprocess.run") as dispatch:
            benchmark.dispatch("focuswindow", "address:owned")
        self.assertGreater(dispatch.call_args.kwargs.get("timeout", 0), 0)

    def test_hidden_workspace_is_not_on_a_monitor_or_occupied(self):
        monitors = [{"activeWorkspace": {"id": 1000}, "specialWorkspace": {"id": -1}}]
        self.assertEqual(hidden_workspace(monitors, [{"id": 1001}]), 1002)


if __name__ == "__main__":
    unittest.main()

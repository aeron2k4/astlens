#!/usr/bin/env python3
"""Stdio MCP bridge between Antigravity (SEP-2567 NO_INITIALIZE or legacy) and `astlens --serve <ROOT>`."""

from __future__ import annotations

from collections import OrderedDict
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
from typing import Any

ASTLENS_BIN = (
    os.environ.get("ASTLENS_BIN")
    or (sys.argv[1] if len(sys.argv) > 1 and sys.argv[1] else None)
    or "astlens"
)
CACHE_ROOT_FILE = (
    Path(os.environ.get("XDG_CACHE_HOME") or (Path.home() / ".cache"))
    / "astlens"
    / "antigravity_root"
)
MAX_WORKERS = 3
REINIT_ID = "__agy_reinit__"


def _is_broad_root(path: Path) -> bool:
    if path == Path("/"):
        return True
    home_raw = os.environ.get("HOME") or os.environ.get("USERPROFILE")
    if home_raw:
        try:
            home = Path(home_raw).expanduser().resolve()
            if path == home:
                return True
        except OSError:
            pass
    return False


def _read_dir(raw: str | None) -> str:
    if not raw or not raw.strip():
        return ""
    try:
        resolved = Path(raw.strip()).expanduser().resolve()
    except OSError:
        return ""
    if not resolved.is_dir() or _is_broad_root(resolved):
        return ""
    return str(resolved)


def current_root_str() -> str:
    try:
        if CACHE_ROOT_FILE.is_file():
            cached = _read_dir(CACHE_ROOT_FILE.read_text(encoding="utf-8"))
            if cached:
                return cached
    except OSError:
        pass
    return _read_dir(os.environ.get("ASTLENS_ROOT"))


def current_bin_sig() -> tuple[int, int] | None:
    try:
        st = os.stat(ASTLENS_BIN)
        return (st.st_ino, st.st_mtime_ns)
    except OSError:
        return None


class Worker:
    """One `astlens --serve` subprocess bound to an explicit workspace root."""

    def __init__(
        self,
        root_str: str,
        bin_sig: tuple[int, int] | None,
        stdout_lock: threading.Lock,
    ) -> None:
        self.root_str = root_str
        self.bin_sig = bin_sig
        self.stdout_lock = stdout_lock
        self.stdin_lock = threading.Lock()
        self.reinit_done = threading.Event()
        self.reinit_ok = False

        cmd = [ASTLENS_BIN, "--serve", root_str] if root_str else [ASTLENS_BIN, "--serve"]
        self.proc = subprocess.Popen(
            cmd,
            cwd=root_str if root_str else None,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=sys.stderr,
            bufsize=0,
        )
        assert self.proc.stdin is not None
        assert self.proc.stdout is not None
        self.pump = threading.Thread(target=self._stdout_pump, daemon=True)
        self.pump.start()

    def is_alive(self) -> bool:
        return self.proc.poll() is None

    def send(self, payload: bytes) -> bool:
        with self.stdin_lock:
            if self.proc.stdin is None:
                return False
            try:
                self.proc.stdin.write(payload)
                self.proc.stdin.flush()
                return True
            except OSError:
                return False

    def _stdout_pump(self) -> None:
        assert self.proc.stdout is not None
        try:
            for raw_line in self.proc.stdout:
                stripped = raw_line.strip()
                if not stripped:
                    continue

                if b"__agy_reinit__" in stripped or b'"roots/list"' in stripped:
                    try:
                        msg = json.loads(stripped)
                        if isinstance(msg, dict):
                            if msg.get("id") == REINIT_ID and "method" not in msg:
                                self.reinit_ok = "error" not in msg
                                self.reinit_done.set()
                                continue
                            if msg.get("method") == "roots/list" and "id" in msg:
                                roots = (
                                    [
                                        {
                                            "uri": Path(self.root_str).as_uri(),
                                            "name": Path(self.root_str).name,
                                        }
                                    ]
                                    if self.root_str
                                    else []
                                )
                                reply = (
                                    json.dumps(
                                        {
                                            "jsonrpc": "2.0",
                                            "id": msg["id"],
                                            "result": {"roots": roots},
                                        }
                                    ).encode("utf-8")
                                    + b"\n"
                                )
                                self.send(reply)
                                continue
                    except Exception:
                        pass

                with self.stdout_lock:
                    try:
                        sys.stdout.buffer.write(raw_line)
                        sys.stdout.buffer.flush()
                    except OSError:
                        break
        finally:
            self.reinit_done.set()

    def close(self) -> None:
        with self.stdin_lock:
            if self.proc.stdin is not None:
                try:
                    self.proc.stdin.close()
                except OSError:
                    pass
        try:
            self.proc.wait(timeout=0.5)
        except subprocess.TimeoutExpired:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=1.0)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=1.0)
        self.pump.join(timeout=0.5)


def main() -> int:
    stdout_lock = threading.Lock()
    workers: OrderedDict[str, Worker] = OrderedDict()
    saved_init_req: dict[str, Any] | None = None
    saved_init_notif: bytes | None = None
    last_worker: Worker | None = None

    def get_worker(root_str: str, *, replay_init: bool) -> Worker:
        sig = current_bin_sig()
        existing = workers.get(root_str)
        if existing is not None and existing.is_alive() and existing.bin_sig == sig:
            workers.move_to_end(root_str)
            return existing

        if existing is not None:
            del workers[root_str]
            existing.close()

        worker = Worker(root_str, sig, stdout_lock)
        if replay_init and saved_init_req is not None:
            reinit = copy.deepcopy(saved_init_req)
            reinit["id"] = REINIT_ID
            payload = json.dumps(reinit).encode("utf-8") + b"\n"
            if worker.send(payload):
                worker.reinit_done.wait(timeout=1.0)
                if saved_init_notif is not None:
                    worker.send(saved_init_notif)

        workers[root_str] = worker
        while len(workers) > MAX_WORKERS:
            _, evicted = workers.popitem(last=False)
            evicted.close()
        return worker

    try:
        for raw_line in sys.stdin.buffer:
            stripped = raw_line.strip()
            if not stripped:
                continue

            msg: Any = None
            try:
                msg = json.loads(stripped)
            except Exception:
                msg = None

            method = msg.get("method") if isinstance(msg, dict) else None

            if method == "initialize":
                saved_init_req = copy.deepcopy(msg)
                saved_init_notif = None
                worker = get_worker(current_root_str(), replay_init=False)
                last_worker = worker
                if not worker.send(raw_line):
                    worker = get_worker(current_root_str(), replay_init=False)
                    last_worker = worker
                    worker.send(raw_line)
                continue

            if method == "notifications/initialized":
                saved_init_notif = raw_line
                if last_worker is not None and last_worker.is_alive():
                    last_worker.send(raw_line)
                else:
                    worker = get_worker(current_root_str(), replay_init=False)
                    last_worker = worker
                    worker.send(raw_line)
                continue

            if method == "notifications/cancelled" and last_worker is not None and last_worker.is_alive():
                last_worker.send(raw_line)
                continue

            root_str = current_root_str()
            worker = get_worker(root_str, replay_init=True)
            last_worker = worker
            if not worker.send(raw_line):
                worker = get_worker(root_str, replay_init=True)
                last_worker = worker
                if not worker.send(raw_line):
                    break
    finally:
        for worker in list(workers.values()):
            worker.close()

    return 0


if __name__ == "__main__":
    raise SystemExit(main())

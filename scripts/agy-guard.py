#!/usr/bin/env python3
"""Antigravity hook adapter for astlens: tracks workspace root and enforces astlens-guard.sh."""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from typing import Any
from urllib.parse import unquote, urlparse

CACHE_ROOT_FILE = (
    Path(os.environ.get("XDG_CACHE_HOME") or (Path.home() / ".cache"))
    / "astlens"
    / "antigravity_root"
)
GUARD_SCRIPT = Path(__file__).resolve().parent / "astlens-guard.sh"


def _allow() -> dict[str, Any]:
    return {"decision": "allow"}


def _to_dir(raw: Any) -> Path | None:
    if not isinstance(raw, str) or not raw.strip():
        return None
    text = raw.strip()
    if text.startswith("file://"):
        parsed = urlparse(text)
        text = unquote(parsed.path)
        if not text:
            return None
    try:
        resolved = Path(text).expanduser().resolve()
    except OSError:
        return None
    if not resolved.is_dir():
        return None
    return resolved


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


def _pick_workspace_root(payload: dict[str, Any]) -> Path | None:
    candidates: list[Any] = [payload.get("cwd")]
    ws_paths = payload.get("workspacePaths")
    if isinstance(ws_paths, list):
        candidates.extend(ws_paths)
    for raw in candidates:
        candidate = _to_dir(raw)
        if candidate is not None and not _is_broad_root(candidate):
            return candidate
    return None


def _record_root(ws_root: Path) -> None:
    target_text = f"{ws_root}\n"
    try:
        if CACHE_ROOT_FILE.is_file() and CACHE_ROOT_FILE.read_text(encoding="utf-8") == target_text:
            return
    except OSError:
        pass

    CACHE_ROOT_FILE.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    try:
        os.chmod(CACHE_ROOT_FILE.parent, 0o700)
    except OSError:
        pass

    fd, tmp_name = tempfile.mkstemp(
        dir=CACHE_ROOT_FILE.parent,
        prefix=".antigravity_root.",
        text=True,
    )
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(target_text)
        os.chmod(tmp_name, 0o600)
        os.replace(tmp_name, CACHE_ROOT_FILE)
    except Exception:
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise


def _adapt_reason(reason: str) -> str:
    adapted = re.sub(
        r'\s*\(load with ToolSearch "[^"]+"\)',
        ' (via call_mcp_tool with ServerName="astlens")',
        reason,
    )
    adapted = adapted.replace("Whole-file Read of ", "Whole-file read_file of ")
    adapted = adapted.replace("or a Read without", "or a read_file without")
    adapted = adapted.replace(
        "Read only the needed lines with offset and a limit of",
        "pass StartLine and EndLine spanning",
    )
    return adapted


def _map_tool_input(tool_name: str, args: dict[str, Any], ws_root: Path) -> dict[str, Any] | None:
    cwd_str = str(ws_root)
    if tool_name in {"read_file", "view_file"}:
        file_path = args.get("Path") or args.get("AbsolutePath") or ""
        if not isinstance(file_path, str) or not file_path:
            return None
        end = args.get("EndLine")
        start = args.get("StartLine")
        limit = ""
        if isinstance(end, int) and not isinstance(end, bool):
            start_line = start if isinstance(start, int) and not isinstance(start, bool) and start >= 1 else 1
            limit = str(end - start_line + 1) if end >= start_line else "999999"
        return {
            "tool_name": "Read",
            "cwd": cwd_str,
            "tool_input": {
                "file_path": file_path,
                "limit": limit,
            },
        }

    if tool_name == "grep_search":
        search_path = args.get("SearchPath") or args.get("Directory") or "."
        if not isinstance(search_path, str) or not search_path:
            search_path = "."
        includes = args.get("Includes")
        if isinstance(includes, list):
            glob = ",".join(str(item) for item in includes if item)
        elif isinstance(includes, str):
            glob = includes
        else:
            glob = ""
        return {
            "tool_name": "Grep",
            "cwd": cwd_str,
            "tool_input": {
                "path": search_path,
                "glob": glob,
            },
        }

    if tool_name == "run_command":
        cmd = args.get("CommandLine") or ""
        workdir = args.get("Cwd") or ""
        if not isinstance(cmd, str) or not cmd:
            return None
        if not isinstance(workdir, str):
            workdir = ""
        return {
            "tool_name": "Bash",
            "cwd": cwd_str,
            "tool_input": {
                "command": cmd,
                "workdir": workdir,
            },
        }

    return None


def main() -> int:
    try:
        raw = sys.stdin.read()
        if not raw.strip():
            return 0
        payload = json.loads(raw)
        if not isinstance(payload, dict):
            return 0
    except Exception:
        return 0

    ws_root = _pick_workspace_root(payload)
    if ws_root is not None:
        try:
            _record_root(ws_root)
        except OSError:
            pass

    event = payload.get("hookEventName")
    if event != "PreToolUse":
        print("{}")
        return 0

    tool_call = payload.get("toolCall")
    if not isinstance(tool_call, dict):
        print(json.dumps(_allow()))
        return 0

    tool_name = str(tool_call.get("name") or "")
    if tool_name == "call_mcp_tool" or ws_root is None:
        print(json.dumps(_allow()))
        return 0

    args = tool_call.get("args")
    if not isinstance(args, dict):
        args = {}

    mapped = _map_tool_input(tool_name, args, ws_root)
    if mapped is None or not GUARD_SCRIPT.is_file():
        print(json.dumps(_allow()))
        return 0

    env = os.environ.copy()
    env.pop("ASTLENS_OPENCODE", None)

    try:
        proc = subprocess.run(
            [str(GUARD_SCRIPT)],
            input=json.dumps(mapped),
            text=True,
            capture_output=True,
            env=env,
            timeout=8,
            check=False,
        )
    except Exception:
        print(json.dumps(_allow()))
        return 0

    out = proc.stdout.strip()
    if proc.returncode != 0 or not out:
        print(json.dumps(_allow()))
        return 0

    try:
        guard_out = json.loads(out)
        hook_out = guard_out.get("hookSpecificOutput") if isinstance(guard_out, dict) else None
        if isinstance(hook_out, dict) and hook_out.get("permissionDecision") == "deny":
            reason = str(hook_out.get("permissionDecisionReason") or "Blocked by astlens guard.")
            print(json.dumps({"decision": "deny", "reason": _adapt_reason(reason)}))
            return 0
    except Exception:
        pass

    print(json.dumps(_allow()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

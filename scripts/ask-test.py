#!/usr/bin/env python3
"""Asks Meshgrove a question the way an AI app does, to see the question window.

It starts `meshgrove-mcp` (the program AI apps talk to), says hello as an app named
"ask-test" and calls its `ask_user` tool. Meshgrove, if it runs and AI is switched on, shows
the question in a window of its own. The script waits for the answer and prints it.

    python3 scripts/ask-test.py              # two questions
    python3 scripts/ask-test.py --seconds 30 # give up after 30 seconds

Nothing in the library changes: a question only asks.
"""

import argparse
import json
import shutil
import subprocess
import sys
import threading

QUESTIONS = [
    {
        "question": "Which filament should the test print use?",
        "options": ["PLA", "PETG", "TPU"],
        "allow_other": True,
    },
    {
        "question": "Which extras do you want? (pick any)",
        "options": ["Brim", "Supports", "Ironing"],
        "multi": True,
    },
]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--mcp", default=shutil.which("meshgrove-mcp") or "meshgrove-mcp",
                        help="the meshgrove-mcp program (default: the one on the PATH)")
    parser.add_argument("--seconds", type=float, default=900.0,
                        help="how long to wait for the answer (default: 900, Meshgrove's own limit)")
    args = parser.parse_args()

    try:
        server = subprocess.Popen([args.mcp], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    except OSError as e:
        print(f"cannot start {args.mcp}: {e}", file=sys.stderr)
        return 2

    def send(message: dict) -> None:
        server.stdin.write(json.dumps(message) + "\n")
        server.stdin.flush()

    def answer_to(wanted: int) -> dict | None:
        # One JSON message per line; anything that is not our answer (a log line, a
        # notification) is passed over.
        for line in server.stdout:
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if message.get("id") == wanted:
                return message
        return None

    # Give up after the time asked for: closing the server's input ends it, and it then marks
    # the question as not answered, so no window is left waiting in Meshgrove.
    def give_up() -> None:
        try:
            server.stdin.close()
        except OSError:
            pass

    timer = threading.Timer(args.seconds, give_up)
    timer.daemon = True
    timer.start()

    send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-03-26",
        "capabilities": {},
        "clientInfo": {"name": "ask-test", "version": "1"},
    }})
    if answer_to(1) is None:
        print("meshgrove-mcp did not answer the hello", file=sys.stderr)
        return 2
    send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    print("Asked. Look at Meshgrove: the question is in a window of its own.", flush=True)
    send({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
          "params": {"name": "ask_user", "arguments": {"questions": QUESTIONS}}})
    reply = answer_to(2)
    timer.cancel()
    give_up()
    try:
        server.wait(timeout=5)
    except subprocess.TimeoutExpired:
        server.kill()
    if reply is None:
        print("no answer in time", file=sys.stderr)
        return 1
    result = reply.get("result", reply.get("error"))
    print(json.dumps(result, indent=2, ensure_ascii=False))
    return 0 if "result" in reply and not reply["result"].get("isError") else 1


if __name__ == "__main__":
    sys.exit(main())

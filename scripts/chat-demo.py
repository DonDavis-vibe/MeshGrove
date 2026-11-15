#!/usr/bin/env python3
"""Opens a second Meshgrove with a stand-in agent, to see the chat's cards.

The second Meshgrove runs on empty folders of its own (settings, library index, cache), made
for this run and removed when it closes. Your Meshgrove, your libraries, your chats and your AI
apps are not touched, and nothing goes to any AI service: the agent is `scripts/fake-agent.py`,
which makes its answers up.

    python3 scripts/chat-demo.py

In the window that opens: the "AI chat" tab at the right, then write one of these words:

    permission   the approval card of an agent that wants to run something
    sign in      the sign-in card
    warn         a warning under an answer
    fail         an answer that did not come through

Close the window to end the demo.
"""

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--meshgrove", default=shutil.which("meshgrove") or str(Path.home() / ".local/bin/meshgrove"),
                        help="the Meshgrove program (default: the installed one)")
    args = parser.parse_args()
    if not Path(args.meshgrove).exists():
        print(f"Meshgrove is not at {args.meshgrove}. Install it, or name it with --meshgrove.", file=sys.stderr)
        return 1

    agent = Path(__file__).resolve().with_name("fake-agent.py")
    language = "de" if os.environ.get("LANG", "").lower().startswith("de") else "en"
    with tempfile.TemporaryDirectory(prefix="meshgrove-chat-demo-") as home:
        home = Path(home)
        config = home / "config" / "meshgrove"
        config.mkdir(parents=True)
        (home / "data").mkdir()
        (home / "cache").mkdir()
        (home / "models").mkdir()
        # The agent counts as running on this computer, so no question about a cloud comes up.
        (config / "config.toml").write_text(
            f'language = "{language}"\n\n'
            "[ai]\n"
            "enabled = true\n"
            "connect_apps = false\n"
            'assistant = "demo"\n'
            'auto_connect = "demo"\n\n'
            "[[ai.agents]]\n"
            'id = "demo"\n'
            'name = "Demo agent"\n'
            f"command = '\"{sys.executable}\" \"{agent}\"'\n"
            "local = true\n",
            encoding="utf-8",
        )
        env = dict(
            os.environ,
            XDG_CONFIG_HOME=str(home / "config"),
            XDG_DATA_HOME=str(home / "data"),
            XDG_CACHE_HOME=str(home / "cache"),
            # This run never adds itself to the AI apps on this computer.
            MESHGROVE_NO_AI_CONNECT="1",
        )
        print("A second Meshgrove opens on folders of its own. Open the \"AI chat\" tab at the right")
        print("and write: permission, sign in, warn or fail. Close the window to end the demo.")
        return subprocess.run([args.meshgrove, str(home / "models")], env=env, check=False).returncode


if __name__ == "__main__":
    sys.exit(main())

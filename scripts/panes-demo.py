#!/usr/bin/env python3
"""Opens a second Meshgrove on a made-up library, to see the review and its neighbours.

The second Meshgrove runs on folders of its own (settings, a library of a few cubes, cache),
made for this run and removed when it closes. Your Meshgrove, your libraries and your AI apps
are not touched, and nothing goes to any AI service: the suggestions are made up here and
queued through `meshgrove-mcp`, the program AI apps talk to.

    python3 scripts/panes-demo.py

What the window has to show, once it has read its library (a few seconds):

    Review       the ✨ button at the top right: one suggestion of every kind
                 (tags, a note, a move, a print list, a deletion, an archive to unpack)
    History      in the review; it fills as you accept suggestions or import
    Duplicates   at the left: a group of copies, an archive with extracted copies, twin archives
    Import       at the left: a file, an archive and a folder waiting to come in
    Inbox        at the left, once something was imported

Close the window to end the demo.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import zipfile
from pathlib import Path


def cube(path: Path, edge: float) -> None:
    """A cube of this edge as a text STL: every size is another model to Meshgrove."""
    s = edge
    v = [(0, 0, 0), (s, 0, 0), (s, s, 0), (0, s, 0), (0, 0, s), (s, 0, s), (s, s, s), (0, s, s)]
    faces = [(0, 2, 1), (0, 3, 2), (4, 5, 6), (4, 6, 7), (0, 1, 5), (0, 5, 4),
             (1, 2, 6), (1, 6, 5), (2, 3, 7), (2, 7, 6), (3, 0, 4), (3, 4, 7)]
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="ascii") as out:
        out.write(f"solid {path.stem}\n")
        for face in faces:
            out.write(" facet normal 0 0 0\n  outer loop\n")
            for i in face:
                out.write("   vertex %g %g %g\n" % v[i])
            out.write("  endloop\n endfacet\n")
        out.write(f"endsolid {path.stem}\n")
    # Not "still downloading": the import leaves files alone that changed in the last minute.
    old = time.time() - 600
    os.utime(path, (old, old))


def make_library(home: Path) -> tuple[Path, Path]:
    lib, incoming = home / "Models", home / "Incoming"
    cube(lib / "Garden" / "rose-trellis.stl", 40)
    cube(lib / "Garden" / "plant-label.stl", 25)
    cube(lib / "Garden" / "hose-clip.stl", 15)
    cube(lib / "Workshop" / "drill-guide.stl", 32)
    cube(lib / "Workshop" / "old-jig.stl", 18)
    cube(lib / "Workshop" / "failed-test.stl", 9)
    # A copy of a file: a group of duplicates.
    (lib / "Unsorted").mkdir(parents=True, exist_ok=True)
    shutil.copy2(lib / "Garden" / "rose-trellis.stl", lib / "Unsorted" / "rose-trellis (copy).stl")
    # An archive whose models also lie unpacked beside it, and its twin under another name.
    cube(lib / "Kitchen" / "spice-rack.stl", 45)
    cube(lib / "Kitchen" / "jar-lid.stl", 22)
    for name in ("Kitchen/kitchen-set.zip", "Unsorted/kitchen-set-download.zip"):
        with zipfile.ZipFile(lib / name, "w") as pack:
            pack.write(lib / "Kitchen" / "spice-rack.stl", "spice-rack.stl")
            pack.write(lib / "Kitchen" / "jar-lid.stl", "jar-lid.stl")
    # An archive to unpack, for the review.
    cube(home / "tmp" / "bracket-a.stl", 28)
    cube(home / "tmp" / "bracket-b.stl", 29)
    with zipfile.ZipFile(lib / "Workshop" / "brackets.zip", "w") as pack:
        pack.write(home / "tmp" / "bracket-a.stl", "bracket-a.stl")
        pack.write(home / "tmp" / "bracket-b.stl", "bracket-b.stl")
    # What waits to come in: a file, an archive, and a folder (which comes in as a whole).
    cube(incoming / "cable-clip.stl", 12)
    cube(incoming / "Lamp parts" / "shade.stl", 60)
    cube(incoming / "Lamp parts" / "Base" / "foot.stl", 35)
    cube(home / "tmp" / "knob.stl", 14)
    with zipfile.ZipFile(incoming / "knobs.zip", "w") as pack:
        pack.write(home / "tmp" / "knob.stl", "knob.stl")
    old = time.time() - 600
    for path in list(lib.rglob("*.zip")) + list(incoming.rglob("*.zip")):
        os.utime(path, (old, old))
    shutil.rmtree(home / "tmp")
    return lib, incoming


SUGGESTIONS = [
    ("propose_tags", {"title": "Sort the garden parts", "items": [
        {"id": "Models/Garden/rose-trellis.stl", "add": ["garden", "trellis"], "reason": "A trellis for roses"},
        {"id": "Models/Garden/plant-label.stl", "add": ["garden", "label"]},
    ]}),
    ("propose_note", {"title": "Notes on the sizes", "items": [
        {"id": "Models/Garden/hose-clip.stl", "note": "Fits a 15 mm hose. Print with PETG."},
    ]}),
    ("propose_moves", {"title": "Tidy the copies", "items": [
        {"id": "Models/Unsorted/rose-trellis (copy).stl", "to": "Models/Garden/Spare/",
         "reason": "Belongs with the garden parts"},
        {"id": "Models/Workshop/old-jig.stl", "to": "Models/Workshop/jig-v1.stl", "reason": "A clearer name"},
    ]}),
    ("propose_list", {"name": "Weekend prints", "kind": "print", "items": [
        {"id": "Models/Garden/plant-label.stl", "quantity": 6},
        {"id": "Models/Workshop/drill-guide.stl", "quantity": 1},
    ]}),
    ("propose_delete", {"title": "Old versions", "items": [
        {"id": "Models/Workshop/failed-test.stl", "reason": "A test piece that did not work"},
    ]}),
    ("propose_archive", {"title": "Unpack the brackets", "items": [
        {"op": "unpack", "archive": "Models/Workshop/brackets.zip", "reason": "To see what is in it"},
    ]}),
]


class Mcp:
    """A talk with `meshgrove-mcp` over its standard input and output, as an AI app has it."""

    def __init__(self, program: str, env: dict):
        self.server = subprocess.Popen([program], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)
        self.count = 0
        self.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                 "clientInfo": {"name": "panes-demo", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, message: dict) -> None:
        self.server.stdin.write(json.dumps(message) + "\n")
        self.server.stdin.flush()

    def call(self, method: str, params: dict) -> dict:
        self.count += 1
        self.send({"jsonrpc": "2.0", "id": self.count, "method": method, "params": params})
        while True:
            line = self.server.stdout.readline()
            if not line:
                raise RuntimeError("meshgrove-mcp ended")
            reply = json.loads(line)
            if reply.get("id") == self.count:
                return reply

    def tool(self, name: str, arguments: dict) -> str:
        reply = self.call("tools/call", {"name": name, "arguments": arguments})
        return "".join(part.get("text", "") for part in reply.get("result", {}).get("content", []))

    def close(self) -> None:
        self.server.stdin.close()
        try:
            self.server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.server.kill()


def queue_suggestions(mcp_program: str, env: dict, wanted: int) -> None:
    """Waits until the second Meshgrove has read its library, then queues the suggestions."""
    for _ in range(60):
        time.sleep(1)
        talk = Mcp(mcp_program, env)
        try:
            found = json.loads(talk.tool("search_models", {}) or "{}").get("items", [])
            if len(found) < wanted:
                continue
            for name, arguments in SUGGESTIONS:
                answer = talk.tool(name, arguments)
                state = "queued" if '"batch"' in answer or '"items"' in answer else answer[:200]
                print(f"  {name}: {state}")
            return
        except (ValueError, RuntimeError):
            continue
        finally:
            talk.close()
    print("The library was not read within a minute: no suggestions were queued.", file=sys.stderr)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--meshgrove", default=shutil.which("meshgrove") or str(Path.home() / ".local/bin/meshgrove"),
                        help="the Meshgrove program (default: the installed one)")
    parser.add_argument("--mcp", default=shutil.which("meshgrove-mcp") or str(Path.home() / ".local/bin/meshgrove-mcp"),
                        help="the meshgrove-mcp program (default: the installed one)")
    args = parser.parse_args()
    for program in (args.meshgrove, args.mcp):
        if not Path(program).exists():
            print(f"{program} is not there. Install Meshgrove, or name the programs.", file=sys.stderr)
            return 1

    language = "de" if os.environ.get("LANG", "").lower().startswith("de") else "en"
    with tempfile.TemporaryDirectory(prefix="meshgrove-panes-demo-") as home:
        home = Path(home)
        lib, incoming = make_library(home)
        config = home / "config" / "meshgrove"
        config.mkdir(parents=True)
        (home / "data").mkdir()
        (home / "cache").mkdir()
        (config / "config.toml").write_text(
            f'language = "{language}"\n'
            f"library_roots = ['{lib}']\n\n"
            "[import]\n"
            f"folders = ['{incoming}']\n"
            f"library = '{lib}'\n"
            'landing = "Import"\n'
            'at_start = "ask"\n\n'
            "[ai]\n"
            "enabled = true\n"
            "connect_apps = false\n"
            "review_button_always = true\n\n"
            "[[ai.libraries]]\n"
            f"root = '{lib}'\n"
            'access = "cloud"\n'
            "images = false\n",
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
        print("A second Meshgrove opens on a made-up library. Its suggestions are queued once it")
        print("has read the library; the ✨ button at the top right opens the review.")
        app = subprocess.Popen([args.meshgrove, str(lib)], env=env)
        models = sum(1 for _ in lib.rglob("*.stl"))
        try:
            queue_suggestions(args.mcp, env, models)
            print("Close the window to end the demo.")
            return app.wait()
        finally:
            if app.poll() is None:
                app.terminate()


if __name__ == "__main__":
    sys.exit(main())

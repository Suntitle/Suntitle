"""Record launch evidence, then exit on a finish file or timeout."""
import json
import os
from pathlib import Path
import sys
import time

args = sys.argv[1:]
Path(args[args.index("--report") + 1]).write_text(json.dumps({
    "args": args, "pid": os.getpid(), "executable": sys.executable,
    "entry": str(Path(__file__).resolve()), "cwd": str(Path.cwd()),
}))
deadline = time.monotonic() + 120
while not Path(args[args.index("--finish") + 1]).exists() and time.monotonic() < deadline:
    time.sleep(0.05)

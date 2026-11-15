"""Check that the release watch server really starts and serves its health endpoint."""
import os
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.request

root = Path(__file__).resolve().parents[1]
executable = root / "watch-server" / "target" / "release" / ("watch-server.exe" if os.name == "nt" else "watch-server")
process = subprocess.Popen([str(executable), "127.0.0.1:17878"])
try:
    for attempt in range(50):
        if process.poll() is not None:
            raise RuntimeError("Watch server exited before becoming healthy")
        try:
            with urllib.request.urlopen("http://127.0.0.1:17878/health", timeout=1) as response:
                if response.status != 200 or response.read().strip() != b"ok":
                    raise RuntimeError("Unexpected health response")
            print("Watch server health check passed")
            break
        except urllib.error.URLError:
            time.sleep(0.2)
    else:
        raise RuntimeError("Watch server did not become healthy")
finally:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()

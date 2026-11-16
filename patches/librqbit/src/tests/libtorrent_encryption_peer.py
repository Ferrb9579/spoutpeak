"""Local-only independent MSE interoperability peer, invoked by the Rust test."""
import json
from pathlib import Path
import sys
import time

import libtorrent as lt

root = Path(sys.argv[1])
role, transport = sys.argv[2:4]
tcp = transport == "tcp"
session = lt.session({
    "listen_interfaces": "127.0.0.1:0",
    "enable_dht": False,
    "enable_lsd": False,
    "enable_upnp": False,
    "enable_natpmp": False,
    "enable_incoming_tcp": tcp,
    "enable_outgoing_tcp": tcp,
    "enable_incoming_utp": not tcp,
    "enable_outgoing_utp": not tcp,
    "in_enc_policy": 0,  # pe_forced
    "out_enc_policy": 0,
    "allowed_enc_level": 2,  # pe_rc4
    "prefer_rc4": True,
    "allow_multiple_connections_per_ip": True,
})
params = lt.add_torrent_params()
params.ti = lt.torrent_info(str(root / "payload.torrent"))
params.save_path = str(root / ("seed" if role == "seed" else "python-download"))
params.flags &= ~lt.torrent_flags.paused
params.flags &= ~lt.torrent_flags.auto_managed
handle = session.add_torrent(params)
deadline = time.monotonic() + 25
connected = False
encrypted = False


def signal(name, value):
    # Atomic rename keeps the Rust process from observing partial JSON.
    temporary = root / (name + ".tmp")
    temporary.write_text(json.dumps(value))
    temporary.replace(root / name)


while time.monotonic() < deadline:
    status = handle.status()
    if status.errc.value():
        raise RuntimeError(status.errc.message())
    if not (root / "ready.json").exists() and (role != "seed" or status.is_seeding):
        signal("ready.json", {"port": session.listen_port()})
    if role == "download" and not connected and (root / "connect.json").exists():
        port = json.loads((root / "connect.json").read_text())
        handle.connect_peer(("127.0.0.1", port))
        connected = True
    for peer in handle.get_peer_info():
        if peer.flags & lt.peer_info.rc4_encrypted:
            encrypted = True
            signal("encrypted", True)
    if role == "download" and status.is_seeding and encrypted:
        signal("completed", True)
    if (root / "stop").exists():
        break
    time.sleep(0.01)
else:
    raise RuntimeError("timed out waiting for an encrypted transfer")

"""Helpers for tools/linux-e2e.sh, standard library only: replay a recorded capture onto loopback, and talk to
the app's two sockets as the site and as the window page would.

  linux_e2e.py inject <pcap>                       send each Ethernet frame of a pcap out on lo (needs root)
  linux_e2e.py site <ws url> <origin>              the site's socket: print what the first message holds
  linux_e2e.py refused <ws url> <origin>           exit 0 if the socket refuses this origin
  linux_e2e.py page <http url>                     GET the window page; print the status and whether it is the page
  linux_e2e.py window <ws url> <origin> [msg...]   the window's socket: print each state, sending msg after each
  linux_e2e.py until <ws url> <origin> <regex> <s> the window's socket: wait for a state whose update line matches
  linux_e2e.py events <ws url> <origin> <s>        the site's socket: print each message's event (and relic count)
"""
import base64
import json
import os
import re
import socket
import struct
import sys
import time
import urllib.parse


def inject(pcap_path):
    data = open(pcap_path, "rb").read()
    assert data[:4] == b"\xd4\xc3\xb2\xa1", "a little-endian microsecond pcap"
    out = socket.socket(socket.AF_PACKET, socket.SOCK_RAW)
    out.bind(("lo", 0))
    at, sent = 24, 0
    while at + 16 <= len(data):
        captured = struct.unpack_from("<I", data, at + 8)[0]
        out.send(data[at + 16:at + 16 + captured])
        at += 16 + captured
        sent += 1
    print(f"injected {sent} packets")


def connect(url, origin):
    u = urllib.parse.urlsplit(url)
    s = socket.create_connection((u.hostname, u.port), timeout=10)
    key = base64.b64encode(os.urandom(16)).decode()
    path = u.path + ("?" + u.query if u.query else "")
    request = (
        f"GET {path} HTTP/1.1\r\nHost: {u.hostname}:{u.port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nOrigin: {origin}\r\n\r\n"
    )
    s.sendall(request.encode())
    head = b""
    while b"\r\n\r\n" not in head:
        chunk = s.recv(1)
        if not chunk:
            break
        head += chunk
    return s, int(head.split(b" ", 2)[1])


def exact(s, n):
    out = b""
    while len(out) < n:
        chunk = s.recv(n - len(out))
        if not chunk:
            raise EOFError("socket closed")
        out += chunk
    return out


def receive(s):
    """One whole message (continuation frames joined), as text."""
    message = b""
    while True:
        b1, b2 = exact(s, 2)
        n = b2 & 0x7F
        if n == 126:
            n = struct.unpack(">H", exact(s, 2))[0]
        elif n == 127:
            n = struct.unpack(">Q", exact(s, 8))[0]
        message += exact(s, n)
        if b1 & 0x80:
            return message.decode()


def send(s, text):
    data = text.encode()
    mask = os.urandom(4)
    n = len(data)
    header = bytes([0x81]) + (bytes([0x80 | n]) if n < 126 else bytes([0x80 | 126]) + struct.pack(">H", n))
    s.sendall(header + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))


def shown(state):
    """The parts of a window state the tests look at, on one line."""
    update = state.get("update") or {}
    keys = ("headline", "detail", "tone", "installed", "sending", "startup", "startupLabel", "version")
    return json.dumps({k: state.get(k) for k in keys}
                      | {"relics": (state.get("summary") or {}).get("relics"),
                         "update": update.get("text"), "updateAction": update.get("action")})


def main():
    command, args = sys.argv[1], sys.argv[2:]
    if command == "inject":
        inject(args[0])
    elif command == "site":
        s, status = connect(args[0], args[1])
        assert status == 101, f"status {status}"
        message = json.loads(receive(s))
        data = message["data"]
        print(json.dumps({
            "event": message["event"],
            "relics": len(data.get("relics", [])),
            "light_cones": len(data.get("light_cones", [])),
            "characters": len(data.get("characters", [])),
            "trailblazer": data.get("metadata", {}).get("trailblazer"),
        }))
    elif command == "refused":
        _, status = connect(args[0], args[1])
        print(f"status {status}")
        sys.exit(0 if status == 403 else 1)
    elif command == "page":
        u = urllib.parse.urlsplit(args[0])
        s = socket.create_connection((u.hostname, u.port), timeout=10)
        host = args[1] if len(args) > 1 else f"{u.hostname}:{u.port}"
        s.sendall(f"GET {u.path}?{u.query} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n".encode())
        response = b""
        while chunk := s.recv(65536):
            response += chunk
        status = int(response.split(b" ", 2)[1])
        print(json.dumps({"status": status, "page": b"<title>Nous Relic Importer</title>" in response,
                          "no_referrer": b"referrer-policy: no-referrer" in response.lower()}))
    elif command == "window":
        s, status = connect(args[0], args[1])
        assert status == 101, f"status {status}"
        for reply in [None, *args[2:]]:
            if reply is not None:
                send(s, reply)
            try:
                state = json.loads(receive(s))
            except (EOFError, ConnectionError):
                print(json.dumps({"closed": True}))
                break
            print(shown(state))
    elif command == "until":
        s, status = connect(args[0], args[1])
        assert status == 101, f"status {status}"
        pattern, deadline = re.compile(args[2]), time.time() + float(args[3])
        while time.time() < deadline:
            try:
                state = json.loads(receive(s))
            except socket.timeout:
                continue
            if pattern.search((state.get("update") or {}).get("text") or ""):
                print(shown(state))
                return
        sys.exit(f"no update line matching {args[2]!r} within {args[3]} s")
    elif command == "events":
        s, status = connect(args[0], args[1])
        assert status == 101, f"status {status}"
        deadline = time.time() + float(args[2])
        s.settimeout(0.5)
        while time.time() < deadline:
            try:
                message = json.loads(receive(s))
            except socket.timeout:
                continue
            data = message.get("data")
            relics = f":{len(data.get('relics', []))}" if isinstance(data, dict) and "relics" in data else ""
            print(message.get("event") + relics, flush=True)
    else:
        sys.exit(f"unknown command {command}")


if __name__ == "__main__":
    main()

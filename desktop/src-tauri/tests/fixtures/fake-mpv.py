#!/usr/bin/python3
"""Deterministic local IPC peer for transport tests; not a player substitute."""
import json
import os
import socket
import sys
import time

address = next(arg.split("=", 1)[1] for arg in sys.argv if arg.startswith("--input-ipc-server="))
server = socket.socket(socket.AF_UNIX)
server.bind(address)
server.listen(1)
connection, _ = server.accept()
position, paused, speed, updated = 0.0, True, 1.0, time.monotonic()
buffer = b""
while True:
    chunk = connection.recv(4096)
    if not chunk:
        break
    buffer += chunk
    while b"\n" in buffer:
        line, buffer = buffer.split(b"\n", 1)
        request = json.loads(line)
        command = request["command"]
        now = time.monotonic()
        if not paused:
            position += (now - updated) * speed
        updated = now
        data = None
        if command[0] == "get_property":
            data = {"time-pos": position, "pause": paused, "duration": 120.0}.get(command[1])
        elif command[0] == "set_property":
            if command[1] == "pause":
                paused = command[2]
            elif command[1] == "speed":
                speed = command[2]
        elif command[0] == "seek":
            position = command[1]
            connection.sendall(b'{"event":"seek"}\n')
        else:
            raise RuntimeError("Unexpected command: " + repr(command))
        response = json.dumps({"request_id": request.get("request_id"), "error": "success", "data": data}).encode() + b"\n"
        # Exercise fragmentation across reads, not just one-line read assumptions.
        connection.sendall(response[:7])
        time.sleep(0.002)
        connection.sendall(response[7:])

#!/usr/bin/env python3
"""Run the actual smoke upload header assignment against five owned responses."""
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
import errno
import hashlib
import json
import os
import re
import socket
import subprocess
import tempfile
import threading
import time
import unittest


def starttick(pid):
    try:
        return int(Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()[19])
    except FileNotFoundError:
        return None


class HeaderDrainTest(unittest.TestCase):
    def test_actual_upload_assignment_preserves_first_etag_and_http_failure(self):
        lines = Path(__file__).with_name("install-smoke.sh").read_text().splitlines(keepends=True)
        starts = [i for i, line in enumerate(lines) if line.startswith('ETAG="$(')]
        self.assertEqual(len(starts), 1)
        first = last = starts[0]
        while lines[last].rstrip("\n").endswith("\\"):
            last += 1
        assignment = "".join(lines[first:last + 1])
        command = 'set -euo pipefail\n' + assignment + '\nprintf "%s\\n" "$ETAG"\n'
        for case in ("ordinary", "delayed", "duplicate", "missing", "http503"):
            with self.subTest(case=case), tempfile.TemporaryDirectory(prefix="fvoci-header-test-") as temp:
                class Response(BaseHTTPRequestHandler):
                    def log_message(self, *args):
                        pass

                    def do_PUT(self):
                        self.rfile.read(int(self.headers["Content-Length"]))
                        status = b"503 Service Unavailable" if case == "http503" else b"200 OK"
                        etag = b"" if case in ("missing", "http503") else b'etag: "fixture-first"\r\n'
                        prefix = b"HTTP/1.1 " + status + b"\r\n" + etag
                        duplicate = b'etag: "fixture-second"\r\n' if case == "duplicate" else b""
                        tail = duplicate + b"Content-Length: 0\r\nConnection: close\r\n\r\n"
                        try:
                            if case == "delayed":
                                padding = b"".join(b"X-Owned-Padding: " + b"a" * 512 + b"\r\n" for _ in range(32))
                                self.connection.sendall(prefix + padding)
                                time.sleep(.15)
                                self.connection.sendall(tail)
                            else:
                                self.connection.sendall(prefix + tail)
                        except (BrokenPipeError, ConnectionResetError):
                            pass  # Expected with the old early-exiting consumer; curl exit stays observable.

                root = Path(temp)
                body = root / "body"
                body.write_bytes(b"owned-fixture")
                cookie = root / "empty-cookie"
                cookie.write_bytes(b"")
                cookie.chmod(0o600)
                server = HTTPServer(("127.0.0.1", 0), Response)
                server.timeout = 5
                port = server.server_port
                listener_fd = server.fileno()
                listener_link = os.readlink(f"/proc/self/fd/{listener_fd}")
                probe = socket.socket()
                probe.bind(("127.0.0.1", 0))  # Reserve a distinct source while the listener is alive.
                source = probe.getsockname()
                thread = threading.Thread(target=server.handle_request)
                thread.start()
                env = {"PATH": os.environ["PATH"], "LC_ALL": "C", "BASE_URL": f"http://127.0.0.1:{port}",
                       "PART_URL": "/owned-fixture", "ORIGIN": f"http://127.0.0.1:{port}",
                       "COOKIE_JAR": str(cookie), "FIXTURE_HWPX": str(body)}
                begin = datetime.now(timezone.utc).isoformat()
                proc = subprocess.Popen(["bash", "-c", command], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                tick = starttick(proc.pid)
                out, err = proc.communicate()
                thread.join(5)
                alive = thread.is_alive()
                server.server_close()
                closure = probe.connect_ex(("127.0.0.1", port))
                local = probe.getsockname()
                probe.close()
                value = out.decode().rstrip("\n")
                # Closed, synthetic-only receipt precedes every result assertion; never print URLs/cookies/errors.
                print(json.dumps({"case": case, "start_utc": begin, "end_utc": datetime.now(timezone.utc).isoformat(),
                    "script_assignment_sha256": hashlib.sha256(assignment.encode()).hexdigest(),
                    "command_pid": proc.pid, "command_starttick": tick, "command_exit": proc.returncode,
                    "server_owner_pid": os.getpid(), "server_owner_starttick": starttick(os.getpid()),
                    "server_thread_id": thread.native_id, "listener_fd": listener_fd, "listener_fd_link": listener_link,
                    "server_thread_reaped": not alive, "port": port, "probe_reserved_source": source,
                    "probe_local_after": local, "closure_connect_ex": closure, "etag": value,
                    "closed_curl_errors": [int(n) for n in re.findall(rb"curl: \((\d+)\)", err)]}), flush=True)
                self.assertFalse(alive)
                self.assertNotEqual(source[1], port)
                self.assertEqual(closure, errno.ECONNREFUSED)
                self.assertEqual(proc.returncode, 22 if case == "http503" else 0)
                self.assertEqual(value, "" if case in ("missing", "http503") else '"fixture-first"')


if __name__ == "__main__":
    unittest.main()

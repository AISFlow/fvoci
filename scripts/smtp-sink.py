#!/usr/bin/env python3
"""Plaintext SMTP sink for FVOCI mail tests. Binds 127.0.0.1:0, writes the port
and appends captured messages as JSON lines. No AUTH/TLS — matches the product
client. Do not log envelope secrets beyond the captured file the caller owns."""

from __future__ import annotations

import argparse
import email
import email.policy
import json
import socket
import sys
import threading
import time



def decoded_text(data: str) -> str:
    msg = email.message_from_string(data, policy=email.policy.default)
    body = msg.get_body(preferencelist=("plain",))
    content = body.get_content() if body is not None else ""
    return f"Subject: {msg.get('subject', '')}\n\n{content}"

def send_line(conn: socket.socket, line: str) -> None:
    conn.sendall(f"{line}\r\n".encode("utf-8"))


def handle_client(conn: socket.socket, capture_path: str) -> None:
    conn.settimeout(30)
    try:
        send_line(conn, "220 fvoci-smtp-sink")
        mail_from = ""
        rcpt_to = ""
        buf = b""
        while True:
            chunk = conn.recv(4096)
            if not chunk:
                break
            buf += chunk
            while b"\n" in buf:
                raw, buf = buf.split(b"\n", 1)
                line = raw.decode("utf-8", "replace").rstrip("\r")
                upper = line.upper()
                if upper.startswith("EHLO") or upper.startswith("HELO"):
                    send_line(conn, "250 fvoci")
                elif upper.startswith("MAIL FROM:"):
                    mail_from = line.split(":", 1)[1].strip().strip("<>")
                    send_line(conn, "250 ok")
                elif upper.startswith("RCPT TO:"):
                    rcpt_to = line.split(":", 1)[1].strip().strip("<>")
                    send_line(conn, "250 ok")
                elif upper == "DATA":
                    send_line(conn, "354 go")
                    data_lines: list[str] = []
                    while True:
                        while b"\n" not in buf:
                            more = conn.recv(4096)
                            if not more:
                                return
                            buf += more
                        raw, buf = buf.split(b"\n", 1)
                        body_line = raw.decode("utf-8", "replace").rstrip("\r")
                        if body_line == ".":
                            break
                        if body_line.startswith("."):
                            body_line = body_line[1:]
                        data_lines.append(body_line)
                    data = "\n".join(data_lines)
                    record = {
                        "from": mail_from,
                        "to": rcpt_to,
                        "data": data,
                        # Decoded as a mail client shows it (RFC 2047 subject, transfer encoding).
                        "text": decoded_text(data),
                        "ts": time.time(),
                    }
                    with open(capture_path, "a", encoding="utf-8") as out:
                        out.write(json.dumps(record, ensure_ascii=False) + "\n")
                        out.flush()
                    send_line(conn, "250 ok")
                elif upper == "QUIT":
                    send_line(conn, "221 bye")
                    return
                elif upper == "RSET":
                    mail_from = ""
                    rcpt_to = ""
                    send_line(conn, "250 ok")
                elif upper == "NOOP":
                    send_line(conn, "250 ok")
                else:
                    send_line(conn, "250 ok")
    except OSError:
        return
    finally:
        try:
            conn.close()
        except OSError:
            pass


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--capture", required=True)
    parser.add_argument("--port-file", required=True)
    args = parser.parse_args()
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(32)
    port = listener.getsockname()[1]
    with open(args.port_file, "w", encoding="utf-8") as handle:
        handle.write(str(port))
    print(f"smtp sink listening on 127.0.0.1:{port}", file=sys.stderr, flush=True)
    while True:
        conn, _addr = listener.accept()
        thread = threading.Thread(
            target=handle_client, args=(conn, args.capture), daemon=True
        )
        thread.start()


if __name__ == "__main__":
    raise SystemExit(main())

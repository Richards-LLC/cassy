#!/usr/bin/env python3
"""cas-70a8 independent QA round 2: interactive / -y `cas update` against a loopback fake release.

Role-preserving: every CAS factory identity variable (role, name, session,
factory session/mode, CLONE_PATH, supervisor) is inherited unchanged. Only
HOME/XDG/TMPDIR, the project registry (CAS_PROJECT_ROOTS), CAS_ROOT and the
credentials file are pointed at a fresh physical scratch child; VIOLET_* and
Git context variables are removed. Refuses to run as a factory worker (the
binary-install path belongs to the supervisor; no role spoofing).

The loopback server offers v9.9.9 via the debug-only CAS_UPDATE_GITHUB_API_URL.
The asset always fails: HTTP 500, or a body that downloads and then fails
extraction (garbage = 21 B instantly; slow = 256 KiB streamed over ~2 s so the
live download line animates). Nothing is ever installed. The binary is a
private copy; its version must be 4cedda1 and its sha256 is checked before/after.

Usage: accept-proof.py <cas-copy> <out-prefix> <mode y|n|yes|pipe> [cols]
Env: ASSET=500|garbage|slow  LC=<LC_ALL value>  NOCOLOR=1
"""
import fcntl, hashlib, http.server, json, os, pty, re, select, struct, subprocess, sys, tempfile, termios, threading, time

SCRATCH = "/mnt/rewind/cas/scratch/qa70a8r2/runs"
cas, out, mode = sys.argv[1], sys.argv[2], sys.argv[3]
cols = int(sys.argv[4]) if len(sys.argv) > 4 else 80
ASSET = os.environ.get("ASSET", "garbage")
if os.environ.get("CAS_AGENT_ROLE", "").strip().lower() == "worker":
    sys.exit("accept-proof.py: refusing to run as a factory worker; the supervisor executes this driver")
ver = subprocess.run([cas, "--version"], capture_output=True, text=True).stdout.strip()
if "4cedda1" not in ver and os.environ.get("ALLOW_BASE") != "1":
    sys.exit(f"accept-proof.py: {cas} reports {ver!r}, not 4cedda1")

os.makedirs(SCRATCH, exist_ok=True)
home = os.path.realpath(tempfile.mkdtemp(prefix="run-", dir=SCRATCH))
assert os.path.dirname(home) == os.path.realpath(SCRATCH)
for d in ("cwd", "projects", "tmp", ".config", ".cache", ".local/share", ".cas"):
    os.makedirs(os.path.join(home, d), exist_ok=True)
cwd = os.path.join(home, "cwd")

ident_keys = ["CAS_AGENT_ROLE", "CAS_AGENT_NAME", "CAS_SESSION_ID", "CAS_FACTORY_SESSION",
              "CAS_FACTORY_MODE", "CAS_CLONE_PATH", "CAS_SUPERVISOR_NAME"]
identity = {k: os.environ.get(k) for k in ident_keys}
env = os.environ.copy()
for k in list(env):
    if k.startswith(("VIOLET_", "GIT_CONFIG_")) or k in {"GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"}:
        del env[k]
env.update(HOME=home, XDG_CONFIG_HOME=f"{home}/.config", XDG_CACHE_HOME=f"{home}/.cache",
           XDG_DATA_HOME=f"{home}/.local/share", TMPDIR=f"{home}/tmp",
           CAS_PROJECT_ROOTS=f"{home}/projects", CAS_ROOT=f"{home}/.cas",
           CAS_CREDENTIALS_FILE=f"{home}/credentials.env",
           GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", TERM="xterm-256color", COLUMNS=str(cols))
if os.environ.get("LC"):
    env["LC_ALL"] = os.environ["LC"]
if os.environ.get("NOCOLOR"):
    env["NO_COLOR"] = "1"
if os.environ.get("COLOR"):
    # Colour/Unicode control: drop inherited colour suppression only.
    for k in ("NO_COLOR", "CLICOLOR", "TERM_PROGRAM_NO_COLOR"):
        env.pop(k, None)
    env.setdefault("LANG", "C.UTF-8")
assert all(env.get(k) == v for k, v in identity.items() if v is not None)
requests = []


class Release(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        requests.append(self.path)
        base = f"http://127.0.0.1:{self.server.server_port}"
        if self.path.startswith("/asset"):
            if ASSET in ("garbage", "slow"):
                body = b"this is not a tarball" if ASSET == "garbage" else b"x" * (256 * 1024)
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                if ASSET == "slow":
                    for i in range(0, len(body), 16 * 1024):
                        self.wfile.write(body[i:i + 16 * 1024]); self.wfile.flush(); time.sleep(0.125)
                else:
                    self.wfile.write(body)
                return
            body = b"deliberate QA asset failure"
            self.send_response(int(ASSET))
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        release = {"tag_name": "v9.9.9", "name": "9.9.9", "created_at": "2026-10-17T00:00:00Z",
                   "assets": [{"name": "cas-x86_64-unknown-linux-gnu.tar.gz", "url": f"{base}/asset",
                               "browser_download_url": f"{base}/asset"}]}
        body = json.dumps([release] if self.path.endswith("/releases") else release).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def sha(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Release)
threading.Thread(target=server.serve_forever, daemon=True).start()
env["CAS_UPDATE_GITHUB_API_URL"] = f"http://127.0.0.1:{server.server_port}"
before = sha(cas)
argv = [cas, "update"] + (["-y"] if mode in ("yes", "pipe") else [])
stream, answered, code, t0 = b"", None, None, time.time()
if mode == "pipe":
    r = subprocess.run(argv, capture_output=True, timeout=180, env=env, cwd=cwd)
    stream, code = r.stdout + b"\n--- stderr ---\n" + r.stderr, r.returncode
else:
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(cwd)
        os.execve(cas, argv, env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, cols, 0, 0))
    deadline = time.time() + 150
    while time.time() < deadline:
        ready, _, _ = select.select([fd], [], [], 0.2)
        if ready:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            stream += chunk
        if mode in ("y", "n") and answered is None and re.search(rb"\[Y/n\]\s*$", re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b"", stream).rstrip()):
            time.sleep(0.3)
            os.write(fd, mode.encode() + b"\r")
            answered = mode
    _, status = os.waitpid(pid, 0)
    code = os.waitstatus_to_exitcode(status)
server.shutdown()
after = sha(cas)

open(out + ".ansi", "wb").write(stream)
plain = re.sub(rb"\x1b\][^\x07\x1b]*(\x07|\x1b\\)", b"", stream)
plain = re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b"", plain).decode("utf-8", "replace")
# Final terminal state per line: text after the last carriage return.
lines = [line.rstrip("\r").split("\r")[-1] for line in plain.split("\n")]
open(out + ".txt", "w").write("\n".join(lines))
summary = {"version": ver, "inherited_no_color": "NO_COLOR" in os.environ, "child_no_color": env.get("NO_COLOR"), "mode": mode, "asset": ASSET, "cols": cols, "answered": answered, "exit": code,
           "seconds": round(time.time() - t0, 2), "requests": requests,
           "asset_requested": any(p.startswith("/asset") for p in requests),
           "binary_unchanged": before == after, "sha256": before, "cwd": cwd, "home": home,
           "identity_preserved": identity, "widest": max((len(l) for l in lines), default=0),
           "cr_redraws": stream.count(b"\r") - stream.count(b"\r\n"),
           "non_ascii": sorted({hex(ord(c)) for l in lines for c in l if ord(c) > 127})[:20]}
open(out + ".json", "w").write(json.dumps(summary, indent=1))
print(json.dumps(summary))

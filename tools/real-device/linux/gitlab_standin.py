#!/usr/bin/env python3
"""A loopback stand-in for the GitLab v4 API, enough for bridgewatch's poller and wizard.

gitlab_standin.py <workdir> <name> [port]

Every request is appended to <workdir>/<name>.requests.jsonl with its method, path and
headers (the token header is recorded as-is: every token here is made up).
State is <workdir>/<name>.state.json, re-read on every request, so the harness changes a
pipeline by rewriting that file. Shape:
  {"token_expires_at": "YYYY-MM-DD" | null,
   "projects": {"101": {"path": "acme/web", "pipeline": 9101, "sha": "<40 hex>",
                        "jobs": [["build", "build", "success"], ["test", "test:unit", "failed"]]}}}
The pipeline status is derived from the jobs (any running/pending -> running, any failed
not allowed to fail -> failed, else success).
"""
import http.server
import json
import pathlib
import socketserver
import sys
import time
import urllib.parse

work = pathlib.Path(sys.argv[1])
name = sys.argv[2]
port = int(sys.argv[3]) if len(sys.argv) > 3 else 0
NOW = "2026-11-07T12:00:00.000Z"


def state():
    return json.loads((work / f"{name}.state.json").read_text())


def pstatus(jobs):
    st = [j[2] for j in jobs]
    if any(s in ("running", "pending", "created") for s in st):
        return "running"
    if "failed" in st:
        return "failed"
    return "success"


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def origin(self):
        return f"http://127.0.0.1:{self.server.server_address[1]}"

    def send(self, code, body=None, headers=()):
        data = b"" if body is None else json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def record(self):
        with open(work / f"{name}.requests.jsonl", "a", encoding="utf-8") as out:
            out.write(json.dumps({"t": time.time(), "method": self.command, "path": self.path,
                                  "headers": dict(self.headers.items())}) + "\n")

    def do_POST(self):
        self.record()
        self.send(404, {"message": "404 Not Found"})

    def do_GET(self):
        self.record()
        u = urllib.parse.urlparse(self.path)
        p = u.path
        if not p.startswith("/api/v4"):
            return self.send(404, {"message": "404 Not Found"})
        p = p[len("/api/v4"):]
        s = state()
        o = self.origin()
        parts = [urllib.parse.unquote(x) for x in p.strip("/").split("/")]

        def proj(key):
            for pid, pr in s["projects"].items():
                if key == pid or key == pr["path"]:
                    return int(pid), pr
            return None, None

        def pipe(pid, pr):
            st = pstatus(pr["jobs"])
            return {"id": pr["pipeline"], "iid": pr["pipeline"] % 1000, "project_id": pid, "sha": pr["sha"],
                    "ref": "main", "status": st, "source": "push",
                    "web_url": f"{o}/{pr['path']}/-/pipelines/{pr['pipeline']}",
                    "created_at": "2026-11-07T11:50:00.000Z", "updated_at": NOW,
                    "started_at": "2026-11-07T11:50:01.000Z",
                    "finished_at": None if st == "running" else "2026-11-07T11:58:00.000Z"}

        def jobs(pid, pr):
            out = []
            for i, (stage, jname, jst) in enumerate(pr["jobs"]):
                jid = pr["pipeline"] * 100 + i
                out.append({"id": jid, "name": jname, "stage": stage, "status": jst, "allow_failure": False,
                            "started_at": "2026-11-07T11:50:05.000Z",
                            "finished_at": None if jst in ("running", "pending") else "2026-11-07T11:55:00.000Z",
                            "duration": 295.0, "web_url": f"{o}/{pr['path']}/-/jobs/{jid}",
                            "failure_reason": "script_failure" if jst == "failed" else None,
                            "pipeline": {"id": pr["pipeline"]}})
            return out

        if parts == ["user"]:
            return self.send(200, {"id": 42, "username": "standin-user", "name": "Stand-in User", "bot": False})
        if parts == ["personal_access_tokens", "self"]:
            return self.send(200, {"id": 7, "name": "bridgewatch", "scopes": ["read_api"],
                                   "expires_at": s.get("token_expires_at"), "active": True,
                                   "revoked": False, "user_id": 42})
        if parts == ["projects"]:
            rows = [{"id": int(pid), "path_with_namespace": pr["path"], "name": pr["path"].split("/")[-1],
                     "default_branch": "main", "web_url": f"{o}/{pr['path']}",
                     "last_activity_at": pr.get("last_activity_at", NOW)}
                    for pid, pr in s["projects"].items()]
            rows.sort(key=lambda r: r["last_activity_at"], reverse=True)
            return self.send(200, rows, [("x-next-page", ""), ("x-total-pages", "1")])
        if len(parts) >= 2 and parts[0] == "projects":
            pid, pr = proj(parts[1])
            if pr is None:
                return self.send(404, {"message": "404 Project Not Found"})
            rest = parts[2:]
            if rest == []:
                return self.send(200, {"id": pid, "path_with_namespace": pr["path"], "name": pr["path"].split("/")[-1],
                                       "default_branch": "main", "web_url": f"{o}/{pr['path']}",
                                       "last_activity_at": pr.get("last_activity_at", NOW)})
            if rest == ["pipelines"]:
                return self.send(200, [pipe(pid, pr)], [("x-next-page", "")])
            if len(rest) == 2 and rest[0] == "pipelines" and rest[1] == str(pr["pipeline"]):
                return self.send(200, pipe(pid, pr))
            if len(rest) == 3 and rest[0] == "pipelines" and rest[2] == "jobs":
                return self.send(200, jobs(pid, pr), [("x-next-page", "")])
            if len(rest) == 3 and rest[0] == "pipelines" and rest[2] == "bridges":
                return self.send(200, [], [("x-next-page", "")])
            if len(rest) == 3 and rest[:2] == ["repository", "commits"]:
                return self.send(200, {"id": pr["sha"]})
        return self.send(404, {"message": "404 Not Found"})

    def log_message(self, *args):
        pass


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True

    def server_bind(self):
        socketserver.TCPServer.server_bind(self)
        self.server_name = "127.0.0.1"
        self.server_port = self.server_address[1]


srv = Server(("127.0.0.1", port), Handler)
(work / f"{name}.port").write_text(str(srv.server_address[1]))
srv.serve_forever()

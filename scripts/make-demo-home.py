#!/usr/bin/env python3
"""Create a fake home directory with realistic Claude Code, Codex CLI and
Gemini CLI logs, for trying aitop or taking screenshots:

    python3 scripts/make-demo-home.py /tmp/aitop-demo
    HOME=/tmp/aitop-demo aitop --no-history
"""

import hashlib
import json
import os
import random
import sys
import uuid
from datetime import datetime, timedelta, timezone

random.seed(7)
home = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "aitop-demo")
now = datetime.now(timezone.utc)


def iso(t):
    return t.strftime("%Y-%m-%dT%H:%M:%S.") + f"{t.microsecond // 1000:03d}Z"


def repo(name):
    path = os.path.join(home, "code", name)
    os.makedirs(os.path.join(path, ".git"), exist_ok=True)
    return path


def sessions(days, per_day):
    """Yield session start times spread over the last `days` days."""
    for d in range(days, -1, -1):
        for _ in range(random.randint(0, per_day)):
            yield now - timedelta(days=d, hours=random.uniform(0, 10), minutes=random.uniform(0, 59))


# ── Claude Code ────────────────────────────────────────────────────────────
claude_projects = {
    "webapp": 1.0,
    "api-server": 0.7,
    "aitop": 0.5,
    "infra": 0.25,
    "dotfiles": 0.1,
}
models = ["claude-opus-5-5"] * 6 + ["claude-sonnet-5-5"] * 3 + ["claude-haiku-5-5"]
root = os.path.join(home, ".claude", "projects")
for name, weight in claude_projects.items():
    cwd = repo(name)
    enc = os.path.join(root, cwd.replace("/", "-"))
    os.makedirs(enc, exist_ok=True)
    for start in sessions(29, max(1, int(3 * weight))):
        sid = str(uuid.uuid4())
        model = random.choice(models)
        t = start
        lines = []
        ctx = 20_000
        for _ in range(random.randint(8, int(60 * weight) + 10)):
            t += timedelta(seconds=random.uniform(5, 90))
            if t > now:
                break
            ctx += random.randint(500, 6000)
            out = random.randint(80, 4000)
            mid, rid = "msg_" + uuid.uuid4().hex[:24], "req_" + uuid.uuid4().hex[:24]
            usage = {
                "input_tokens": random.randint(1, 50),
                "cache_creation_input_tokens": random.randint(200, 8000),
                "cache_read_input_tokens": ctx,
                "output_tokens": out,
                "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 0},
            }
            usage["cache_creation"]["ephemeral_1h_input_tokens"] = usage["cache_creation_input_tokens"]
            lines.append({"type": "user", "timestamp": iso(t), "cwd": cwd, "sessionId": sid, "message": {"role": "user", "content": "…"}})
            # streamed responses repeat the usage on every content block
            for _ in range(random.randint(1, 3)):
                lines.append({
                    "type": "assistant", "timestamp": iso(t), "cwd": cwd, "sessionId": sid, "requestId": rid,
                    "message": {"id": mid, "model": model, "role": "assistant", "content": [{"type": "text", "text": "…"}], "usage": usage},
                })
        with open(os.path.join(enc, sid + ".jsonl"), "w") as f:
            f.writelines(json.dumps(l) + "\n" for l in lines)

# A session that is running right now.
cwd = repo("webapp")
sid = str(uuid.uuid4())
with open(os.path.join(root, cwd.replace("/", "-"), sid + ".jsonl"), "w") as f:
    t = now - timedelta(minutes=40)
    ctx = 30_000
    while t < now - timedelta(seconds=20):
        t += timedelta(seconds=random.uniform(20, 70))
        ctx += random.randint(1000, 4000)
        u = {"input_tokens": 3, "cache_creation_input_tokens": 2500, "cache_read_input_tokens": ctx, "output_tokens": random.randint(200, 3000)}
        f.write(json.dumps({"type": "assistant", "timestamp": iso(min(t, now)), "cwd": cwd, "sessionId": sid,
                            "requestId": uuid.uuid4().hex, "message": {"id": uuid.uuid4().hex, "model": "claude-opus-5-5", "usage": u}}) + "\n")

# ── Codex CLI ──────────────────────────────────────────────────────────────
for name, weight, model in [("api-server", 1.0, "gpt-5.3-codex"), ("data-pipeline", 0.6, "gpt-5.1-codex"), ("webapp", 0.3, "gpt-5.1-codex-mini")]:
    cwd = repo(name)
    for start in sessions(20, max(1, int(2 * weight))):
        sid = str(uuid.uuid4())
        day = os.path.join(home, ".codex", "sessions", start.strftime("%Y/%m/%d"))
        os.makedirs(day, exist_ok=True)
        t = start
        lines = [
            {"timestamp": iso(t), "type": "session_meta", "payload": {"id": sid, "timestamp": iso(t), "cwd": cwd, "originator": "codex_cli_rs", "cli_version": "0.60.0"}},
            {"timestamp": iso(t), "type": "turn_context", "payload": {"cwd": cwd, "model": model, "effort": "medium"}},
        ]
        tot = {"input_tokens": 0, "cached_input_tokens": 0, "output_tokens": 0, "reasoning_output_tokens": 0, "total_tokens": 0}
        for _ in range(random.randint(5, 40)):
            t += timedelta(seconds=random.uniform(10, 120))
            if t > now:
                break
            inp = random.randint(8_000, 60_000)
            cached = int(inp * random.uniform(0.5, 0.9))
            out = random.randint(200, 5000)
            reas = int(out * random.uniform(0.2, 0.6))
            tot = {k: tot[k] + v for k, v in {"input_tokens": inp, "cached_input_tokens": cached, "output_tokens": out, "reasoning_output_tokens": reas, "total_tokens": inp + out}.items()}
            lines.append({"timestamp": iso(t), "type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": dict(tot), "model_context_window": 272000},
                          "rate_limits": {"primary": {"used_percent": round(random.uniform(10, 70), 1), "window_minutes": 300, "resets_at": int((now + timedelta(hours=2, minutes=17)).timestamp())},
                                          "secondary": {"used_percent": 23.0, "window_minutes": 10080, "resets_at": int((now + timedelta(days=3, hours=5)).timestamp())}}}})
        with open(os.path.join(day, f"rollout-{start.strftime('%Y-%m-%dT%H-%M-%S')}-{sid}.jsonl"), "w") as f:
            f.writelines(json.dumps(l) + "\n" for l in lines)

# ── Gemini CLI ─────────────────────────────────────────────────────────────
for name, model in [("research-notes", "gemini-2.5-pro"), ("webapp", "gemini-3-flash-preview")]:
    cwd = repo(name)
    chats = os.path.join(home, ".gemini", "tmp", hashlib.sha256(cwd.encode()).hexdigest(), "chats")
    os.makedirs(chats, exist_ok=True)
    for start in sessions(14, 1):
        sid = str(uuid.uuid4())
        t = start
        msgs = []
        ctx = 8000
        for i in range(random.randint(3, 20)):
            t += timedelta(seconds=random.uniform(10, 60))
            if t > now:
                break
            ctx += random.randint(500, 3000)
            msgs.append({"id": f"u{i}", "timestamp": iso(t), "type": "user", "content": "…"})
            out = random.randint(100, 2500)
            th = random.randint(0, 800)
            cached = int(ctx * 0.6)
            msgs.append({"id": f"g{i}", "timestamp": iso(t), "type": "gemini", "content": "…", "model": model,
                         "tokens": {"input": ctx, "output": out, "cached": cached, "thoughts": th, "tool": 0, "total": ctx + out + th}})
        doc = {"sessionId": sid, "projectHash": os.path.basename(os.path.dirname(chats)), "startTime": iso(start), "lastUpdated": iso(t), "messages": msgs}
        with open(os.path.join(chats, f"session-{start.strftime('%Y-%m-%dT%H-%M')}-{sid[:8]}.json"), "w") as f:
            json.dump(doc, f)

print(home)

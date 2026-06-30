# Hermes Agent External Monitoring API

This document describes how external applications can access real-time information about the Hermes agent's state, token usage, and activity.

**IMPORTANT FOR IMPLEMENTATION AGENTS**: This document is self-contained. The implementing agent will NOT have access to the Hermes codebase repository. All necessary schema, paths, and attribute names are documented here.

---

## Critical Paths and Configuration

**All paths are relative to `HERMES_HOME` which defaults to:**
- Linux/macOS: `~/.hermes` (e.g., `/home/username/.hermes` or `/Users/username/.hermes`)
- Windows: `%USERPROFILE%\.hermes` (e.g., `C:\Users\username\.hermes`)
- WSL: `/mnt/c/Users/username/.hermes` (Windows path accessed from Linux)

| Resource | Relative Path | Absolute Path Example (Linux) | Description |
|----------|---------------|-------------------------------|-------------|
| State Database | `state.db` | `~/.hermes/state.db` | SQLite database with session data |
| Main Log | `logs/agent.log` | `~/.hermes/logs/agent.log` | All agent activity logs |
| Error Log | `logs/errors.log` | `~/.hermes/logs/errors.log` | Warnings and errors only |
| Gateway Log | `logs/gateway.log` | `~/.hermes/logs/gateway.log` | Gateway-specific events |
| GUI Log | `logs/gui.log` | `~/.hermes/logs/gui.log` | Dashboard/websocket events |
| Config File | `config.yaml` | `~/.hermes/config.yaml` | Agent configuration |
| Auth File | `auth.json` | `~/.hermes/auth.json` | Provider authentication tokens |

**Default Web Server Port**: `9119` (can be changed via `HERMES_WEB_PORT` environment variable)

**Important Notes**:
- The agent must run at least once to create these files
- If `state.db` doesn't exist, the agent hasn't been initialized
- Log files are created when the agent starts logging
- Files are updated in real-time as the agent operates

---

## Overview of Monitoring Methods

| Method | Real-Time | Push/Pull | Complexity | Best For |
|--------|-----------|-----------|------------|----------|
| SQLite Database | ~1-5s latency | Pull (polling) | Low | Simple integrations |
| Log File Watching | True real-time | Push (streaming) | Medium | Live event streaming |
| WebSocket API | True real-time | Push | Medium-High | Dashboard/web apps |
| REST API | Near real-time | Pull | Low-Medium | HTTP-based integrations |
| MCP Server | Varies | Pull | Medium | AI tool integrations |

---

## 1. SQLite Database (Recommended for Simplicity)

The agent persists session data to `~/.hermes/state.db`. Token counters are updated after each API call, with typical latency of 1-5 seconds from action to database update.

### Complete Database Schema

The `sessions` table contains all monitoring-relevant data:

| Column Name | Data Type | Description | Example Value |
|-------------|-----------|-------------|---------------|
| `id` | TEXT | Unique session identifier | `"sess_abc123def456"` |
| `source` | TEXT | Where session originated | `"cli"`, `"tui"`, `"gateway"`, `"telegram"` |
| `model` | TEXT | Model being used | `"claude-sonnet-4-6"`, `"gpt-4o"` |
| `input_tokens` | INTEGER | Total input/prompt tokens consumed | `12500` |
| `output_tokens` | INTEGER | Total output/completion tokens generated | `3400` |
| `cache_read_tokens` | INTEGER | Cache read tokens (Anthropic CDK cache) | `8000` |
| `cache_write_tokens` | INTEGER | Cache write tokens (cache creation) | `5000` |
| `reasoning_tokens` | INTEGER | Tokens used for model reasoning/thinking | `2000` |
| `api_call_count` | INTEGER | Number of API requests made in session | `15` |
| `message_count` | INTEGER | Total messages exchanged | `28` |
| `tool_call_count` | INTEGER | Total tool/function invocations | `12` |
| `estimated_cost_usd` | REAL | Estimated cost in USD | `0.45` |
| `actual_cost_usd` | REAL | Actual billed cost (may be null initially) | `0.42` |
| `started_at` | REAL | Unix timestamp (seconds since epoch) | `1704067200.123` |
| `ended_at` | REAL | Unix timestamp when session ended (NULL = active) | `NULL` or `1704153600.456` |
| `end_reason` | TEXT | Why session ended (NULL if active) | `"compression"`, `"branched"`, `"agent_close"`, `"error"` |
| `cwd` | TEXT | Working directory when session started | `/home/user/projects/myapp` |
| `git_branch` | TEXT | Git branch name | `"main"`, `"feature/x"` |
| `git_repo_root` | TEXT | Root of git repository | `/home/user/projects/myapp` |
| `title` | TEXT | User-assigned or auto-generated title | `"Fix database connection bug"` |
| `billing_provider` | TEXT | Billing provider | `"nous"`, `"anthropic"`, `"openai-codex"` |
| `cost_status` | TEXT | Cost tracking status | `"within_limit"`, `"approaching_limit"`, `"over_limit"` |

### Active Session Query (Most Important)

This query returns the currently running session:

```sql
SELECT 
    id,
    source,
    model,
    input_tokens,
    output_tokens,
    cache_read_tokens,
    cache_write_tokens,
    reasoning_tokens,
    api_call_count,
    message_count,
    tool_call_count,
    estimated_cost_usd,
    actual_cost_usd,
    started_at,
    ended_at,
    end_reason,
    cwd,
    git_branch,
    title,
    billing_provider,
    cost_status
FROM sessions 
WHERE ended_at IS NULL 
ORDER BY started_at DESC 
LIMIT 1;
```

**Interpretation**:
- If this query returns **no rows**: Agent is idle (no active session)
- If this query returns **a row**: Agent is actively processing

### Recent Sessions Query

```sql
SELECT 
    id,
    source,
    model,
    input_tokens,
    output_tokens,
    started_at,
    ended_at,
    end_reason,
    title
FROM sessions 
ORDER BY started_at DESC 
LIMIT 10;
```

### Python Client Implementation

Complete, ready-to-use Python client:

```python
import sqlite3
import time
from pathlib import Path
from typing import Optional, Dict, Any, Generator

class HermesMonitor:
    """
    Polls Hermes SQLite database for real-time agent status.
    
    Usage:
        monitor = HermesMonitor()
        status = monitor.get_active_session()
        if status:
            print(f"Active: {status['tokens']['total']} tokens used")
        else:
            print("Agent is idle")
    """
    
    def __init__(self, hermes_home: Optional[Path] = None):
        """
        Initialize monitor.
        
        Args:
            hermes_home: Path to .hermes directory. Defaults to ~/.hermes
        """
        if hermes_home:
            self.db_path = Path(hermes_home) / "state.db"
        else:
            # Platform detection
            home = Path.home()
            if Path("/mnt").exists() and (home / ".hermes").exists():
                # WSL scenario - use Windows path
                self.db_path = home / ".hermes" / "state.db"
            else:
                self.db_path = home / ".hermes" / "state.db"
    
    def _connect(self) -> sqlite3.Connection:
        """Create database connection with timeout."""
        conn = sqlite3.connect(str(self.db_path), timeout=10.0)
        conn.row_factory = sqlite3.Row
        return conn
    
    def get_active_session(self) -> Optional[Dict[str, Any]]:
        """
        Get current active session with all token and cost data.
        
        Returns:
            Dict with session data, or None if agent is idle.
            
        Structure:
            {
                "status": "active",
                "session_id": "sess_xxx",
                "model": "claude-sonnet-4-6",
                "source": "cli",
                "tokens": {
                    "input": 12500,
                    "output": 3400,
                    "cache_read": 8000,
                    "cache_write": 5000,
                    "reasoning": 2000,
                    "total": 15900  # input + output
                },
                "cost": {
                    "estimated_usd": 0.45,
                    "actual_usd": 0.42
                },
                "activity": {
                    "api_calls": 15,
                    "messages": 28,
                    "tool_calls": 12
                },
                "context": {
                    "cwd": "/home/user/project",
                    "git_branch": "main",
                    "title": "Fix bug"
                },
                "started_at": 1704067200.123,
                "billing_provider": "nous",
                "cost_status": "within_limit"
            }
        """
        if not self.db_path.exists():
            return None
        
        try:
            conn = self._connect()
            cursor = conn.execute("""
                SELECT 
                    id, source, model, input_tokens, output_tokens,
                    cache_read_tokens, cache_write_tokens, reasoning_tokens,
                    api_call_count, message_count, tool_call_count,
                    estimated_cost_usd, actual_cost_usd,
                    started_at, ended_at, end_reason,
                    cwd, git_branch, title,
                    billing_provider, cost_status
                FROM sessions 
                WHERE ended_at IS NULL 
                ORDER BY started_at DESC 
                LIMIT 1
            """)
            
            row = cursor.fetchone()
            conn.close()
            
            if not row:
                return None
            
            input_tok = row["input_tokens"] or 0
            output_tok = row["output_tokens"] or 0
            
            return {
                "status": "active",
                "session_id": row["id"],
                "model": row["model"],
                "source": row["source"],
                "tokens": {
                    "input": input_tok,
                    "output": output_tok,
                    "cache_read": row["cache_read_tokens"] or 0,
                    "cache_write": row["cache_write_tokens"] or 0,
                    "reasoning": row["reasoning_tokens"] or 0,
                    "total": input_tok + output_tok,
                },
                "cost": {
                    "estimated_usd": row["estimated_cost_usd"] or 0,
                    "actual_usd": row["actual_cost_usd"] or 0,
                },
                "activity": {
                    "api_calls": row["api_call_count"] or 0,
                    "messages": row["message_count"] or 0,
                    "tool_calls": row["tool_call_count"] or 0,
                },
                "context": {
                    "cwd": row["cwd"],
                    "git_branch": row["git_branch"],
                    "title": row["title"],
                },
                "started_at": row["started_at"],
                "billing_provider": row["billing_provider"],
                "cost_status": row["cost_status"],
            }
        except sqlite3.Error:
            return None
    
    def get_session_history(self, limit: int = 10) -> list:
        """Get recent sessions including completed ones."""
        if not self.db_path.exists():
            return []
        
        try:
            conn = self._connect()
            cursor = conn.execute("""
                SELECT 
                    id, source, model, input_tokens, output_tokens,
                    started_at, ended_at, end_reason, title
                FROM sessions 
                ORDER BY started_at DESC 
                LIMIT ?
            """, (limit,))
            
            sessions = []
            for row in cursor.fetchall():
                sessions.append({
                    "session_id": row["id"],
                    "model": row["model"],
                    "source": row["source"],
                    "tokens": {
                        "input": row["input_tokens"] or 0,
                        "output": row["output_tokens"] or 0,
                        "total": (row["input_tokens"] or 0) + (row["output_tokens"] or 0),
                    },
                    "started_at": row["started_at"],
                    "ended_at": row["ended_at"],
                    "ended": row["ended_at"] is not None,
                    "end_reason": row["end_reason"],
                    "title": row["title"],
                })
            conn.close()
            return sessions
        except sqlite3.Error:
            return []
    
    def stream_status(self, interval: float = 2.0) -> Generator[dict, None, None]:
        """
        Generator that yields status updates every interval seconds.
        
        Args:
            interval: Seconds between polls (default: 2.0)
            
        Yields:
            Status dict with 'status' field indicating:
            - "idle": No active session
            - "session_started": New session began
            - "activity": Token count changed in existing session
            - "active": Session continues with same token count
        """
        last_session_id = None
        last_total_tokens = None
        
        while True:
            session = self.get_active_session()
            
            if session is None:
                yield {"status": "idle"}
                last_session_id = None
                last_total_tokens = None
            elif last_session_id is None or session["session_id"] != last_session_id:
                # New session started
                last_session_id = session["session_id"]
                last_total_tokens = session["tokens"]["total"]
                yield {
                    "status": "session_started",
                    **session
                }
            elif session["tokens"]["total"] != last_total_tokens:
                # Tokens changed (new API call completed)
                last_total_tokens = session["tokens"]["total"]
                yield {
                    "status": "activity",
                    **session
                }
            else:
                # Session continues, no new activity
                yield {
                    "status": "active",
                    **session
                }
            
            time.sleep(interval)


# =============================================================================
# USAGE EXAMPLES
# =============================================================================

if __name__ == "__main__":
    # Example 1: One-time check
    monitor = HermesMonitor()
    session = monitor.get_active_session()
    
    if session is None:
        print("Hermes agent is currently idle.")
    else:
        print(f"Active session: {session['session_id']}")
        print(f"Model: {session['model']}")
        print(f"Tokens used: {session['tokens']['total']}")
        print(f"  - Input: {session['tokens']['input']}")
        print(f"  - Output: {session['tokens']['output']}")
        print(f"Estimated cost: ${session['cost']['estimated_usd']:.2f}")
        print(f"API calls: {session['activity']['api_calls']}")
        print(f"Working directory: {session['context']['cwd']}")
        print(f"Git branch: {session['context']['git_branch']}")
        print(f"Title: {session['context']['title']}")
    
    print("\n" + "="*60 + "\n")
    
    # Example 2: Continuous monitoring
    print("Monitoring agent activity (Ctrl+C to stop)...")
    try:
        for update in monitor.stream_status(interval=2.0):
            if update["status"] == "idle":
                print("[Idle] Agent is not processing")
            elif update["status"] == "session_started":
                print(f"[NEW SESSION] {update['session_id']} - {update['model']}")
            elif update["status"] == "activity":
                print(f"[Activity] Tokens: {update['tokens']['total']} (+{update['tokens']['total'] - (update.get('_last', 0))})")
            else:
                print(f"[Active] Session continuing...")
            
            # Store for delta calculation
            update["_last"] = update["tokens"]["total"]
            
    except KeyboardInterrupt:
        print("\nMonitoring stopped.")
```

---

## 2. Log File Watching (True Real-Time)

Log files are written immediately as the agent operates. External apps can tail these files for real-time events with zero polling latency.

### Log File Locations

| Log Type | Path | Contents |
|----------|------|----------|
| Main | `~/.hermes/logs/agent.log` | All agent/tool/session activity |
| Errors | `~/.hermes/logs/errors.log` | WARNING+ level only |
| Gateway | `~/.hermes/logs/gateway.log` | Gateway platform events |
| GUI | `~/.hermes/logs/gui.log` | Dashboard/websocket events |

### Log Format

Each line follows this format:
```
YYYY-MM-DD HH:MM:SS,mmm LEVEL [session_id] logger.name: message
```

**Examples**:
```
2026-01-15 14:32:01,123 INFO [sess_abc123] run_agent.conversation_loop: Starting iteration 5
2026-01-15 14:32:05,456 INFO [sess_abc123] agent.usage_pricing: Token usage - input: 1250, output: 340
2026-01-15 14:32:10,789 WARNING [sess_abc123] tools.terminal_tool: Command execution taking longer than expected
2026-01-15 14:32:15,012 INFO [sess_abc123] run_agent: Tool call complete: execute_command
```

### Log Level Meanings

| Level | Meaning | When It Appears |
|-------|---------|-----------------|
| DEBUG | Detailed debugging | Verbose mode enabled |
| INFO | Normal operation | Every API call, tool execution |
| WARNING | Potential issues | Slow operations, retries |
| ERROR | Errors that didn't crash | Failed tool calls, API errors |
| CRITICAL | Serious failures | Unrecoverable errors |

### Python Log Watcher Implementation

```python
import re
import time
import threading
from pathlib import Path
from typing import Callable, Optional, Dict, Any

# Regex patterns for parsing log lines
LOG_LINE_PATTERN = re.compile(
    r"^(\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}:\d{2},\d{3})\s+"  # Timestamp
    r"(DEBUG|INFO|WARNING|ERROR|CRITICAL)\s+"              # Level
    r"(?:\[([^\]]+)\])?\s+"                                # Optional session ID
    r"([\w.]+):\s+"                                        # Logger name
    r"(.+)$"                                               # Message
)

# Patterns for extracting specific information
TOKEN_USAGE_PATTERN = re.compile(
    r"Token usage\s*-\s*input:\s*(\d+)[,\s]+output:\s*(\d+)"
)

STATUS_PATTERNS = {
    "thinking": re.compile(r"\b(thinking|reasoning|processing)\b", re.IGNORECASE),
    "tool_execution": re.compile(r"\b(tool.*call|executing|running).*\b", re.IGNORECASE),
    "api_call": re.compile(r"\b(api.*call|request.*sent|response.*received)\b", re.IGNORECASE),
    "iteration": re.compile(r"\b(iteration\s*\d+|step\s*\d+)\b", re.IGNORECASE),
    "complete": re.compile(r"\b(complete|finished|done)\b", re.IGNORECASE),
}


class LogWatcher:
    """
    Streams and parses Hermes log files in real-time.
    
    Usage:
        watcher = LogWatcher(log_file="agent")
        watcher.watch(on_line=handle_event)
    """
    
    def __init__(self, log_file: str = "agent", hermes_home: Optional[Path] = None):
        """
        Initialize log watcher.
        
        Args:
            log_file: One of "agent", "errors", "gateway", "gui"
            hermes_home: Path to .hermes directory (optional)
        """
        if hermes_home:
            base = Path(hermes_home)
        else:
            base = Path.home() / ".hermes"
        
        log_files = {
            "agent": base / "logs" / "agent.log",
            "errors": base / "logs" / "errors.log",
            "gateway": base / "logs" / "gateway.log",
            "gui": base / "logs" / "gui.log",
        }
        
        self.log_path = log_files.get(log_file)
        if not self.log_path:
            raise ValueError(f"Unknown log type: {log_file}. Choose from: {list(log_files.keys())}")
        
        self._stop_event = threading.Event()
    
    def watch(
        self,
        on_line: Callable[[Dict[str, Any]], None],
        start_from_beginning: bool = False,
    ):
        """
        Stream log lines, parsing them into structured events.
        
        Args:
            on_line: Callback function receiving parsed event dicts
            start_from_beginning: If False, start from end of file (default)
        """
        # Wait for file to exist
        while not self.log_path.exists():
            if self._stop_event.is_set():
                return
            time.sleep(0.5)
        
        with open(self.log_path, 'r', encoding='utf-8', errors='replace') as f:
            if not start_from_beginning:
                f.seek(0, 2)  # Seek to end
            
            while not self._stop_event.is_set():
                line = f.readline()
                if not line:
                    time.sleep(0.1)
                    continue
                
                parsed = self._parse_line(line.strip())
                if parsed:
                    on_line(parsed)
    
    def stop(self):
        """Stop the watcher."""
        self._stop_event.set()
    
    def _parse_line(self, line: str) -> Optional[Dict[str, Any]]:
        """Parse a log line into structured data."""
        match = LOG_LINE_PATTERN.match(line)
        if not match:
            return None
        
        timestamp, level, session_id, logger, message = match.groups()
        
        event = {
            "timestamp": timestamp,
            "level": level,
            "session_id": session_id,
            "logger": logger,
            "message": message,
            "raw": line,
        }
        
        # Extract token usage if present
        token_match = TOKEN_USAGE_PATTERN.search(message)
        if token_match:
            event["tokens"] = {
                "input": int(token_match.group(1)),
                "output": int(token_match.group(2)),
            }
        
        # Detect status indicators
        for status, pattern in STATUS_PATTERNS.items():
            if pattern.search(message):
                event["status_hint"] = status
                break
        
        return event


# Usage example
if __name__ == "__main__":
    watcher = LogWatcher(log_file="agent")
    
    def handle_event(event: dict):
        # Print all events
        print(f"[{event['level']}] {event['message']}")
        
        # Or filter for specific info
        if event.get("tokens"):
            print(f"  >> Tokens: input={event['tokens']['input']}, output={event['tokens']['output']}")
        
        if event.get("status_hint"):
            print(f"  >> Status: {event['status_hint']}")
    
    try:
        watcher.watch(on_line=handle_event)
    except KeyboardInterrupt:
        watcher.stop()
        print("\nLog watching stopped.")
```

---

## 3. WebSocket API (Dashboard Integration)

When the Hermes dashboard runs (`hermes dashboard`), it exposes WebSocket endpoints for real-time event streaming.

### Server Details

| Property | Value |
|----------|-------|
| Default URL | `ws://127.0.0.1:9119/api/events` |
| Alternative URL | `ws://localhost:9119/api/events` |
| HTTPS Variant | `wss://your-domain.com/api/events` (if configured) |
| Authentication | Session cookie or JWT token (check dashboard config) |

### Message Protocol

**Client → Server (Subscribe):**
```json
{
  "type": "subscribe",
  "channel": "session_updates"
}
```

**Server → Client (Event Types):**

1. **Token Update Event**:
```json
{
  "type": "token_update",
  "session_id": "sess_abc123",
  "timestamp": "2026-01-15T14:32:05.123Z",
  "data": {
    "input_tokens": 1250,
    "output_tokens": 340,
    "total_tokens": 1590,
    "api_call_count": 12,
    "estimated_cost_usd": 0.35
  }
}
```

2. **Status Change Event**:
```json
{
  "type": "status_change",
  "session_id": "sess_abc123",
  "status": "thinking",
  "message": "Model is reasoning about next step"
}
```

3. **Tool Event**:
```json
{
  "type": "tool_started",
  "session_id": "sess_abc123",
  "tool_name": "execute_command",
  "tool_args": {"command": "git status"}
}
```

```json
{
  "type": "tool_completed",
  "session_id": "sess_abc123",
  "tool_name": "execute_command",
  "duration_ms": 1250,
  "success": true
}
```

4. **Session Lifecycle Event**:
```json
{
  "type": "session_started",
  "session_id": "sess_abc123",
  "model": "claude-sonnet-4-6",
  "source": "cli"
}
```

```json
{
  "type": "session_ended",
  "session_id": "sess_abc123",
  "end_reason": "compression",
  "final_tokens": {
    "input": 15000,
    "output": 4500,
    "total": 19500
  }
}
```

### Python WebSocket Client

```python
import asyncio
import json
import websockets

async def listen_to_hermes_websocket(
    url: str = "ws://127.0.0.1:9119/api/events",
    auth_token: str = None,
):
    """
    Connect to Hermes dashboard WebSocket and stream events.
    
    Args:
        url: WebSocket URL (default dashboard port)
        auth_token: Optional authentication token
    """
    headers = {}
    if auth_token:
        headers["Authorization"] = f"Bearer {auth_token}"
    
    async with websockets.connect(url, extra_headers=headers) as ws:
        # Subscribe to session updates
        await ws.send(json.dumps({
            "type": "subscribe",
            "channel": "session_updates"
        }))
        
        print(f"Connected to Hermes at {url}")
        
        async for message in ws:
            try:
                event = json.loads(message)
                event_type = event.get("type", "unknown")
                
                if event_type == "token_update":
                    data = event.get("data", {})
                    print(f"Tokens: {data.get('total_tokens')} | "
                          f"Input: {data.get('input_tokens')} | "
                          f"Output: {data.get('output_tokens')}")
                
                elif event_type == "status_change":
                    print(f"Status: {event.get('status')} - {event.get('message')}")
                
                elif event_type == "tool_started":
                    print(f"Tool starting: {event.get('tool_name')}")
                
                elif event_type == "tool_completed":
                    print(f"Tool complete: {event.get('tool_name')} "
                          f"({event.get('duration_ms')}ms)")
                
                elif event_type == "session_started":
                    print(f"New session: {event.get('session_id')} "
                          f"using {event.get('model')}")
                
                elif event_type == "session_ended":
                    print(f"Session ended: {event.get('session_id')} "
                          f"reason={event.get('end_reason')} "
                          f"final_tokens={event.get('final_tokens', {}).get('total')}")
                
                else:
                    print(f"Event: {event_type} - {event}")
                    
            except json.JSONDecodeError:
                print(f"Raw message: {message}")


# Run with: python script.py
if __name__ == "__main__":
    asyncio.run(listen_to_hermes_websocket())
```

---

## 4. REST API Endpoints

The dashboard provides REST endpoints for querying state.

### Available Endpoints

| Endpoint | Method | Description | Response Format |
|----------|--------|-------------|-----------------|
| `/api/sessions` | GET | List all sessions | JSON array |
| `/api/sessions/{id}` | GET | Get specific session | JSON object |
| `/api/sessions/{id}/messages` | GET | Get session messages | JSON array |
| `/api/status` | GET | Runtime status | JSON object |
| `/api/usage` | GET | Account credits/usage | JSON object |

### Base URL

```
http://127.0.0.1:9119
```

### Python REST Client

```python
import requests
from typing import Optional, List, Dict, Any

BASE_URL = "http://127.0.0.1:9119"


def get_active_session() -> Optional[Dict[str, Any]]:
    """Get the currently active session."""
    response = requests.get(f"{BASE_URL}/api/sessions")
    if response.status_code != 200:
        return None
    
    sessions = response.json()
    for session in sessions:
        if session.get("ended_at") is None:
            return session
    return None


def get_session_details(session_id: str) -> Optional[Dict[str, Any]]:
    """Get detailed info for a specific session."""
    response = requests.get(f"{BASE_URL}/api/sessions/{session_id}")
    if response.status_code != 200:
        return None
    return response.json()


def get_session_messages(session_id: str) -> List[Dict[str, Any]]:
    """Get message history for a session."""
    response = requests.get(f"{BASE_URL}/api/sessions/{session_id}/messages")
    if response.status_code != 200:
        return []
    return response.json()


def get_runtime_status() -> Dict[str, Any]:
    """Get current runtime status."""
    response = requests.get(f"{BASE_URL}/api/status")
    if response.status_code != 200:
        return {"error": "Could not fetch status"}
    return response.json()


def get_account_usage() -> Dict[str, Any]:
    """Get account credits and usage."""
    response = requests.get(f"{BASE_URL}/api/usage")
    if response.status_code != 200:
        return {"error": "Could not fetch usage"}
    return response.json()


# Usage example
if __name__ == "__main__":
    session = get_active_session()
    if session:
        print(f"Active Session: {session['id']}")
        print(f"Model: {session.get('model')}")
        print(f"Input Tokens: {session.get('input_tokens')}")
        print(f"Output Tokens: {session.get('output_tokens')}")
    else:
        print("No active session")
```

---

## 5. MCP Server (AI Tool Integration)

Hermes provides an MCP (Model Context Protocol) server for integration with AI development tools like Claude Code, Cursor, and Codex.

### Starting the MCP Server

```bash
hermes mcp serve
```

### Client Configuration

**For Claude Code (`~/.claude/settings.json`):**
```json
{
  "mcpServers": {
    "hermes": {
      "command": "hermes",
      "args": ["mcp", "serve"]
    }
  }
}
```

**For Cursor (settings.json):**
```json
{
  "mcpServers": {
    "hermes": {
      "command": "hermes",
      "args": ["mcp", "serve"]
    }
  }
}
```

### Available MCP Tools

The MCP server exposes these tools:

1. **`hermes_list_sessions`** - List all sessions
2. **`hermes_get_session`** - Get session details by ID
3. **`hermes_get_active_session`** - Get currently active session
4. **`hermes_get_session_messages`** - Get message history
5. **`hermes_get_usage`** - Get token usage summary

Example MCP tool call:
```json
{
  "method": "tools/call",
  "params": {
    "name": "hermes_get_active_session",
    "arguments": {}
  }
}
```

---

## State Definitions and Interpretation

### Session States

| State | How to Detect | Description |
|-------|---------------|-------------|
| `idle` | No session with `ended_at IS NULL` | Agent is not processing any request |
| `active` | Session exists, `ended_at IS NULL` | Agent is actively processing |
| `thinking` | Log contains "thinking"/"reasoning" | Model generating reasoning content |
| `tool_execution` | Log contains "tool" + "executing" | Tools/functions being invoked |
| `streaming` | Receiving stream deltas | Response tokens being streamed to user |
| `waiting_user_input` | Session idle > threshold | Waiting for user to respond |
| `completed` | `ended_at IS NOT NULL` | Session finished normally |
| `error` | `end_reason = 'error'` | Session terminated due to error |

### End Reasons Explained

| `end_reason` Value | Meaning |
|--------------------|---------|
| `None` | Session still active |
| `compression` | Session was compressed (context management) |
| `branched` | Session was branched (user created parallel conversation) |
| `agent_close` | Agent closed session gracefully |
| `error` | Session terminated due to error |
| `timeout` | Session exceeded time/iteration limits |
| `user_cancelled` | User explicitly cancelled |

### Token Counter Update Latency

| Event | Time to DB Update | Time to Log |
|-------|-------------------|-------------|
| API response received | ~1-5 seconds | Immediate |
| Message appended | ~1-2 seconds | Immediate |
| Session ended | Immediate | Immediate |
| Tool call completed | ~1 second | Immediate |

**Recommendation**: For sub-second latency, use log watching. For structured data, use SQLite polling.

---

## Error Handling Patterns

### Database Lock Handling

SQLite may report "database is locked" during writes. Always implement retry logic:

```python
import sqlite3
import time

def safe_query(db_path: str, query: str, params: tuple = (), max_retries: int = 5):
    """Execute query with automatic retry on lock errors."""
    for attempt in range(max_retries):
        try:
            conn = sqlite3.connect(db_path, timeout=10.0)
            conn.row_factory = sqlite3.Row
            cursor = conn.execute(query, params)
            result = cursor.fetchall()
            conn.close()
            return result
        except sqlite3.OperationalError as e:
            error_msg = str(e).lower()
            if ("locked" in error_msg or "busy" in error_msg) and attempt < max_retries - 1:
                # Exponential backoff: 0.1s, 0.2s, 0.4s, 0.8s
                wait_time = 0.1 * (2 ** attempt)
                time.sleep(wait_time)
                continue
            raise  # Re-raise non-lock errors or after max retries
```

### File Existence Checks

Always verify files exist before accessing:

```python
from pathlib import Path

def check_hermes_ready():
    """Check if Hermes has been initialized."""
    home = Path.home() / ".hermes"
    
    checks = {
        "database": home / "state.db",
        "logs_dir": home / "logs",
        "config": home / "config.yaml",
    }
    
    missing = [name for name, path in checks.items() if not path.exists()]
    
    if missing:
        return {
            "ready": False,
            "missing": missing,
            "message": f"Hermes not initialized. Missing: {', '.join(missing)}"
        }
    
    return {"ready": True, "missing": [], "message": "Hermes is ready"}
```

---

## Troubleshooting Guide

| Problem | Possible Cause | Solution |
|---------|----------------|----------|
| "No active session found" | Agent is idle | This is normal when no conversation is running |
| "Database is locked" | Another process writing | Implement retry logic, wait 100-500ms |
| "File not found: state.db" | Agent never run | Run `hermes chat` once to initialize |
| Logs not updating | Logging disabled | Check `config.yaml` for `logging.enabled: true` |
| Wrong paths on WSL | Using Linux vs Windows path | Use `/mnt/c/Users/...` for Windows home |
| WebSocket connection refused | Dashboard not running | Start with `hermes dashboard` |
| REST API 404 | Wrong port | Check `HERMES_WEB_PORT` env variable |

---

## Security Considerations

1. **Database permissions**: Contains potential secrets in messages. Set `chmod 600` on `state.db`
2. **Log redaction**: Logs use `RedactingFormatter` but review what your app stores
3. **WebSocket auth**: Dashboard may require authentication - handle auth failures
4. **Network exposure**: Don't expose port 9119 without authentication
5. **File access**: Ensure your external app has read permissions for `.hermes` directory

---

## Quick Reference Card

```
HERMES_HOME: ~/.hermes
DATABASE: ~/.hermes/state.db
MAIN LOG: ~/.hermes/logs/agent.log
WEB PORT: 9119

ACTIVE SESSION QUERY:
  SELECT * FROM sessions WHERE ended_at IS NULL ORDER BY started_at DESC LIMIT 1;

KEY COLUMNS:
  id, input_tokens, output_tokens, api_call_count, 
  estimated_cost_usd, ended_at, end_reason, model, title

STATES:
  idle = no active session
  active = ended_at IS NULL
  completed = ended_at IS NOT NULL

MONITORING METHODS:
  SQLite (simple, 1-5s latency)
  Logs (real-time, parse needed)
  WebSocket (real-time, structured)
  REST API (HTTP, polling)
```

---

*Document version: 1.0*  
*Last updated: 2026-01*  
*For issues or questions, refer to main Hermes documentation*

"""Offline integration tests: actual LangGraph, mock provider and app bridge."""

import base64
import copy
import http.client
import importlib.util
import json
import os
from pathlib import Path
import queue
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
from email.utils import formatdate
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

_source = Path(os.environ.get("AETHERDESK_ASSISTANT_TEST_SERVER", Path(__file__).with_name("server.py")))
_spec = importlib.util.spec_from_file_location("assistant_server", _source)
server = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(server)


PNG_URL = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII="


class Fixture(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self):
        self.calls = []
        self.requests = []
        self.models = [{"id": "test-model", "name": "Test model"}]
        self.models_error = False
        self.provider_failures = []
        self.paths = []
        self.release_stream = threading.Event()
        self.stream_started = threading.Event()
        self.tool_revision = "current-revision"
        super().__init__(("127.0.0.1", 0), FixtureHandler)
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)
        self.thread.start()

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server_port}"

    def close(self):
        self.release_stream.set()
        self.shutdown()
        self.server_close()
        self.thread.join(3)


class FixtureHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def reply(self, value, status=200, headers=None):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self.server.paths.append(self.path)
        if self.path.endswith("/models"):
            self.reply({"data": self.server.models}, 404 if self.server.models_error else 200)
        elif self.path == "/redirect":
            self.send_response(302)
            self.send_header("Location", self.server.url + "/models")
            self.send_header("Content-Length", "0")
            self.end_headers()
        else:
            self.reply({}, 404)

    def do_POST(self):
        self.server.paths.append(self.path)
        value = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        if self.path == "/mcp":
            assert self.headers.get("Authorization") == "Bearer bridge-secret"
            method, params = value["method"], value.get("params", {})
            if method == "initialize":
                result = {"protocolVersion": "2025-03-26", "capabilities": {"tools": {}}}
            elif method == "tools/list":
                result = {"tools": [{"name": name, "description": name, "inputSchema": {"type": "object", "properties": {}}} for name in sorted(server.READ_TOOLS | server.APPROVAL_TOOLS | {"untrusted_tool"})]}
            elif method == "tools/call":
                self.server.calls.append(copy.deepcopy(params))
                if params["name"] == "tool_read":
                    output = {"manifest": {"id": "hello", "title": "Hello", "target": "windows", "script": "Write-Output 'review me'"}, "revision": self.server.tool_revision}
                elif params["name"] == "app_capture":
                    output = {"label": "Design connection", "url": PNG_URL}
                elif params["name"] == "workspace_read":
                    output = {"text": "actual file text", "revision": "sha256"}
                else:
                    output = {"ok": True}
                result = {"content": [{"type": "text", "text": json.dumps(output)}]}
            else:
                result = {}
            self.reply({"jsonrpc": "2.0", "id": value["id"], "result": result})
            return
        self.server.requests.append(copy.deepcopy(value))
        if self.server.provider_failures:
            failure = self.server.provider_failures.pop(0)
            status = failure.get("status", 503)
            payload = {"error": {"code": status, "message": failure.get("message", "Provider busy. fixture-api-key")}}
            if failure.get("stream"):
                payload["error"]["metadata"] = {"headers": {"Retry-After": failure.get("retry_after", "0")}}
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Retry-After", failure.get("retry_after", "0"))
                self.send_header("Connection", "close")
                self.end_headers()
                self.close_connection = True
                if failure.get("partial"):
                    self.wfile.write(b'data: {"choices":[{"index":0,"delta":{"content":"Partial answer"}}]}\n\n')
                self.wfile.write(b"data: " + json.dumps(payload).encode() + b"\n\n")
                self.wfile.flush()
            else:
                self.reply(payload, status, {"Retry-After": failure.get("retry_after", "0")})
            return
        history = value["messages"]
        prompt = next(item["content"] for item in reversed(history) if item["role"] == "user")
        if isinstance(prompt, list):
            prompt = "\n".join(part.get("text", "") for part in prompt if part.get("type") == "text")
        if prompt in {"tools unavailable", "privacy blocked", "model offline", "route missing"}:
            messages = {
                "tools unavailable": "No endpoints found that support tool use. fixture-api-key",
                "privacy blocked": "No endpoints found matching your data policy. fixture-api-key",
                "model offline": "No endpoints found for test-model. fixture-api-key",
                "route missing": "Not found. fixture-api-key",
            }
            self.reply({"error": {"message": messages[prompt], "code": 404}}, 404)
            return
        if prompt == "stream tools unavailable":
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Connection", "close")
            self.end_headers()
            self.close_connection = True
            self.wfile.write(b'data: {"error":{"code":404,"message":"No endpoints found that support tool use. fixture-api-key"}}\n\n')
            self.wfile.flush()
            return
        if prompt == "error":
            self.reply({"error": "do not leak fixture-api-key"}, 401)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        self.close_connection = True

        def chunk(delta, reason=None):
            try:
                self.wfile.write(b"data: " + json.dumps({"choices": [{"index": 0, "delta": delta, "finish_reason": reason}]}).encode() + b"\n\n")
                self.wfile.flush()
            except (BrokenPipeError, ConnectionError, OSError):
                return False
            return True

        tool_args = {
            "capture": ("app_capture", {"connectionId": "design"}),
            "read": ("workspace_read", {"path": "notes.txt"}),
            "write": ("workspace_write", {"path": "notes.txt", "text": "new text", "expectedRevision": "sha256"}),
            "run tool": ("tool_run", {"id": "hello", "expectedRevision": "current-revision", "arguments": {"name": "world"}}),
            "install": ("tool_install", {"manifest": {"id": "hello", "script": "Write-Output hi"}, "expectedRevision": "new"}),
            "unknown": ("untrusted_tool", {}),
        }
        if prompt == "three commands" and sum(item["role"] == "tool" for item in history) < 3:
            index = sum(item["role"] == "tool" for item in history)
            chunk({"tool_calls": [{"index": 0, "id": f"call_{index}", "type": "function", "function": {"name": "aetherdesk_workspace_command", "arguments": json.dumps({"command": f"echo {index}"})}}]}, "tool_calls")
        elif history[-1]["role"] == "tool":
            chunk({"content": "Tool result received."}, "stop")
        elif prompt in tool_args:
            name, arguments = tool_args[prompt]
            # Arguments and tool names deliberately arrive split across frames.
            chunk({"tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": "aetherdesk_", "arguments": ""}}]})
            chunk({"tool_calls": [{"index": 0, "function": {"name": name, "arguments": json.dumps(arguments)}}]}, "tool_calls")
        elif prompt == "stop stream":
            chunk({"content": "Starting"})
            self.server.stream_started.set()
            self.server.release_stream.wait(10)
            chunk({"tool_calls": [{"index": 0, "id": "late", "function": {"name": "aetherdesk_workspace_read", "arguments": "{}"}}]}, "tool_calls")
        elif prompt == "nullable stream":
            chunk({"content": "Hello from LangGraph.", "tool_calls": None})
            chunk(None, "stop")
        elif prompt == "truncated":
            chunk({"content": "Incomplete"})
            return
        else:
            chunk({"content": "Hello "})
            chunk({"content": "from LangGraph."}, "stop")
        try:
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except (BrokenPipeError, ConnectionError, OSError):
            pass


class BackendTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="aether-assistant-")
        self.fixture = Fixture()
        self.config = {
            "provider": {"custom": {"options": {"apiKey": "fixture-api-key", "baseURL": self.fixture.url + "/v1"}}},
            "mcp": {"aetherdesk": {"url": self.fixture.url + "/mcp", "headers": {"Authorization": "Bearer bridge-secret"}}},
        }
        self.service = server.AgentService(self.config, Path(self.directory.name), "private-test-token-0123456789")
        self.http = server.AssistantServer(0, self.service)
        self.thread = threading.Thread(target=self.http.serve_forever, daemon=True)
        self.thread.start()
        self.session = self.service.create_session("New chat")["id"]

    def tearDown(self):
        self.service.close()
        self.http.shutdown()
        self.http.server_close()
        self.thread.join(3)
        self.fixture.close()
        self.directory.cleanup()

    def request(self, method, path, body=None, headers=None):
        client = http.client.HTTPConnection("127.0.0.1", self.http.server_port, timeout=10)
        authorization = "Basic " + base64.b64encode(("aetherdesk:" + self.service.token).encode()).decode()
        all_headers = {"Authorization": authorization, "Content-Type": "application/json"}
        all_headers.update(headers or {})
        client.request(method, path, body=None if body is None else json.dumps(body), headers=all_headers)
        response = client.getresponse()
        data = response.read()
        client.close()
        return response.status, json.loads(data) if data else None

    def start(self, text):
        code, result = self.request("POST", f"/session/{self.session}/prompt_async", {"model": {"providerID": "custom", "modelID": "test-model"}, "system": "Use app workspace tools.", "parts": [{"type": "text", "text": text}]})
        self.assertEqual(code, 204, result)

    def finished(self):
        turn = self.service.turns[self.session]
        self.assertTrue(turn.done.wait(6), "Turn did not finish")
        return self.request("GET", f"/session/{self.session}/message")[1]

    def permission(self):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            permissions = self.request("GET", "/permission")[1]
            if permissions:
                return permissions[0]
            time.sleep(0.015)
        self.fail("No approval request appeared")

    def test_streaming_chat_history_and_secret_free_restart(self):
        events = queue.Queue()
        self.service.subscribers.add(events)
        self.start("hello")
        messages = self.finished()
        self.assertEqual(messages[-1]["parts"][0]["text"], "Hello from LangGraph.")
        self.assertTrue(any(event["type"] == "message.part.updated" for event in list(events.queue)))
        self.start("second turn")
        self.finished()
        history = self.fixture.requests[-1]["messages"]
        self.assertEqual([item["role"] for item in history], ["system", "user", "assistant", "user"])
        saved = (Path(self.directory.name) / "langgraph-sessions.json").read_text()
        self.assertNotIn("fixture-api-key", saved)
        self.assertNotIn("bridge-secret", saved)
        restored = server.AgentService(self.config, Path(self.directory.name), self.service.token)
        self.assertEqual(len(restored.sessions[self.session]["messages"]), 4)

    def test_real_graph_routes_read_tool_result_back_to_model(self):
        self.start("read")
        messages = self.finished()
        self.assertEqual(self.fixture.calls, [{"name": "workspace_read", "arguments": {"path": "notes.txt"}}])
        self.assertIn("actual file text", self.fixture.requests[-1]["messages"][-1]["content"])
        self.assertEqual(messages[-1]["parts"][0]["text"], "Tool result received.")
        names = {item["function"]["name"] for item in self.fixture.requests[0]["tools"]}
        self.assertNotIn("aetherdesk_untrusted_tool", names)
        self.assertNotIn("bash", names)

    def test_write_denial_and_approval_are_one_time_and_no_action_precedes_approval(self):
        self.start("write")
        permission = self.permission()
        self.assertEqual(self.fixture.calls, [])
        self.assertEqual(permission["metadata"]["input"]["text"], "new text")
        self.assertEqual(self.request("POST", f"/permission/{permission['id']}/reply", {"reply": "reject"})[0], 200)
        self.finished()
        self.assertEqual(self.fixture.calls, [])
        self.start("write")
        permission = self.permission()
        self.assertEqual(self.request("POST", f"/permission/{permission['id']}/reply", {"reply": "once"})[0], 200)
        self.finished()
        self.assertEqual(len(self.fixture.calls), 1)
        self.assertEqual(self.fixture.calls[0]["name"], "workspace_write")
        self.assertEqual(self.request("POST", f"/permission/{permission['id']}/reply", {"reply": "once"})[0], 400)

    def test_installed_tool_approval_contains_actual_script_and_requires_matching_revision(self):
        self.start("run tool")
        permission = self.permission()
        self.assertEqual(permission["metadata"]["input"]["manifest"]["script"], "Write-Output 'review me'")
        self.assertEqual([item["name"] for item in self.fixture.calls], ["tool_read"])
        self.request("POST", f"/permission/{permission['id']}/reply", {"reply": "once"})
        self.finished()
        self.assertEqual([item["name"] for item in self.fixture.calls], ["tool_read", "tool_run"])
        self.fixture.tool_revision = "changed"
        self.start("run tool")
        self.finished()
        self.assertEqual(len([item for item in self.fixture.calls if item["name"] == "tool_run"]), 1)
        self.assertEqual(self.request("GET", "/permission")[1], [])

    def test_stop_waiting_approval_prevents_execution_and_rejects_stale_reply(self):
        self.start("write")
        permission = self.permission()
        self.assertEqual(self.request("POST", f"/session/{self.session}/abort")[0], 200)
        self.finished()
        self.assertEqual(self.fixture.calls, [])
        self.assertEqual(self.request("POST", f"/permission/{permission['id']}/reply", {"reply": "once"})[0], 400)
        self.assertEqual(self.request("GET", "/permission")[1], [])
        self.start("hello")
        self.finished()

    def test_stop_interrupts_stream_and_cannot_dispatch_late_tool(self):
        self.start("stop stream")
        self.assertTrue(self.fixture.stream_started.wait(4))
        start = time.monotonic()
        self.assertEqual(self.request("POST", f"/session/{self.session}/abort")[0], 200)
        self.assertLess(time.monotonic() - start, 3)
        self.fixture.release_stream.set()
        self.finished()
        self.assertEqual(self.fixture.calls, [])

    def test_provider_error_and_truncated_stream_are_visible_without_secrets(self):
        self.start("error")
        messages = self.finished()
        text = messages[-1]["info"]["error"]["data"]["message"]
        self.assertIn("API key was rejected", text)
        self.assertNotIn("fixture-api-key", json.dumps(messages))
        self.start("truncated")
        messages = self.finished()
        self.assertIn("before its response was complete", messages[-1]["info"]["error"]["data"]["message"])

    def test_message_approval_covers_multiple_rounds_but_expires(self):
        self.start("three commands")
        pending = self.permission()
        self.assertEqual(self.fixture.calls, [])
        self.assertEqual(self.request("POST", f"/permission/{pending['id']}/reply", {"reply": "message"})[0], 200)
        self.finished()
        self.assertEqual([call["arguments"]["command"] for call in self.fixture.calls], ["echo 0", "echo 1", "echo 2"])
        self.start("write")
        pending = self.permission()
        self.request("POST", f"/permission/{pending['id']}/reply", {"reply": "reject"})
        self.finished()
        self.assertEqual(len(self.fixture.calls), 3)

    def test_chat_approval_is_scoped_revocable_and_not_persisted(self):
        self.start("write")
        pending = self.permission()
        self.request("POST", f"/permission/{pending['id']}/reply", {"reply": "session"})
        self.finished()
        self.start("write")
        self.finished()
        self.assertEqual(len(self.fixture.calls), 2)
        original = self.session
        self.session = self.service.create_session("Other chat")["id"]
        self.start("write")
        pending = self.permission()
        self.request("POST", f"/permission/{pending['id']}/reply", {"reply": "reject"})
        self.finished()
        self.session = original
        self.assertEqual(self.request("POST", f"/session/{original}/revoke_approvals")[0], 200)
        self.start("write")
        pending = self.permission()
        self.request("POST", f"/permission/{pending['id']}/reply", {"reply": "reject"})
        self.finished()
        restored = server.AgentService(self.config, Path(self.directory.name), self.service.token)
        self.assertEqual(restored.approved_sessions, set())
        restored.close()
        self.assertEqual(len(self.fixture.calls), 2)

    def test_attached_screen_reaches_provider_and_survives_history(self):
        code, result = self.request("POST", f"/session/{self.session}/prompt_async", {
            "model": {"providerID": "custom", "modelID": "test-model"},
            "parts": [{"type": "text", "text": "Explain this error"}, {"type": "image", "url": PNG_URL}]})
        self.assertEqual(code, 204, result)
        messages = self.finished()
        content = self.fixture.requests[-1]["messages"][0]["content"]
        self.assertEqual(content[1], {"type": "image_url", "image_url": {"url": PNG_URL}})
        self.assertEqual(messages[0]["parts"][1]["type"], "image")
        restored = server.AgentService(self.config, Path(self.directory.name), self.service.token)
        self.assertEqual(restored._history(self.session)[0]["content"], content)
        restored.close()
        for bad in ["https://example.com/private.png", "data:image/png;base64,invalid", "data:image/png;base64," + "A" * 1400001]:
            with self.assertRaises(server.AgentError):
                server.image_part(bad)

    def test_capture_tool_requires_approval_and_supplies_image_after_tool_result(self):
        self.start("capture")
        pending = self.permission()
        self.assertEqual(self.fixture.calls, [])
        self.assertEqual(pending["metadata"]["input"]["connectionId"], "design")
        self.request("POST", f"/permission/{pending['id']}/reply", {"reply": "once"})
        self.finished()
        history = self.fixture.requests[-1]["messages"]
        self.assertEqual(history[-2]["role"], "tool")
        self.assertNotIn("base64", history[-2]["content"])
        self.assertEqual(history[-1]["content"][1]["image_url"]["url"], PNG_URL)

    def test_delete_chat_and_clear_history_preserve_other_chats_and_provider(self):
        self.start("hello")
        self.finished()
        other = self.service.create_session("Keep me")["id"]
        self.service.approved_sessions.add(self.session)
        self.assertEqual(self.request("POST", f"/session/{self.session}/delete")[0], 200)
        self.assertNotIn(self.session, self.service.sessions)
        self.assertNotIn(self.session, self.service.approved_sessions)
        self.assertIn(other, self.service.sessions)
        self.assertEqual(self.request("POST", "/session/clear_history")[0], 200)
        restored = server.AgentService(self.config, Path(self.directory.name), self.service.token)
        self.assertEqual(restored.sessions, {})
        self.assertIn("custom", restored.providers()["connected"])
        restored.close()

    def test_deletion_is_rejected_while_a_turn_is_waiting_for_approval(self):
        self.start("write")
        pending = self.permission()
        self.assertEqual(self.request("POST", "/session/clear_history")[0], 400)
        self.assertEqual(self.request("POST", f"/session/{self.session}/delete")[0], 400)
        self.assertIn(self.session, self.service.sessions)
        self.request("POST", f"/permission/{pending['id']}/reply", {"reply": "reject"})
        self.finished()

    def test_catalog_excludes_known_incompatible_models_and_manual_overrides(self):
        self.fixture.models = [
            {"id": "agent", "supported_parameters": ["tools", "max_tokens"]},
            {"id": "moderation", "supported_parameters": ["max_tokens"], "architecture": {"output_modalities": ["text"]}},
            {"id": "embedding", "architecture": {"output_modalities": ["embedding"]}},
            {"id": "unknown-capabilities"},
        ]
        self.service.config["provider"]["custom"]["models"] = {"moderation": {"name": "Manual override"}}
        provider = next(item for item in self.service.providers()["all"] if item["id"] == "custom")
        self.assertEqual(set(provider["models"]), {"agent", "unknown-capabilities"})

    def test_free_and_paid_models_are_listed_with_tool_compatibility_checks(self):
        self.fixture.models = [
            {"id": "moderation:free", "supported_parameters": ["max_tokens"]},
            {"id": "agent:free", "supported_parameters": ["tools"]},
            {"id": "paid-agent", "supported_parameters": ["tools"], "pricing": {"prompt": "1", "completion": "1"}},
        ]
        self.service.config["provider"]["openrouter"] = self.service.config["provider"]["custom"]
        provider = next(item for item in self.service.providers()["all"] if item["id"] == "openrouter")
        self.assertEqual(set(provider["models"]), {"agent:free", "paid-agent"})

    def test_explicit_paid_model_selection_reaches_provider_unchanged(self):
        self.fixture.models = [{"id": "paid-agent", "supported_parameters": ["tools"],
                                "pricing": {"prompt": "1", "completion": "2"}}]
        self.service.config["provider"]["openrouter"] = self.service.config["provider"]["custom"]
        code, result = self.request("POST", f"/session/{self.session}/prompt_async", {
            "model": {"providerID": "openrouter", "modelID": "paid-agent"},
            "parts": [{"type": "text", "text": "hello"}]})
        self.assertEqual(code, 204, result)
        self.finished()
        self.assertEqual(self.fixture.requests[-1]["model"], "paid-agent")

    def test_full_endpoint_normalizes_and_output_limit_is_respected(self):
        self.service.config["provider"]["custom"]["options"]["baseURL"] = self.fixture.url + "/v1/chat/completions/"
        self.fixture.models = [{"id": "test-model", "supported_parameters": ["tools"], "top_provider": {"max_completion_tokens": 512}}]
        self.start("hello")
        self.finished()
        self.assertIn("/v1/models", self.fixture.paths)
        self.assertIn("/v1/chat/completions", self.fixture.paths)
        self.assertNotIn("/v1/chat/completions/chat/completions", self.fixture.paths)
        self.assertEqual(self.fixture.requests[-1]["max_tokens"], 512)
        self.assertEqual(server.provider_base_url("https://openrouter.ai/v1"), "https://openrouter.ai/api/v1")
        self.assertEqual(server.provider_base_url("https://integrate.api.nvidia.com/v1/chat/completions"), "https://integrate.api.nvidia.com/v1")
        self.assertEqual(server.provider_base_url("https://example.com/prefix/v2"), "https://example.com/prefix/v2")

    def test_404_and_stream_errors_explain_cause_without_leaking_provider_body(self):
        for prompt, expected in [
            ("tools unavailable", "do not support"),
            ("privacy blocked", "privacy/data policy"),
            ("model offline", "no available endpoint"),
            ("route missing", "API route"),
            ("stream tools unavailable", "do not support"),
        ]:
            with self.subTest(prompt=prompt):
                self.start(prompt)
                messages = self.finished()
                failure = messages[-1]["info"]["error"]["data"]["message"]
                self.assertIn(expected, failure)
                self.assertNotIn("fixture-api-key", json.dumps(messages))
        self.assertNotIn("provider", self.fixture.requests[-1])  # No privacy-policy override.

    def test_transient_http_errors_recover_without_duplicate_history_or_actions(self):
        self.fixture.provider_failures = [
            {"status": 429, "retry_after": "0.01"},
            {"status": 503, "retry_after": "0.01"},
        ]
        events = queue.Queue()
        self.service.subscribers.add(events)
        self.start("read")
        messages = self.finished()
        self.assertNotIn("error", messages[-1]["info"])
        self.assertEqual(len(messages), 3)  # user + tool call + model answer
        self.assertEqual(len(self.fixture.requests), 4)  # 2 rejects + tool + answer
        self.assertEqual(len(self.fixture.calls), 1)
        self.assertEqual(self.fixture.calls[0]["name"], "workspace_read")
        retries = [e["properties"]["status"] for e in list(events.queue)
                   if e["type"] == "session.status" and e["properties"]["status"]["type"] == "retry"]
        self.assertEqual([e["attempt"] for e in retries], [1, 2])
        self.assertNotIn("fixture-api-key", json.dumps(retries))
        self.assertEqual(self.request("GET", "/session/status")[1][self.session], {"type": "idle"})

    def test_stream_error_retries_only_before_visible_answer(self):
        self.fixture.provider_failures = [{"status": 503, "stream": True}]
        self.start("hello")
        messages = self.finished()
        self.assertEqual(messages[-1]["parts"][0]["text"], "Hello from LangGraph.")
        self.assertNotIn("error", messages[-1]["info"])
        self.fixture.provider_failures = [{"status": 503, "stream": True, "partial": True}]
        requests = len(self.fixture.requests)
        self.start("hello")
        messages = self.finished()
        self.assertEqual(len(self.fixture.requests), requests + 1)
        self.assertEqual(messages[-1]["parts"][0]["text"], "Partial answer")
        self.assertIn("error", messages[-1]["info"])

    def test_retry_limit_and_permanent_errors_do_not_loop(self):
        self.fixture.provider_failures = [{"status": 503}] * 4
        self.start("hello")
        messages = self.finished()
        self.assertIn("temporarily unavailable", messages[-1]["info"]["error"]["data"]["message"])
        self.assertEqual(len(self.fixture.requests), 4)
        for failure in [
            {"status": 401},
            {"status": 400},
            {"status": 429, "message": "Rate limit exceeded: free-models-per-day"},
            {"status": 429, "retry_after": "86400"},
        ]:
            with self.subTest(failure=failure):
                self.fixture.provider_failures = [failure]
                requests = len(self.fixture.requests)
                self.start("hello")
                messages = self.finished()
                self.assertIn("error", messages[-1]["info"])
                self.assertEqual(len(self.fixture.requests), requests + 1)

    def test_stop_during_retry_prevents_request_and_recovers_next_turn(self):
        self.fixture.provider_failures = [{"status": 429, "retry_after": "20"}]
        self.start("write")
        deadline = time.monotonic() + 4
        while time.monotonic() < deadline:
            status = self.request("GET", "/session/status")[1][self.session]
            if status["type"] == "retry":
                break
            time.sleep(0.01)
        self.assertEqual(status["type"], "retry")
        start = time.monotonic()
        self.assertEqual(self.request("POST", f"/session/{self.session}/abort")[0], 200)
        self.assertLess(time.monotonic() - start, 2)
        messages = self.finished()
        self.assertEqual(messages[-1]["info"]["error"]["name"], "Cancelled")
        self.assertEqual(self.fixture.calls, [])
        self.assertEqual(len(self.fixture.requests), 1)
        self.start("hello")
        self.assertNotIn("error", self.finished()[-1]["info"])

    def test_nvidia_and_openrouter_use_selected_endpoint_and_model_during_recovery(self):
        for provider in ("nvidia", "openrouter"):
            with self.subTest(provider=provider):
                self.service.config["provider"] = {provider: {
                    "options": {"baseURL": self.fixture.url + "/v1", "apiKey": "fixture-api-key"}}}
                self.service._provider_cache = None
                self.fixture.provider_failures = [{"status": 503}]
                code, result = self.request("POST", f"/session/{self.session}/prompt_async", {
                    "model": {"providerID": provider, "modelID": "test-model"},
                    "parts": [{"type": "text", "text": "hello"}]})
                self.assertEqual(code, 204, result)
                self.assertNotIn("error", self.finished()[-1]["info"])
                self.assertEqual(self.fixture.requests[-1]["model"], "test-model")
                self.assertEqual(self.fixture.paths[-1], "/v1/chat/completions")

    def test_retry_headers_and_slow_provider_stream_tolerance(self):
        self.assertEqual(server.retry_delay({"Retry-After": "2.5"}), 2.5)
        self.assertEqual(server.retry_delay({"retry-after-ms": "250"}), .25)
        self.assertIsNone(server.retry_delay({"Retry-After": "nan"}))
        self.assertIsNone(server.retry_delay({"Retry-After": "bad"}))
        self.assertGreater(server.retry_delay({"Retry-After": formatdate(time.time() + 30, usegmt=True)}), 28)
        with patch.object(self.service.http, "open", wraps=self.service.http.open) as opening:
            self.start("nullable stream")
            messages = self.finished()
            provider = [call for call in opening.call_args_list if call.args[0].endswith("/chat/completions")]
        self.assertEqual(provider[0].kwargs["timeout"], 120)
        self.assertEqual(messages[-1]["parts"][0]["text"], "Hello from LangGraph.")
        self.assertNotIn("error", messages[-1]["info"])

    def test_auth_origin_host_and_body_bounds(self):
        self.assertEqual(self.request("GET", "/global/health")[1], {"healthy": True, "version": "1", "engine": "langgraph"})
        self.assertEqual(self.request("GET", "/global/health", headers={"Authorization": "Basic wrong"})[0], 401)
        self.assertEqual(self.request("GET", "/global/health", headers={"Origin": "https://evil.example"})[0], 403)
        self.assertEqual(self.request("GET", "/global/health", headers={"Host": "evil.example"})[0], 403)
        self.assertEqual(self.request("POST", "/session", headers={"Content-Length": str(server.MAX_BODY + 1)})[0], 400)
        with self.assertRaises(server.AgentError):
            server.endpoint("http://api.example/v1")
        with self.assertRaises(server.AgentError):
            server.endpoint("https://user:secret@api.example/v1")
        with self.assertRaises(server.AgentError):
            server.Http().json(self.fixture.url + "/redirect", {"Authorization": "Bearer secret"})

    def test_openrouter_lists_free_paid_and_manual_models(self):
        self.fixture.models = [{"id": "a:free"}, {"id": "zero", "pricing": {"prompt": "0", "completion": "0"}}, {"id": "paid", "pricing": {"prompt": "1", "completion": "2"}}]
        self.service.config["provider"]["openrouter"] = {"options": {"apiKey": "key", "baseURL": self.fixture.url + "/v1"}, "models": {"manual:free": {"name": "Manual"}, "manual-paid": {"name": "Paid"}}}
        providers = self.request("GET", "/provider")[1]
        free = next(item for item in providers["all"] if item["id"] == "openrouter")
        self.assertEqual(set(free["models"]), {"a:free", "zero", "paid", "manual:free", "manual-paid"})
        self.assertTrue(free["requiresApiKey"])
        self.assertEqual(free["baseURL"], self.fixture.url + "/v1")
        self.fixture.models_error = True
        other = server.AgentService({"provider": {"custom": {"options": {"baseURL": self.fixture.url + "/v1"}, "models": {"manual": {"name": "Manual"}}}}}, Path(self.directory.name) / "manual", self.service.token)
        providers = other.providers()
        self.assertIn("custom", providers["connected"])
        self.assertIn("manual", next(item for item in providers["all"] if item["id"] == "custom")["models"])

    def test_corrupt_history_is_preserved_and_incomplete_turn_is_recovered(self):
        path = Path(self.directory.name) / "langgraph-sessions.json"
        path.write_text('{"schemaVersion":1,"sessions":[{"id":4}]}')
        recovered = server.AgentService(self.config, Path(self.directory.name), self.service.token)
        self.assertEqual(recovered.sessions, {})
        self.assertTrue(list(path.parent.glob("langgraph-sessions.corrupt-*.json")))
        sid = recovered.create_session("Interrupted")["id"]
        recovered._append_message(sid, "assistant", "unfinished")
        recovered.save()
        restarted = server.AgentService(self.config, path.parent, self.service.token)
        info = restarted.sessions[sid]["messages"][-1]["info"]
        self.assertTrue(info["time"]["completed"])
        self.assertEqual(info["error"]["name"], "Interrupted")

    def test_unknown_tool_is_never_dispatched(self):
        self.start("unknown")
        messages = self.finished()
        self.assertEqual(self.fixture.calls, [])
        self.assertIn("not permitted", json.dumps(messages))

    def test_only_one_chat_can_control_the_workspace_bridge(self):
        self.start("write")
        self.permission()
        another = self.service.create_session("Other")["id"]
        self.assertEqual(self.request("POST", f"/session/{another}/prompt_async", {"model": {"providerID": "custom", "modelID": "test-model"}, "parts": [{"type": "text", "text": "hello"}]})[0], 400)
        self.request("POST", f"/session/{self.session}/abort")


if __name__ == "__main__":
    unittest.main(verbosity=2)

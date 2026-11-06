"""Exercise the shipped MCP companion without model access or real workspaces."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import tempfile
import threading


class Client:
    def __init__(self, executable, directory):
        env = os.environ.copy()
        env['AETHERDESK_CONNECTOR_DIRECTORY'] = str(directory)
        self.process = subprocess.Popen([str(executable)], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                        env=env, creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
        self.responses = queue.Queue()
        self.next_id = 0
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self):
        for line in self.process.stdout:
            try:
                self.responses.put(json.loads(line))
            except ValueError:
                self.responses.put({'invalid_stdout': True})
        self.responses.put({'eof': True})

    def send(self, value):
        self.process.stdin.write(json.dumps(value).encode() + b'\n')
        self.process.stdin.flush()

    def call(self, method, params):
        self.next_id += 1
        self.send({'jsonrpc': '2.0', 'id': self.next_id, 'method': method, 'params': params})
        response = self.responses.get(timeout=10)
        assert response.get('id') == self.next_id, response
        assert 'error' not in response, response
        return response['result']

    def close(self):
        try:
            self.process.stdin.close()
        except (BrokenPipeError, OSError):
            pass
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
            raise AssertionError('Companion failed to stop after its input closed')
        self.process.stdout.close()
        self.process.stderr.close()
        self.reader.join(timeout=1)


def verify(executable):
    with tempfile.TemporaryDirectory(prefix='aetherdesk-mcp-') as directory:
        client = Client(executable, directory)
        try:
            result = client.call('initialize', {'protocolVersion': '2025-06-18',
                'capabilities': {}, 'clientInfo': {'name': 'AetherDesk packaging check', 'version': '1'}})
            assert result['serverInfo']['name'] == 'AetherDesk'
            assert result['protocolVersion'] == '2025-06-18'
            client.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
            tools = client.call('tools/list', {})['tools']
            assert len(tools) == 8
            names = {tool['name'] for tool in tools}
            assert {'aetherdesk_status', 'aetherdesk_list_connections', 'aetherdesk_run_command'} <= names
            assert len(names) == len(tools)
            assert all(tool['inputSchema']['additionalProperties'] is False for tool in tools)
            status = client.call('tools/call', {'name': 'aetherdesk_status', 'arguments': {}})
            assert json.loads(status['content'][0]['text'])['connected'] is False
            refused = client.call('tools/call', {'name': 'aetherdesk_run_command',
                'arguments': {'command': 'must never run'}})
            assert refused['isError'] is True
            refused = client.call('tools/call', {'name': 'aetherdesk_read_file',
                'arguments': {'workspace_id': 'not-shared', 'path': 'file'}})
            assert refused['isError'] is True
            client.process.stdin.write(b'{invalid-json}\n')
            client.process.stdin.flush()
            assert client.responses.get(timeout=5)['error']['code'] == -32700
            assert client.call('ping', {}) == {}
        finally:
            client.close()
        assert client.process.returncode == 0
        bounded = Client(executable, directory)
        try:
            try:
                bounded.process.stdin.write(b'x' * (2 * 1024 * 1024 + 2) + b'\n')
                bounded.process.stdin.flush()
            except (BrokenPipeError, OSError):
                pass
            bounded.process.wait(timeout=10)
            assert bounded.process.returncode != 0, 'Oversized frames must be rejected'
        finally:
            try:
                bounded.close()
            except (BrokenPipeError, OSError):
                pass
    return dict(schema=1, handshakePassed=True, toolCatalogPassed=True,
                disabledAccessPassed=True, boundedInputPassed=True,
                liveHostAcceptancePassed=False,
                executableSha256=hashlib.sha256(executable.read_bytes()).hexdigest())


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--executable', type=Path, required=True)
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    report = verify(args.executable)
    if args.report:
        args.report.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print('PASS: packaged MCP handshake, catalog, closed-GUI access refusal, bounded input and shutdown.')

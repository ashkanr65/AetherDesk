"""Verify the packaged LangGraph runtime with no developer tools on PATH."""
from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

from assistant_bundle import read_pin, safe_relative, sha256


def verify_files(package: Path) -> tuple[Path, dict]:
    pin = read_pin()
    runtime = package.resolve(strict=True) / 'ai' / 'assistant'
    metadata = json.loads((runtime / 'bundle-info.json').read_text(encoding='utf-8-sig'))
    for key in ('schemaVersion', 'engine', 'version', 'platform', 'executable', 'entrypoint', 'langgraphVersion'):
        if metadata.get(key) != pin[key]:
            raise ValueError('Assistant package does not match the source-owned runtime pin')
    if metadata.get('pythonVersion') != pin['python']['version']:
        raise ValueError('Assistant package Python version does not match its pin')
    if json.loads((runtime / 'runtime-lock.json').read_text(encoding='utf-8-sig')) != pin:
        raise ValueError('Assistant runtime dependency lock does not match its source pin')
    files = metadata.get('files', [])
    names = {x.get('path') for x in files}
    actual = {x.relative_to(runtime).as_posix() for x in runtime.rglob('*') if x.is_file() and x.name != 'bundle-info.json'}
    if (not files or len({x.casefold() for x in names}) != len(files) or names != actual
            or not {'python/python.exe', 'python/python313._pth', 'python/LICENSE.txt', 'server.py', 'runtime-lock.json'}.issubset(names)):
        raise ValueError('Assistant manifest must describe every packaged file exactly once')
    for entry in files:
        name = entry['path']
        if not safe_relative(name):
            raise ValueError('Unsafe assistant manifest path')
        file = runtime / name
        if (file.is_symlink() or any(x.is_symlink() for x in file.parents if x != runtime.parent)
                or not file.is_file() or file.stat().st_size != entry.get('size') or sha256(file) != entry.get('sha256')):
            raise ValueError(f'Packaged assistant file failed integrity verification: {name}')
    if (runtime / 'python' / 'python313._pth').read_text(encoding='utf-8') != 'python313.zip\n.\n../packages\n..\n':
        raise ValueError('Embedded Python import isolation has changed')
    for artifact in pin['licenses']:
        if sha256(runtime / 'licenses' / artifact['filename']) != artifact['sha256']:
            raise ValueError('Assistant dependency license failed verification')
    return runtime / 'python' / 'python.exe', metadata


def smoke_test(executable: Path, version: str) -> None:
    if os.name != 'nt':
        raise RuntimeError('The bundled assistant runtime must be smoke-tested on Windows')
    with tempfile.TemporaryDirectory(prefix='aetherdesk-assistant-smoke-') as temporary:
        isolated = Path(temporary)
        env = {key:value for key,value in os.environ.items() if key.upper() in
               {'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'PATHEXT', 'TEMP', 'TMP', 'SYSTEMDRIVE'}}
        env.update({'PATH':str(Path(os.environ['SystemRoot']) / 'System32'),
                    'HOME':temporary, 'USERPROFILE':temporary,
                    'APPDATA':str(isolated / 'appdata'), 'LOCALAPPDATA':str(isolated / 'localappdata'),
                    'AETHERDESK_ASSISTANT_TOKEN':secrets.token_hex(32),
                    'AETHERDESK_ASSISTANT_DATA':str(isolated / 'data'),
                    'AETHERDESK_ASSISTANT_CONFIG':json.dumps({'provider':{}, 'mcp':{}}),
                    'LANGSMITH_TRACING':'false', 'LANGCHAIN_TRACING_V2':'false'})
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            port = listener.getsockname()[1]
        base = f'http://127.0.0.1:{port}'
        authorization = 'Basic ' + base64.b64encode(f'aetherdesk:{env["AETHERDESK_ASSISTANT_TOKEN"]}'.encode()).decode()
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

        def request(route, body=None, *, authenticated=True):
            headers = {'Content-Type':'application/json'}
            if authenticated:
                headers['Authorization'] = authorization
            data = None if body is None else json.dumps(body).encode()
            with opener.open(urllib.request.Request(base + route, data=data, headers=headers), timeout=10) as response:
                return json.load(response)

        with (isolated / 'server.log').open('wb') as log:
            process = subprocess.Popen([str(executable), '-B', str(executable.parent.parent / 'server.py'), '--port',str(port)],
                cwd=isolated, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                creationflags=subprocess.CREATE_NO_WINDOW)
            try:
                deadline = time.monotonic() + 45
                while True:
                    if process.poll() is not None:
                        raise RuntimeError(f'Bundled assistant exited during startup ({process.returncode})')
                    try:
                        health = request('/global/health')
                        break
                    except urllib.error.URLError:
                        if time.monotonic() >= deadline:
                            raise RuntimeError('Bundled assistant did not become ready')
                        time.sleep(.1)
                if health != {'healthy':True, 'version':version, 'engine':'langgraph'}:
                    raise ValueError(f'Unexpected bundled assistant health: {health}')
                try:
                    request('/global/health', authenticated=False)
                except urllib.error.HTTPError as error:
                    if error.code != 401:
                        raise
                else:
                    raise ValueError('Assistant server accepted an unauthenticated request')
                providers = request('/provider')
                if not providers.get('all'):
                    raise ValueError('Bundled assistant has no embedded providers')
                session = request('/session', {'title':'AetherDesk packaged assistant smoke test'})
                if not session.get('id'):
                    raise ValueError('Bundled assistant failed to create a conversation')
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=5)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--package', type=Path, required=True)
    parser.add_argument('--files-only', action='store_true')
    args = parser.parse_args()
    executable, metadata = verify_files(args.package)
    if not args.files_only:
        smoke_test(executable, metadata['version'])
        # Exercise the packaged code/dependencies, including Windows Stop,
        # approval handling and free/paid model selection, using local fixtures only.
        environment = dict(os.environ)
        environment['AETHERDESK_ASSISTANT_TEST_SERVER'] = str(executable.parent.parent / 'server.py')
        subprocess.run([str(executable), '-I', '-B', str(Path(__file__).resolve().parents[1] / 'assistant-runtime' / 'test_server.py')],
                       env=environment, check=True, timeout=120)
    print('PASS: bundled assistant integrity' + ('' if args.files_only else ', authenticated LangGraph server without installed Python/Node/Bun'))


if __name__ == '__main__':
    main()

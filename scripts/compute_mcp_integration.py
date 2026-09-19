#!/usr/bin/env python3
"""Official MCP SDK -> Porta stdio -> real WASM, without implicit directory grants."""
import asyncio
import json
import pathlib
import sys
import tempfile
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from mcp.shared.exceptions import McpError

porta, compute, demo = [str(pathlib.Path(p).resolve()) for p in sys.argv[1:4]]
repo = pathlib.Path(__file__).resolve().parents[1]


async def main():
    with tempfile.TemporaryDirectory(prefix='porta-compute-mcp-') as directory:
        root = pathlib.Path(directory)
        (root / 'marker.txt').write_text('explicit grant only')
        params = StdioServerParameters(command=porta, cwd=directory, args=[
            'serve', compute, '--manifest', str(repo / 'examples/compute/compute.manifest.json'),
            '--step-limit', '50000000', '--max-memory', '1024'])
        async with stdio_client(params) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                listed = await session.list_tools()
                assert any(t.name == 'compute' for t in listed.tools)
                for script, text, expected in [('input.a + input.b', '{"a":0.1,"b":0.2}', '0.3'),
                                                ('input', '9007199254740993', '9007199254740993')]:
                    result = await session.call_tool('compute', {'script': script, 'input_json': text})
                    assert not result.isError, result
                    assert json.loads(result.content[0].text) == {'ok': True, 'json': expected}, result
                result = await session.call_tool('compute', {'script': '1 / 0', 'input_json': 'null'})
                assert result.isError and json.loads(result.content[0].text)['error']['code'] == 'compute_failed', result
                try:
                    await session.call_tool('porta.exec', {'command': 'echo', 'args': ['not granted']})
                except McpError as error:
                    assert 'capability not granted' in str(error), error
                else:
                    raise AssertionError('native execution unexpectedly granted')
        print('PASS: official SDK receives full compute results, exact numeric text and structured errors; native execution is denied')

        for mounted in (False, True):
            params = StdioServerParameters(command=porta, cwd=directory, args=[
                'serve', demo, '--manifest', str(repo / 'examples/demo-agent/manifest.json'),
                '--step-limit', '50000000', '--max-memory', '1024'] + (['-v', '.'] if mounted else []))
            async with stdio_client(params) as (read, write):
                async with ClientSession(read, write) as session:
                    await session.initialize()
                    result = await session.call_tool('read_file', {'path': 'marker.txt'})
                    if mounted:
                        assert not result.isError and result.content[0].text == 'explicit grant only', result
                    else:
                        assert result.isError, result
                    written = await session.call_tool('write_file', {'path':'created.txt', 'content':'explicit'})
                    if mounted:
                        assert not written.isError and (root / 'created.txt').read_text() == 'explicit', written
                    else:
                        assert written.isError and not (root / 'created.txt').exists(), written
        print('PASS: declared filesystem capability alone exposes no cwd; explicit mounts enable legacy reads and writes')


asyncio.run(asyncio.wait_for(main(), timeout=60))

from evals.compute_tool import compute as shared_compute
for script, expected in [('input + 0.2', True), ('1 / 0', False)]:
    result, wire = shared_compute(porta, compute, repo / 'examples/compute/compute.manifest.json',
                                  {'script': script, 'input_json': '0.1'})
    assert result['ok'] is expected and wire['isError'] is (not expected)
    if expected:
        assert result['json'] == '0.3'
print('PASS: shared evaluation adapter preserves WASM success and failure payloads')

async def nested_loop():
    result, _ = shared_compute(porta, compute, repo / 'examples/compute/compute.manifest.json',
                               {'script': 'input + 0.2', 'input_json': '0.1'})
    assert result == {'ok': True, 'json': '0.3'}

asyncio.run(nested_loop())
print('PASS: synchronous compute adapter works when its caller already owns an event loop')

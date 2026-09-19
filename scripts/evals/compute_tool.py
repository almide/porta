"""The same real WASM computation service for both evaluated agents."""
import asyncio
import json
import tempfile
from concurrent.futures import ThreadPoolExecutor
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client


async def invoke(porta, wasm, manifest, arguments):
    # No -v/-e flags; fresh WASI memory, empty environment and no preopens.
    with tempfile.TemporaryDirectory(prefix='porta-shared-compute-') as directory:
        params = StdioServerParameters(command=str(porta), cwd=directory, args=[
            'serve', str(wasm), '--manifest', str(manifest),
            '--step-limit', '50000000', '--max-memory', '1024'])
        async with stdio_client(params) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                reply = await session.call_tool('compute', arguments)
                wire = reply.model_dump(by_alias=True, exclude_none=True)
                if len(reply.content) != 1 or reply.content[0].type != 'text':
                    raise ValueError('unexpected compute MCP content')
                result = json.loads(reply.content[0].text)
                if not isinstance(result, dict) or not isinstance(result.get('ok'), bool):
                    raise ValueError('compute response lost its status/result envelope')
                if bool(reply.isError) != (not result['ok']):
                    raise ValueError('compute status differs from MCP isError')
                return result, wire


def compute(porta, wasm, manifest, arguments):
    # FastMCP may invoke synchronous tools from its running event loop. Own the
    # stdio client's event loop in a worker instead of nesting asyncio.run.
    def run():
        return asyncio.run(asyncio.wait_for(invoke(porta, wasm, manifest, arguments), timeout=20))
    with ThreadPoolExecutor(max_workers=1) as executor:
        return executor.submit(run).result()

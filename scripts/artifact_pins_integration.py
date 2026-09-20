#!/usr/bin/env python3
"""Content-pinned WASM/team loading, strict inheritance and frozen bytes."""
import hashlib
import http.server
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
import threading
from wasm_fixtures import fixed_stdout_guest

porta, agent, tool = [str(pathlib.Path(p).resolve()) for p in sys.argv[1:4]]
requests=[]
mutate=None
class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        global mutate
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])));requests.append(body)
        results=[m for m in body['messages'] if m['role']=='tool']
        if mutate:
            action=mutate;mutate=None;action()
        if body['model'] in ('root','tool') and not results:
            name,args=('delegate_worker',{'task':'finish'}) if body['model']=='root' else ('add',{'a':20,'b':22})
            message={'role':'assistant','tool_calls':[{'id':'call-1','type':'function','function':{'name':name,'arguments':json.dumps(args)}}]}
        else:
            message={'role':'assistant','content':results[-1]['content'] if results else 'done'}
        data=json.dumps({'choices':[{'message':message}]}).encode()
        self.send_response(200);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
    def log_message(self,*args):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Model)
thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='porta-pins-') as directory:
        root=pathlib.Path(directory).resolve()
        for source,name in ((agent,'agent.wasm'),(tool,'tools.wasm')):shutil.copyfile(source,root/name)
        for name in ('before.wasm','complete.wasm'):(root/name).write_bytes(fixed_stdout_guest({'passed':True}))
        originals={name:(root/name).read_bytes() for name in ('agent.wasm','tools.wasm','before.wasm','complete.wasm')}
        digest=lambda path:hashlib.sha256(path.read_bytes()).hexdigest()
        def config_text(model='worker',strict=True,pins=True):
            text=f'''version=1
require_artifact_hashes={str(strict).lower()}
[agent]
wasm="agent.wasm"
'''
            if pins:text+=f'sha256="{digest(root/"agent.wasm")}"\n'
            text+=f'''[model]
endpoint="http://127.0.0.1:{server.server_port}/model"
name="{model}"
'''
            for section,name,file in [('tools','add','tools.wasm'),('before_tool_checks','before','before.wasm'),('completion_checks','complete','complete.wasm')]:
                text+=f'[[{section}]]\nname="{name}"\nwasm="{file}"\n'
                if pins:text+=f'sha256="{digest(root/file)}"\n'
            return text
        config=root/'agent.toml';child=root/'child.toml'
        def parent():return config_text('root')+f'[[agents]]\nname="worker"\nconfig="child.toml"\nsha256="{digest(child)}"\n'
        def run(*args):return subprocess.run([porta,*map(str,args)],capture_output=True,text=True,timeout=15)
        def success(r):assert r.returncode==0,r.stderr;return r
        def failure(r,text):assert r.returncode!=0 and text in r.stderr,r
        child.write_text(config_text(strict=False));base=parent();config.write_text(base)
        journal=root/'team.jsonl';success(run('agent',config,'--record',journal,'--','finish'))
        count=len(requests);success(run('agent-replay',config,journal));assert len(requests)==count
        print('PASS: fully pinned agent/tool/check/team executes and replays offline')
        for name,original in originals.items():
            (root/name).write_bytes(original+b'\x00\x05\x04test')
            count=len(requests)
            failure(run('agent',config,'--','finish'),'sha256 mismatch')
            failure(run('agent-replay',config,journal),'sha256 mismatch')
            assert len(requests)==count
            (root/name).write_bytes(original)
        original_child=child.read_text();child.write_text(original_child+'\n# changed policy\n')
        count=len(requests);failure(run('agent',config,'--','finish'),'sha256 mismatch for agent config');assert len(requests)==count
        child.write_text(original_child)
        print('PASS: changed module bytes or delegated configuration are rejected before model requests, including replay')
        # Explicit false in a descendant cannot weaken its parent's strict mode.
        child.write_text(config_text(strict=False,pins=False));config.write_text(parent());count=len(requests)
        failure(run('agent',config,'--','finish'),'sha256 is required for WASM');assert len(requests)==count
        child.write_text(original_child);config.write_text(base.replace(f'sha256="{digest(child)}"\n',''))
        failure(run('agent',config,'--','finish'),'sha256 is required for agent config')
        # A pinned child cannot smuggle an unpinned grandchild configuration.
        grand=root/'grand.toml';grand.write_text(config_text(strict=False))
        child.write_text(original_child+'[[agents]]\nname="grand"\nconfig="grand.toml"\n')
        config.write_text(parent());failure(run('agent',config,'--','finish'),'sha256 is required for agent config')
        assert len(requests)==count;child.write_text(original_child);config.write_text(base)
        print('PASS: strict mode propagates through the team and requires descendant module/config pins')
        for name in originals:
            text=config_text().replace(f'wasm="{name}"\nsha256="{digest(root/name)}"', f'wasm="{name}"')
            config.write_text(text);count=len(requests)
            failure(run('agent',config,'--','finish'),'sha256 is required for WASM')
            assert len(requests)==count
        for bad in ('', '0'*63, 'A'*64, 'g'*64, 'sha256:'+'0'*64):
            config.write_text(config_text(strict=False).replace(digest(root/'agent.wasm'),bad))
            count=len(requests);failure(run('agent',config,'--','finish'),'64 lowercase hexadecimal');assert len(requests)==count
        config.write_text(config_text(strict=False).replace(digest(root/'agent.wasm'),'0'*64))
        failure(run('agent',config,'--','finish'),'sha256 mismatch')
        config.write_text(config_text(strict=False,pins=False));success(run('agent',config,'--','finish'))
        print('PASS: optional pins are still enforced; malformed pins fail closed; unpinned compatibility remains explicit')
        # Change a tool after launch: the pinned, compiled bytes remain in memory.
        config.write_text(config_text('tool'))
        mutate=lambda:(root/'tools.wasm').write_bytes(fixed_stdout_guest({'ok':'tampered'}))
        result=success(run('agent',config,'--','add'))
        assert json.loads(result.stdout)=={'ok':'42'},result
        count=len(requests);failure(run('agent',config,'--','add'),'sha256 mismatch');assert len(requests)==count
        (root/'tools.wasm').write_bytes(originals['tools.wasm'])
        # Hash validation precedes compilation and authenticating bytes cannot make invalid WASM executable.
        config.write_text(config_text());(root/'agent.wasm').write_bytes(b'not wasm')
        failure(run('agent',config,'--','finish'),'sha256 mismatch')
        config.write_text(config_text());failure(run('agent',config,'--','finish'),'compile WASM')
        print('PASS: execution uses the same verified bytes; next launch rejects replacement; matching digest does not bypass WASM validation')
finally:
    server.shutdown();server.server_close();thread.join()

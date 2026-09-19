#!/usr/bin/env python3
"""Offline configuration inspection: no credentials, networking or guest execution."""
import hashlib
import http.server
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
from wasm_fixtures import section, string as wasm_string

porta,agent,tool=[str(pathlib.Path(p).resolve()) for p in sys.argv[1:4]]
traffic=[]
class Reject(http.server.BaseHTTPRequestHandler):
    def do_POST(self):traffic.append(self.path);self.send_error(500)
    def do_GET(self):traffic.append(self.path);self.send_error(500)
    def log_message(self,*args):pass
def import_probe(name, valid_signature=False):
    # Import one function; a distinct () -> () function exports _start.
    signature = bytes.fromhex('60047f7f7f7f017f') if valid_signature else bytes.fromhex('600000')
    types = b'\x02' + signature + bytes.fromhex('600000')
    imports = b'\x01' + wasm_string('wasi_snapshot_preview1') + wasm_string(name) + b'\x00\x00'
    exports = b'\x01' + wasm_string('_start') + b'\x00\x01'
    return (bytes.fromhex('0061736d01000000') + section(1, types) + section(2, imports)
            + section(3, b'\x01\x01') + section(7, exports) + section(10, b'\x01\x02\x00\x0b'))

server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Reject)
thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='porta-check-') as directory:
        root=pathlib.Path(directory).resolve();work=root/'work';work.mkdir()
        marker=work/'marker.txt';marker.write_text('unchanged')
        # Both a module start section and _start trap if ever instantiated/executed.
        trap=root/'trap.wasm';trap.write_bytes(bytes.fromhex('0061736d0100000001040160000003020100070a01065f737461727400000801000a05010300000b'))
        digest=lambda p:hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
        endpoint=f'http://127.0.0.1:{server.server_port}'
        def cfg(strict, steps=64):return f'''version=1
require_artifact_hashes={str(strict).lower()}
[agent]
wasm={json.dumps(str(trap))}
sha256="{digest(trap)}"
[model]
endpoint="{endpoint}/model"
name="inspection-only"
token_env="PORTA_CHECK_SECRET"
[limits]
max_steps={steps}
max_model_calls=7
[[tools]]
name="write_file"
wasm={json.dumps(tool)}
sha256="{digest(tool)}"
mounts=[{{host="work",guest=".",read_only=false}}]
input_schema={{type="object"}}
[[mcp_tools]]
name="remote"
remote_name="upstream"
endpoint="{endpoint}/mcp"
token_env="PORTA_MCP_CHECK_SECRET"
input_schema={{type="object"}}
[[before_tool_checks]]
name="before"
wasm={json.dumps(str(trap))}
sha256="{digest(trap)}"
[[completion_checks]]
name="after"
wasm={json.dumps(str(trap))}
sha256="{digest(trap)}"
mounts=[{{host="work",guest="."}}]
'''
        child=root/'child.toml';child.write_text(cfg(False,99))
        config=root/'root.toml'
        base=cfg(True,32)+f'[[agents]]\nname="worker"\nconfig="child.toml"\nsha256="{digest(child)}"\n'
        config.write_text(base)
        def run(*args,secret=False):
            env=dict(os.environ);env.pop('PORTA_CHECK_SECRET',None);env.pop('PORTA_MCP_CHECK_SECRET',None)
            if secret:env.update(PORTA_CHECK_SECRET='never-print-model-value',PORTA_MCP_CHECK_SECRET='never-print-mcp-value')
            r=subprocess.run([porta,*map(str,args)],env=env,capture_output=True,text=True,timeout=15)
            assert 'never-print-model-value' not in r.stdout+r.stderr and 'never-print-mcp-value' not in r.stdout+r.stderr
            return r
        def success(r):assert r.returncode==0,r.stderr;return r
        def failure(r,text):assert r.returncode!=0 and text in r.stderr,r
        first=success(run('agent-check',config));report=json.loads(first.stdout);team=report['team']
        assert report['valid'] is True and len(report['fingerprint'])==64
        assert team['strict_artifact_pins'] is True
        assert team['children']['delegate_worker']['strict_artifact_pins'] is True
        assert team['children']['delegate_worker']['limits']['local']['max_steps']==99
        assert team['children']['delegate_worker']['limits']['root']['max_steps']==32
        assert team['tools']['write_file']['mounts']==[{'host':str(work),'guest':'.','read_only':False}]
        assert team['checks'][0]['mounts'][0]['read_only'] is True
        assert team['agent']==digest(trap) and team['tools']['write_file']['wasm']==digest(tool)
        assert team['model']['token_env']=='PORTA_CHECK_SECRET'
        assert team['remote_tools'][0]['endpoint']==endpoint+'/mcp'
        assert json.loads(success(run('agent-check',config,secret=True)).stdout)==report
        assert marker.read_text()=='unchanged' and traffic==[]
        print('PASS: deterministic team inspection shows effective pins, mounts, endpoints and shared limits without credentials, network or trap execution')
        changed=base.replace('max_steps=32','max_steps=31');config.write_text(changed)
        assert json.loads(success(run('agent-check',config)).stdout)['fingerprint']!=report['fingerprint']
        config.write_text(base)
        failure(run('agent',config,'--','execute',secret=True),'instantiate guest')
        assert traffic==[] and marker.read_text()=='unchanged'
        # Pins, child policy and schemas are validated by the same loader as execution.
        child.write_text(child.read_text()+'\n# drift\n')
        failure(run('agent-check',config),'sha256 mismatch')
        child.write_text(cfg(False,99))
        config.write_text(base.replace('sha256="'+digest(trap)+'"','sha256="'+'0'*64+'"',1))
        failure(run('agent-check',config),'sha256 mismatch')
        config.write_text(base.replace('input_schema={type="object"}', 'input_schema={type="object",properties={x={"$ref"="https://invalid.example/schema"}}}',1))
        failure(run('agent-check',config),'invalid or unresolved input_schema')
        assert traffic==[]
        print('PASS: configuration fingerprints change; drift, bad pins and external schemas fail offline')
        missing=root/'missing.wasm';missing.write_bytes(bytes.fromhex('0061736d01000000'))
        config.write_text(cfg(False).replace(str(trap),str(missing)).replace(digest(trap),digest(missing)))
        failure(run('agent-check',config),'must export _start')
        # A valid module exporting _start(i32) is still an invalid Porta entry point.
        wrong=root/'wrong.wasm';wrong.write_bytes(bytes.fromhex('0061736d0100000001050160017f0003020100070a01065f737461727400000a040102000b'))
        config.write_text(cfg(False).replace(str(trap),str(wrong)).replace(digest(trap),digest(wrong)))
        failure(run('agent-check',config),'must export _start')
        assert traffic==[]
        imported=root/'import.wasm'
        for name, signature in [('porta_missing_function',False),('fd_write',False)]:
            imported.write_bytes(import_probe(name,signature))
            config.write_text(cfg(False).replace(str(trap),str(imported)).replace(digest(trap),digest(imported)))
            failure(run('agent-check',config),'WASI link validation')
            failure(run('agent',config,'--','execute',secret=True),'WASI link validation')
            # Invalid imports in an unused tool also reject the whole team.
            config.write_text(cfg(False).replace(tool,str(imported)).replace(digest(tool),digest(imported)))
            failure(run('agent-check',config),'WASI link validation')
            failure(run('agent',config,'--','execute',secret=True),'WASI link validation')
            assert traffic==[]
        imported.write_bytes(import_probe('fd_write',True))
        config.write_text(cfg(False).replace(str(trap),str(imported)).replace(digest(trap),digest(imported)))
        success(run('agent-check',config));assert traffic==[]
        print('PASS: nonexistent or incompatible WASI imports fail before execution, including unused tools; compatible imports pass')
        failure(run('agent-check'),'expected one agent config path')
        failure(run('agent-check',config,'extra'),'expected one agent config path')
        assert 'without model calls' in success(run('agent-check','--help')).stdout
        print('PASS: absent/wrong entry points and malformed CLI invocations fail before execution')
finally:
    server.shutdown();server.server_close();thread.join()

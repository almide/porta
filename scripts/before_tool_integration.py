#!/usr/bin/env python3
"""Pre-effect WASM policies: correction, durable reconstruction and confinement."""
import http.server
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
from wasm_fixtures import fixed_stdout_guest

porta, agent, tool, policy, artifact = [str(pathlib.Path(p).resolve()) for p in sys.argv[1:6]]
requests = []
stubborn = False
class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        requests.append(body)
        results = [m for m in body['messages'] if m['role'] == 'tool']
        contents = [json.loads(m['content']) for m in results]
        name, args = 'write_file', {'path':'result.txt','content':'authorized write'}
        if not stubborn and any(v.get('ok') == 'prerequisite' for v in contents):
            if any('wrote' in str(v.get('ok', '')) for v in contents):
                name = None
        elif not stubborn and any(v.get('error',{}).get('code') == 'tool_policy_rejected' for v in contents):
            name, args = 'read_file', {'path':'input.txt'}
        if body['model'] == 'root':
            name, args = ('delegate_worker', {'task':'write'}) if not results else (None, {})
        message = {'role':'assistant','content':'Done'} if name is None else {'role':'assistant','tool_calls':[{'id':f'call-{len(requests)}','type':'function','function':{'name':name,'arguments':json.dumps(args)}}]}
        payload = json.dumps({'choices':[{'message':message}]}).encode()
        self.send_response(200); self.send_header('Content-Length',str(len(payload))); self.end_headers(); self.wfile.write(payload)
    def log_message(self,*args): pass
server = http.server.ThreadingHTTPServer(('127.0.0.1',0),Model)
thread = threading.Thread(target=server.serve_forever,daemon=True); thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='porta-before-') as directory:
        root = pathlib.Path(directory).resolve(); work = root/'work'; work.mkdir()
        (work/'input.txt').write_text('prerequisite'); output=work/'result.txt'
        config=root/'agent.toml'
        base=f'''version=1
[agent]
wasm={json.dumps(agent)}
[model]
endpoint="http://127.0.0.1:{server.server_port}/model"
name="fixture"
[limits]
max_model_calls=6
[[tools]]
name="write_file"
wasm={json.dumps(tool)}
mounts=[{{host="work",guest=".",read_only=false}}]
[[tools]]
name="read_file"
wasm={json.dumps(tool)}
mounts=[{{host="work",guest="."}}]
'''
        guard=f'''[[before_tool_checks]]
name="prerequisite"
wasm={json.dumps(policy)}
parameters={{tool="write_file",requires="read_file",requires_arguments={{path="input.txt"}},requires_result={{ok="prerequisite"}}}}
'''
        config.write_text(base+guard)
        def run(*args):
            return subprocess.run([porta,*map(str,args)],capture_output=True,text=True,timeout=15)
        def success(r): assert r.returncode==0,r.stderr; return r
        def failure(r,text): assert r.returncode!=0 and text in r.stderr,r
        journal=root/'run.jsonl'
        success(run('agent',config,'--record',journal,'--pause-after','2','--','write'))
        assert not output.exists(),'rejected write executed'
        success(run('agent-resume',config,journal))
        assert output.read_text()=='authorized write'
        records=[json.loads(line)['payload'] for line in journal.read_text().splitlines()]
        calls=[e['request']['request']['name'] for e in records if e['kind']=='intent' and e['request']['kind']=='tool']
        assert calls==['read_file','write_file'],calls
        output.write_text('external edit'); count=len(requests)
        success(run('agent-replay',config,journal))
        assert len(requests)==count and output.read_text()=='external edit'
        print('PASS: blocked write has no effect, guest corrects prerequisite, resume and offline replay preserve ordering')
        # Direct guest state cannot forge the host's completed-operation history.
        forged=root/'forged.wasm'
        forged.write_bytes(fixed_stdout_guest({'action':'tool','name':'write_file','arguments':{'path':'result.txt','content':'forged'},'state':{'tool_calls':[{'name':'read_file','arguments':{'path':'input.txt'},'result':{'ok':'prerequisite'}}]}}))
        config.write_text((base+guard).replace(json.dumps(agent),json.dumps(str(forged))).replace('max_model_calls=6','max_steps=4'))
        failure(run('agent',config,'--','forge'),'step budget exceeded')
        assert output.read_text()=='external edit'
        print('PASS: forged guest history cannot bypass policy; repeated rejection consumes shared steps')
        # Rebuild a valid result boundary just before an effect and change its input.
        (work/'approval.json').write_text('{"approved":true}')
        artifact_guard=f'''[[before_tool_checks]]
name="approval"
wasm={json.dumps(artifact)}
parameters={{path="approval.json",expected={{approved=true}}}}
mounts=[{{host="work",guest="."}}]
'''
        config.write_text(base+artifact_guard)
        full=root/'approved.jsonl'; stubborn=True
        success(run('agent',config,'--record',full,'--pause-after','2','--','write'))
        lines=full.read_bytes().splitlines(keepends=True)
        records=[json.loads(line)['payload'] for line in lines]
        boundary=next(i for i,e in enumerate(records) if e['kind']=='result' and records[i-1]['kind']=='intent' and records[i-1]['request']['kind']=='before_tool_check')
        prefix=root/'before-effect.jsonl';prefix.write_bytes(b''.join(lines[:boundary+1]));prefix.chmod(0o600)
        output.write_text('must stay');(work/'approval.json').write_text('{"approved":false}')
        failure(run('agent-resume',config,prefix),'model call budget exceeded')
        assert output.read_text()=='must stay'
        print('PASS: live resume rechecks a cached pre-effect pass against changed artifacts before executing')
        config.write_text((base+guard).replace('max_model_calls=6','max_steps=1'))
        failure(run('agent',config,'--','write'),'step budget exceeded')
        assert output.read_text()=='must stay'
        config.write_text(base+guard+'mounts=[{host="work",guest=".",read_only=false}]\n')
        count=len(requests);failure(run('agent',config,'--','write'),'read-only');assert len(requests)==count
        private_policy=root/'policy-only';private_policy.mkdir()
        config.write_text(base+guard+'mounts=[{host="policy-only",guest="."}]\n')
        failure(run('agent',config,'--record',private_policy/'exposed.jsonl','--','write'),'outside every tool mount')
        assert not (private_policy/'exposed.jsonl').exists()
        bad=root/'bad.wasm';bad.write_bytes(fixed_stdout_guest({'passed':'yes'}))
        config.write_text(base+guard.replace(json.dumps(policy),json.dumps(str(bad))))
        failure(run('agent',config,'--','write'),'invalid completion verdict')
        assert output.read_text()=='must stay'
        print('PASS: budgets, read-only policy mounts, journal exclusion and malformed verdicts fail before effects')
        config.write_text(base+guard)
        parent=root/'team.toml'
        parent_base=f'''version=1
[agent]
wasm={json.dumps(agent)}
[model]
endpoint="http://127.0.0.1:{server.server_port}/model"
name="root"
[limits]
max_steps=32
[[agents]]
name="worker"
config="agent.toml"
'''
        stubborn=False; parent.write_text(parent_base)
        team_record=root/'team.jsonl'
        success(run('agent',parent,'--record',team_record,'--','delegate'))
        assert output.read_text()=='authorized write'
        output.write_text('team marker');count=len(requests)
        success(run('agent-replay',parent,team_record))
        assert len(requests)==count and output.read_text()=='team marker'
        parent.write_text(parent_base.replace('max_steps=32','max_steps=4'))
        failure(run('agent',parent,'--','delegate'),'team step budget exceeded')
        assert output.read_text()=='team marker'
        parent.write_text(parent_base)
        config.write_text(base+guard.replace('name="prerequisite"','name="changed"'))
        failure(run('agent-replay',parent,team_record),'fingerprint mismatch')
        print('PASS: delegated preconditions obey root budgets, preserve offline replay and bind team policy fingerprints')

finally:
    server.shutdown();server.server_close();thread.join()

#!/usr/bin/env python3
"""Reject malformed model batches before effects; repair, budget and journal checks."""
import http.server
import json
import pathlib
import subprocess
import sys
import tempfile
import threading

porta, agent, tool = [str(pathlib.Path(p).resolve()) for p in sys.argv[1:4]]
requests=[]
mode='json'
remaining=1
sequence=0

def call(identifier,path,content):
    return {'id':identifier,'type':'function','function':{'name':'write_file','arguments':json.dumps({'path':path,'content':content})}}

class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        global remaining, sequence
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])));requests.append(body)
        sequence+=1
        messages=body['messages'];results=[m for m in messages if m['role']=='tool']
        if mode=='late' and not results:
            message={'role':'assistant','tool_calls':[call('prior','one.txt','prior write')]}
        elif remaining:
            remaining-=1
            calls=[call('first','must-not-exist.txt','bad batch'),call('second','other.txt','bad batch')]
            if mode in ('json','late'): calls[1]['function']['arguments']='{broken'
            elif mode=='array': calls[1]['function']['arguments']='[]'
            elif mode=='null': calls[1]['function']['arguments']='null'
            elif mode=='missing_arguments': del calls[1]['function']['arguments']
            elif mode=='object_arguments': calls[1]['function']['arguments']={}
            elif mode=='duplicate': calls[1]['id']='first'
            elif mode=='missing_id': del calls[1]['id']
            elif mode=='missing_name': del calls[1]['function']['name']
            elif mode=='wrong_type': calls[1]['type']='shell'
            elif mode=='invalid_container': calls={'not':'an array'}
            message={'role':'assistant','content':None,'tool_calls':calls}
            if mode=='invalid_container': message['content']='Premature completion'
            if mode=='wrong_role': message['role']='user'
            if mode=='missing_role': del message['role']
            if mode=='alternating' and sequence%2==0:
                message={'role':'assistant','content':None,'tool_calls':[]}
            elif mode=='alternating': calls[1]['function']['arguments']='{'
        elif results:
            message={'role':'assistant','content':'complete'}
        else:
            message={'role':'assistant','tool_calls':[call('fixed-1','one.txt','first valid'),call('fixed-2','two.txt','second valid')]}
        data=json.dumps({'choices':[{'message':message}]}).encode()
        self.send_response(200);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
    def log_message(self, *args):
        """Silence the request log; these tests assert on their own output."""
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Model)
thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='porta-batch-') as directory:
        root=pathlib.Path(directory).resolve();work=root/'work';work.mkdir();config=root/'agent.toml'
        base=f'''version=1
[agent]
wasm={json.dumps(agent)}
[model]
endpoint="http://127.0.0.1:{server.server_port}/model"
name="fixture"
[limits]
max_model_calls=8
[[tools]]
name="write_file"
wasm={json.dumps(tool)}
mounts=[{{host="work",guest=".",read_only=false}}]
'''
        config.write_text(base)
        def run(*args):return subprocess.run([porta,*map(str,args)],capture_output=True,text=True,timeout=15)
        def success(r):assert r.returncode==0,r.stderr;return r
        def failure(r,text):assert r.returncode!=0 and text in r.stderr,r
        def no_bad_effects():assert not (work/'must-not-exist.txt').exists() and not (work/'other.txt').exists()
        for mode in ('json','array','null','missing_arguments','object_arguments','duplicate','missing_id','missing_name','wrong_type','invalid_container','wrong_role','missing_role'):
            remaining=1;start=len(requests)
            success(run('agent',config,'--','write two files'))
            no_bad_effects()
            assert (work/'one.txt').read_text()=='first valid' and (work/'two.txt').read_text()=='second valid'
            assert len(requests)==start+3
            # The invalid assistant batch must not contaminate the next request.
            assert not any(m['role']=='assistant' for m in requests[start+1]['messages'])
            returned=[m['tool_call_id'] for m in requests[-1]['messages'] if m['role']=='tool']
            assert returned==['fixed-1','fixed-2'],returned
        print('PASS: malformed JSON, non-object arguments, missing fields, duplicate IDs and wrong call types reject the whole batch before effects')
        mode='alternating';remaining=20;sequence=0;start=len(requests)
        failure(run('agent',config,'--','recover'),'after two recovery attempts')
        assert len(requests)==start+3;no_bad_effects()
        mode='json';remaining=1;start=len(requests)
        config.write_text(base.replace('max_model_calls=8','max_model_calls=1'))
        failure(run('agent',config,'--','recover'),'model call budget exceeded')
        assert len(requests)==start+1;no_bad_effects();config.write_text(base)
        print('PASS: empty and malformed replies share a bounded repair counter and consume the host model budget')
        # Pause after the invalid reply; correction is reconstructed without new effects.
        remaining=1;journal=root/'repair.jsonl';start=len(requests)
        success(run('agent',config,'--record',journal,'--pause-after','1','--','recover'))
        assert len(requests)==start+1;no_bad_effects()
        success(run('agent-resume',config,journal));assert len(requests)==start+3
        (work/'one.txt').write_text('external one');(work/'two.txt').write_text('external two')
        start=len(requests);success(run('agent-replay',config,journal));assert len(requests)==start
        assert (work/'one.txt').read_text()=='external one' and (work/'two.txt').read_text()=='external two'
        # The completed first call in a corrected batch must not execute on resume.
        remaining=1;partial=root/'partial.jsonl'
        success(run('agent',config,'--record',partial,'--pause-after','3','--','recover'))
        (work/'one.txt').write_text('keep first')
        success(run('agent-resume',config,partial))
        assert (work/'one.txt').read_text()=='keep first' and (work/'two.txt').read_text()=='second valid'
        mode='late';remaining=1;late=root/'late.jsonl';start=len(requests)
        success(run('agent',config,'--record',late,'--pause-after','3','--','recover after write'))
        assert len(requests)==start+2 and (work/'one.txt').read_text()=='prior write'
        (work/'one.txt').write_text('prior external edit')
        success(run('agent-resume',config,late));assert len(requests)==start+3
        assert (work/'one.txt').read_text()=='prior external edit';no_bad_effects()
        returned=[m['tool_call_id'] for m in requests[-1]['messages'] if m['role']=='tool']
        assert returned==['prior'],returned
        mode='json'
        remaining=20;exhausted=root/'exhausted.jsonl';start=len(requests)
        success(run('agent',config,'--record',exhausted,'--pause-after','2','--','recover'))
        assert len(requests)==start+2
        failure(run('agent-resume',config,exhausted),'after two recovery attempts')
        assert len(requests)==start+3;no_bad_effects()
        print('PASS: correction counters survive resume; corrected batches preserve IDs, partial progress and offline replay without repeated writes')
finally:
    server.shutdown();server.server_close();thread.join()

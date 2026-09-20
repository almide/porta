#!/usr/bin/env python3
"""Reject malformed model batches before effects; repair, budget and journal checks."""
import http.server
import json
import pathlib
import subprocess
import sys
import tempfile
import threading
from types import SimpleNamespace

porta, agent, tool = [str(pathlib.Path(p).resolve()) for p in sys.argv[1:4]]
requests=[]
# What the fixture model does next. The test body sets these between cases.
state=SimpleNamespace(mode='json',remaining=1,sequence=0)

def call(identifier,path,content):
    return {'id':identifier,'type':'function','function':{'name':'write_file','arguments':json.dumps({'path':path,'content':content})}}

def corrupt_second_call(calls,mode):
    """Break the second call of the batch in the one way this mode names."""
    function=calls[1]['function']
    if mode in ('json','late'): function['arguments']='{broken'
    elif mode=='array': function['arguments']='[]'
    elif mode=='null': function['arguments']='null'
    elif mode=='missing_arguments': del function['arguments']
    elif mode=='object_arguments': function['arguments']={}
    elif mode=='duplicate': calls[1]['id']='first'
    elif mode=='missing_id': del calls[1]['id']
    elif mode=='missing_name': del function['name']
    elif mode=='wrong_type': calls[1]['type']='shell'

def alternating_batch(calls,sequence):
    """Alternate an empty reply with a broken batch, one per model call."""
    if sequence%2==0: return {'role':'assistant','content':None,'tool_calls':[]}
    calls[1]['function']['arguments']='{'
    return {'role':'assistant','content':None,'tool_calls':calls}

def bad_batch(mode,sequence):
    """A batch the broker must reject whole: neither call may execute."""
    if mode=='invalid_container':
        return {'role':'assistant','content':'Premature completion','tool_calls':{'not':'an array'}}
    calls=[call('first','must-not-exist.txt','bad batch'),call('second','other.txt','bad batch')]
    corrupt_second_call(calls,mode)
    if mode=='alternating': return alternating_batch(calls,sequence)
    message={'role':'assistant','content':None,'tool_calls':calls}
    if mode=='wrong_role': message['role']='user'
    if mode=='missing_role': del message['role']
    return message

class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])));requests.append(body)
        state.sequence+=1
        results=[m for m in body['messages'] if m['role']=='tool']
        data=json.dumps({'choices':[{'message':self.reply(results)}]}).encode()
        self.send_response(200);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)

    def reply(self,results):
        if state.mode=='late' and not results:
            return {'role':'assistant','tool_calls':[call('prior','one.txt','prior write')]}
        if state.remaining:
            state.remaining-=1
            return bad_batch(state.mode,state.sequence)
        if results: return {'role':'assistant','content':'complete'}
        return {'role':'assistant','tool_calls':[call('fixed-1','one.txt','first valid'),call('fixed-2','two.txt','second valid')]}
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
            state.mode=mode;state.remaining=1;start=len(requests)
            success(run('agent',config,'--','write two files'))
            no_bad_effects()
            assert (work/'one.txt').read_text()=='first valid' and (work/'two.txt').read_text()=='second valid'
            assert len(requests)==start+3
            # The invalid assistant batch must not contaminate the next request.
            assert not any(m['role']=='assistant' for m in requests[start+1]['messages'])
            returned=[m['tool_call_id'] for m in requests[-1]['messages'] if m['role']=='tool']
            assert returned==['fixed-1','fixed-2'],returned
        print('PASS: malformed JSON, non-object arguments, missing fields, duplicate IDs and wrong call types reject the whole batch before effects')
        state.mode='alternating';state.remaining=20;state.sequence=0;start=len(requests)
        failure(run('agent',config,'--','recover'),'after two recovery attempts')
        assert len(requests)==start+3;no_bad_effects()
        state.mode='json';state.remaining=1;start=len(requests)
        config.write_text(base.replace('max_model_calls=8','max_model_calls=1'))
        failure(run('agent',config,'--','recover'),'model call budget exceeded')
        assert len(requests)==start+1;no_bad_effects();config.write_text(base)
        print('PASS: empty and malformed replies share a bounded repair counter and consume the host model budget')
        # Pause after the invalid reply; correction is reconstructed without new effects.
        state.remaining=1;journal=root/'repair.jsonl';start=len(requests)
        success(run('agent',config,'--record',journal,'--pause-after','1','--','recover'))
        assert len(requests)==start+1;no_bad_effects()
        success(run('agent-resume',config,journal));assert len(requests)==start+3
        (work/'one.txt').write_text('external one');(work/'two.txt').write_text('external two')
        start=len(requests);success(run('agent-replay',config,journal));assert len(requests)==start
        assert (work/'one.txt').read_text()=='external one' and (work/'two.txt').read_text()=='external two'
        # The completed first call in a corrected batch must not execute on resume.
        state.remaining=1;partial=root/'partial.jsonl'
        success(run('agent',config,'--record',partial,'--pause-after','3','--','recover'))
        (work/'one.txt').write_text('keep first')
        success(run('agent-resume',config,partial))
        assert (work/'one.txt').read_text()=='keep first' and (work/'two.txt').read_text()=='second valid'
        state.mode='late';state.remaining=1;late=root/'late.jsonl';start=len(requests)
        success(run('agent',config,'--record',late,'--pause-after','3','--','recover after write'))
        assert len(requests)==start+2 and (work/'one.txt').read_text()=='prior write'
        (work/'one.txt').write_text('prior external edit')
        success(run('agent-resume',config,late));assert len(requests)==start+3
        assert (work/'one.txt').read_text()=='prior external edit';no_bad_effects()
        returned=[m['tool_call_id'] for m in requests[-1]['messages'] if m['role']=='tool']
        assert returned==['prior'],returned
        state.mode='json'
        state.remaining=20;exhausted=root/'exhausted.jsonl';start=len(requests)
        success(run('agent',config,'--record',exhausted,'--pause-after','2','--','recover'))
        assert len(requests)==start+2
        failure(run('agent-resume',config,exhausted),'after two recovery attempts')
        assert len(requests)==start+3;no_bad_effects()
        print('PASS: correction counters survive resume; corrected batches preserve IDs, partial progress and offline replay without repeated writes')
finally:
    server.shutdown();server.server_close();thread.join()

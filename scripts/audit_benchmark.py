#!/usr/bin/env python3
"""Independently verify archived fixed-response latency and RSS measurements."""
import argparse
import gzip
import hashlib
import json
import math
import re
import statistics

parser=argparse.ArgumentParser()
parser.add_argument('report')
parser.add_argument('--latency-runs',type=int,default=15)
parser.add_argument('--porta-record', action='store_true')
parser.add_argument('--memory-runs',type=int,default=7)
args=parser.parse_args()
with (gzip.open(args.report,'rt') if args.report.endswith('.gz') else open(args.report)) as source:r=json.load(source)
names=('porta_wasm','docker_agent_native')
parameters={'latency_runs':args.latency_runs,'memory_runs':args.memory_runs,'warmups_per_runtime':1}
if 'porta_record' in r['parameters'] or args.porta_record: parameters['porta_record']=args.porta_record
assert r['parameters']==parameters
expected={(name,'warmup','warmup') for name in names}
expected|={(name,'latency',i) for name in names for i in range(args.latency_runs)}
expected|={(name,'memory',f'memory-{i}') for name in names for i in range(args.memory_runs)}
keys=[(v['runtime'],v['phase'],v['index']) for v in r['runs']]
assert len(keys)==len(expected) and set(keys)==expected,'incomplete or duplicated matrix'
all_requests=[]
for v in r['runs']:
    assert v['exit_code']==0 and not v['timed_out'] and v['passed'] is True
    assert math.isfinite(v['elapsed_ms']) and v['elapsed_ms']>0
    assert len(v['requests'])==1,'model-call count mismatch'
    call=v['requests'][0];all_requests+=v['requests']
    assert call['path']=='/v1/chat/completions' and call['model']=='fixture'
    assert call['body']['model']=='fixture'
    assert call['body']['messages']==[{'role':'system','content':'Return benchmark-ok.'},{'role':'user','content':'Return benchmark-ok.'}]
    if v['runtime']=='porta_wasm':
        answer=v['stdout'].strip();assert not call['stream']
    else:
        events=[json.loads(line) for line in v['stdout'].splitlines() if line.startswith('{')]
        answer=''.join(e.get('content','') for e in events if e.get('type')=='agent_choice')
        assert any(e.get('type')=='stream_stopped' and e.get('reason')=='normal' for e in events)
        assert call['stream']
    assert answer==v['final_answer']=='benchmark-ok'
    if args.porta_record and v['runtime']=='porta_wasm':
        previous=''; kinds=[]
        for entry in v['journal']['records']:
            canonical=json.dumps(entry['payload'],ensure_ascii=False,sort_keys=True,separators=(',', ':'))
            expected_hash=hashlib.sha256((previous+'\n'+canonical).encode()).hexdigest()
            assert entry['previous']==previous and entry['hash']==expected_hash,'journal integrity mismatch'
            previous=entry['hash']; kinds.append(entry['payload']['kind'])
        assert kinds==['header','intent','result','complete']
        assert v['journal']['records'][-1]['payload']['output']=='benchmark-ok'
    if v['phase']=='memory':
        assert r['memory']['measured'] and r['memory']['unit']=='bytes' and 'macOS' in r['memory']['method']
        values=re.findall(r'^\s*(\d+)\s+maximum resident set size\s*$',v['resource_usage'],re.MULTILINE)
        assert len(values)==1 and int(values[0])==v['max_rss_bytes']>0,'RSS report mismatch'
for name in names:
    times=[v['elapsed_ms'] for v in r['runs'] if v['runtime']==name and v['phase']=='latency']
    rss=[v['max_rss_bytes'] for v in r['runs'] if v['runtime']==name and v['phase']=='memory']
    assert r['samples_ms'][name]==times and r['median_ms'][name]==statistics.median(times)
    assert r['memory']['samples_max_rss_bytes'][name]==rss
    assert r['memory']['median_max_rss_bytes'][name]==(statistics.median(rss) if rss else None)
assert r['requests']==all_requests,'unaccounted fixture requests'
print(json.dumps({'median_ms':r['median_ms'],'median_max_rss_bytes':r['memory']['median_max_rss_bytes']},indent=2))
print('PASS: complete matrix, exact answers, matched prompts, one model call, phase-separated latency and raw RSS evidence')

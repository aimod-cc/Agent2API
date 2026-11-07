import ast, collections, contextlib, datetime, importlib, json, pathlib, sqlite3, sys, tempfile, threading, types

root=pathlib.Path(sys.argv[1]).resolve() if len(sys.argv)>1 else pathlib.Path(__file__).resolve().parents[1];sys.path.insert(0,str(root))
p=importlib.import_module('maintenance_policy');d=importlib.import_module('dashboard')
tree=ast.parse((root/'server.py').read_text(encoding='utf-8-sig'))
nodes={n.name:n for n in tree.body if isinstance(n,ast.FunctionDef)}
ts=1794067199000  # 2026-11-07 23:59:59 Asia/Shanghai
zero={'id':'zero','provider':'loomy','balance':0,'lastSuccess':ts-7200000,'signedAt':ts-8000000,'signed':True,'enabled':True,'supported':True,'needsLogin':False,'cooldownUntil':0,'lastAttempt':ts-7200000,'credentialExpiresAt':0}
positive={**zero,'id':'positive','balance':100}
unknown={**zero,'id':'unknown','balance':None,'lastSuccess':0,'lastAttempt':1}
new_sign={**zero,'id':'new_sign','signedAt':ts-1000}
cfg={'paused':False,'globalConcurrency':1,'globalBalanceConcurrency':1,'providers':{'loomy':{'concurrency':1,'balance':{'enabled':True,'concurrency':1,'interval':3600,'positiveInterval':3600,'route':'auto'},'checkin':{'enabled':False},'credential':{'enabled':False}}}}
old=not hasattr(p,'balance_waits_for_checkin')
if old:
    jobs=[{'id':'z','account':'zero','action':'balance','origin':'auto','dispatch_rank':1,'created':1},{'id':'p','account':'positive','action':'balance','origin':'auto','dispatch_rank':2,'created':2}]
    order=[j['account'] for j in sorted(jobs,key=lambda j:d.queue_order(j,{'zero':zero,'positive':positive}))]
    assert order==['zero','positive']
    print(json.dumps({'positive_first':False,'confirmed_zero_periodic':True,'midnight_gate':False,'order':order,'real_requests':0}));sys.exit()

assert p.balance_waits_for_checkin(zero)
assert p.balance_waits_for_checkin({**zero,'signed':False,'signedAt':0}) # midnight alone cannot reopen
assert not p.balance_waits_for_checkin(new_sign)
assert not p.balance_waits_for_checkin(positive)
assert not p.balance_waits_for_checkin(unknown)
assert not p.balance_waits_for_checkin({**zero,'unlimited':True})
assert not p.balance_waits_for_checkin({**zero,'balance':-1})
assert not p.balance_waits_for_checkin({**zero,'lastSuccess':0})
assert not p.balance_satisfied({'origin':'auto'},new_sign,ts,cfg['providers']['loomy']['balance'])
assert p.balance_waits_for_checkin({**new_sign,'lastSuccess':ts}) # follow-up zero goes back to sleep
assert not p.balance_waits_for_checkin({**new_sign,'lastSuccess':ts,'balance':100})
assert not p.balance_satisfied({'origin':'post_checkin','required_after':ts},zero,ts,cfg['providers']['loomy']['balance'])

with tempfile.TemporaryDirectory() as tmp:
    @contextlib.contextmanager
    def db():
        c=sqlite3.connect(pathlib.Path(tmp)/'fixture.db');c.row_factory=sqlite3.Row
        try:yield c;c.commit()
        finally:c.close()
    with db() as c:c.executescript('create table jobs(id text,account text,provider text,action text,status text,created int,started int,ended int,message text,attempt int default 0,origin text default "auto",priority int default 0,required_after int default 0); create table cooldowns(scope text,until int);')
    rows=[zero,positive,unknown];added=[]
    ns={'settings':lambda:cfg,'get_snapshot':lambda:{'accounts':rows},'now':lambda:ts,'db':db,'LOCK':threading.RLock(),'collections':collections,'datetime':datetime,'TZ':datetime.timezone(datetime.timedelta(hours=8)),'credential_due':p.credential_due,'balance_interval':p.balance_interval,'balance_waits_for_checkin':p.balance_waits_for_checkin,'enqueue':lambda ids,a,**kw:added.extend(ids)}
    exec(compile(ast.Module(body=[nodes['auto_enqueue']],type_ignores=[]),'actual-auto','exec'),ns)
    ns['auto_enqueue']();assert added==['positive','unknown'],added
    rows=[{**zero,'signed':False,'signedAt':0}];added.clear();ns['now']=lambda:ts+2000;ns['auto_enqueue']();assert not added
    rows=[new_sign];ns['auto_enqueue']();assert added==['new_sign']

    # A positive account beyond the old 256-row SQL window must still be dispatched first.
    rows=[zero,positive,unknown]
    with db() as c:
        for i in range(300):c.execute('insert into jobs(id,account,provider,action,status,created) values(?,?,"loomy","balance","queued",?)',(str(i),'zero',i))
        c.execute('insert into jobs(id,account,provider,action,status,created) values("positive","positive","loomy","balance","queued",1000)')
    submitted=[];ns.update(now=lambda:ts,ACTIVE={},queue_order=d.queue_order,balance_satisfied=p.balance_satisfied,HEALTH=types.SimpleNamespace(available_routes=lambda:{'auto'}),execute=lambda j:None)
    exec(compile(ast.Module(body=[nodes['dispatch_jobs']],type_ignores=[]),'actual-dispatch','exec'),ns)
    ns['dispatch_jobs'](cfg,types.SimpleNamespace(submit=lambda f,j:submitted.append(j)))
    assert len(submitted)==1 and submitted[0]['account']=='positive',submitted
    with db() as c:c.execute('delete from jobs')
    ns['ACTIVE'].clear();submitted.clear();rows=[zero]
    with db() as c:c.execute('insert into jobs(id,account,provider,action,status,created) values("zero","zero","loomy","balance","queued",1)')
    ns['dispatch_jobs'](cfg,types.SimpleNamespace(submit=lambda f,j:submitted.append(j)))
    assert not submitted
    with db() as c:assert c.execute('select status from jobs').fetchone()[0]=='skipped'
    # Explicit manual requests and post-checkin confirmation are not blocked by the zero gate.
    for origin in ['manual','post_checkin']:
        with db() as c:
            c.execute('delete from jobs');c.execute('insert into jobs(id,account,provider,action,status,created,origin,required_after) values("explicit","zero","loomy","balance","queued",1,?,?)',(origin,ts if origin=='post_checkin' else 0))
        ns['ACTIVE'].clear();submitted.clear();ns['dispatch_jobs'](cfg,types.SimpleNamespace(submit=lambda f,j:submitted.append(j)));assert len(submitted)==1

print(json.dumps({'positive_first':True,'confirmed_zero_periodic':False,'midnight_gate':True,'new_checkin_reopens':True,'zero_again_suspends':True,'unknown_and_manual_preserved':True,'positive_beyond_old_window':True,'stale_zero_queue_skipped':True,'real_requests':0}))

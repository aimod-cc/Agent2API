import pathlib,os,json,sqlite3,time,sys,threading,urllib.request,http.cookiejar
for p in ['/state','/gateway','/key','/secrets']:pathlib.Path(p).mkdir(exist_ok=True)
pathlib.Path('/key/account.key').write_bytes(os.urandom(32));pathlib.Path('/secrets/auth.json').write_text(json.dumps({'username':'fixture','password':'fixture'}));pathlib.Path('/secrets/bridge.key').write_text('fixture')
with sqlite3.connect('/gateway/agent2api.db') as c:
 c.executescript('create table accounts(id text,provider text,enabled int,data text);create table account_usage_records(account_id text,remaining real,unlimited int,last_success_at int,last_attempt_at int,error text);create table checkin_records(account_id text,date text,ok int,ts int)')
 for i,p in enumerate(['loomy','raccoon','codearts']):
  c.execute('insert into accounts values(?,?,1,?)',(str(i),p,json.dumps({'name':'fixture','pluginSecret':{},'usageQuery':{'enabled':False}})));c.execute('insert into account_usage_records values(?,100,0,?,?,null)',(str(i),int(time.time()*1000),int(time.time()*1000)))
sys.path.insert(0,'/app');import server
state=server.build_ui_state();assert set(state['dashboard']['platforms'])=={'loomy','raccoon'};server.UI_STATE['data']=state
srv=server.ThreadingHTTPServer(('127.0.0.1',0),server.Handler);t=threading.Thread(target=srv.serve_forever,daemon=True);t.start();base='http://127.0.0.1:'+str(srv.server_port)
op=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
for path,mime in [('/','text/html'),('/ui.js','javascript'),('/ux.js','javascript'),('/ui.css','text/css'),('/vendor/tom-select.min.js','javascript'),('/vendor/tom-select.css','text/css')]:
 with op.open(base+path) as r:assert mime in r.headers['Content-Type'];assert len(r.read())>100
op.open(urllib.request.Request(base+'/api/login',data=b'{"username":"fixture","password":"fixture"}',headers={'Content-Type':'application/json'})).read()
with op.open(base+'/api/state?compact=1') as r:s=json.load(r);assert 'accounts' not in s and s['summary']['total']==2 and not s['proxyHealth']['config']['enabled']
with op.open(base+'/api/accounts?size=1') as r:s=json.load(r);assert len(s['items'])==1
srv.shutdown();server.HEALTH.pool.shutdown();print(json.dumps({'full_module_boot':True,'static_routes':6,'compact_state':True,'accounts_pagination':True,'network':'none','real_accounts':0}))

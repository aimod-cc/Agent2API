#!/usr/bin/env python3
import os,sys,json,time,tempfile,subprocess,urllib.request,urllib.error,http.cookiejar,socket,io,zipfile,importlib.util
from pathlib import Path
# Run in a loopback-only network namespace. All accounts and credentials are synthetic.
if len(sys.argv)!=5:
 raise SystemExit("Usage: test-maintenance-runtime.py HOST_BINARY PLUGIN_BINARY PLUGIN_FIXTURE_SCRIPT HOST_UI_DIR")
HOST_BINARY,PLUGIN_BINARY,FIXTURE_SCRIPT,HOST_UI_DIR=map(lambda p:Path(p).resolve(),sys.argv[1:])
spec=importlib.util.spec_from_file_location('fixture',FIXTURE_SCRIPT);fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
results=[]
def check(name,condition,details=None):
 assert condition,(name,details)
 row={'test':name,'passed':True,'details':details};results.append(row);print(json.dumps(row),flush=True)
with tempfile.TemporaryDirectory(prefix='maintenance-host-fixture-') as directory:
 data=Path(directory)
 config={'pluginUpdates':{'checkMode':'custom','channelMode':'custom','autoCheck':False,'autoUpdate':False,'autoInstallOfficial':False,'stable':True,'beta':False},'autoUpdate':False}
 (data/'config.json').write_text(json.dumps(config))
 with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
 env=os.environ.copy();env.update(AGENT2API_HOST='127.0.0.1',AGENT2API_PROXY_HOME=str(data),AGENT2API_PROXY_PORT=str(port),AGENT2API_ADMIN_USER='fixture-admin',AGENT2API_ADMIN_PASSWORD='fixture-password-not-a-secret',AGENT2API_CAPTCHA_ENABLED='0',AGENT2API_UI_DIR=str(HOST_UI_DIR),RACCOON_MAIN_SITE_URL=f'http://127.0.0.1:{fixture.UPSTREAM.server_port}',LOOMY_POINTS_BASE_URL=f'http://127.0.0.1:{fixture.UPSTREAM.server_port}')
 base=f'http://127.0.0.1:{port}'
 log=(data/'host-stderr.log').open('w')
 def start():
  process=subprocess.Popen([str(HOST_BINARY)],env=env,stdout=log,stderr=log)
  for _ in range(150):
   if process.poll() is not None:raise RuntimeError('fixture host exited '+str(process.returncode))
   try:
    with urllib.request.urlopen(base+'/health',timeout=1) as r:
     if r.status==200:return process
   except Exception:time.sleep(.2)
  process.terminate();process.wait(timeout=10);raise RuntimeError('fixture host not ready')
 def stop(process):
  process.terminate()
  try:process.wait(timeout=15)
  except subprocess.TimeoutExpired:process.kill();process.wait()
 p=start()
 try:
  op=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
  def request(path,body=None,method=None,raw=False):
   payload=body if raw else (json.dumps(body).encode() if body is not None else None)
   req=urllib.request.Request(base+path,data=payload,method=method or ('POST' if payload is not None else 'GET'),headers={'Content-Type':'application/zip' if raw else 'application/json'})
   try:
    with op.open(req,timeout=40) as r:return r.status,json.load(r)
   except urllib.error.HTTPError as e:return e.code,json.loads(e.read())
  def data_of(v):return v.get('data',v)
  status,_=request('/api/maintenance-policy');check('authentication_required',status==401,status)
  status,_=request('/api/panel/login',{'username':'fixture-admin','password':'fixture-password-not-a-secret'});check('fixture_login',status==200,status)
  status,response=request('/api/maintenance-policy');settings=data_of(response)['settings'];check('default_off',status==200 and settings['enabled'] is False)
  invalid={**settings,'maxConcurrent':0};status,_=request('/api/maintenance-policy',invalid,'PUT');check('invalid_policy_rejected',status==400,status)
  _,response=request('/api/maintenance-policy');check('invalid_policy_not_persisted',data_of(response)['settings']==settings)
  settings.update(enabled=True,mode='direct',minIntervalMs=10,maxIntervalMs=20,usageCacheSeconds=0)
  status,response=request('/api/maintenance-policy',settings,'PUT');check('save_direct_policy',status==200,response if status!=200 else None)
  # Minimal unsigned test package, installed only into this temporary namespace/data dir.
  manifest={'id':'maintenance-fixture','name':'Maintenance fixture','version':'0.0.1','protocol':1,'providers':[{'id':x,'label':x,'default_enabled':True} for x in ['raccoon','loomy']],'entry':{'x86_64-unknown-linux-gnu':'bin/fixture'}}
  buf=io.BytesIO()
  with zipfile.ZipFile(buf,'w',zipfile.ZIP_DEFLATED) as z:
   z.writestr('pack.json',json.dumps(manifest));z.write(PLUGIN_BINARY,'bin/fixture')
  status,response=request('/api/plugins/install',buf.getvalue(),raw=True);check('install_fixture_plugin',status==200,response if status!=200 else None)
  for _ in range(50):
   _,response=request('/api/plugins');packs=data_of(response).get('packs',[])
   if any(x.get('id')=='maintenance-fixture' and x.get('enabled') for x in packs):break
   time.sleep(.1)
  accounts=[{'id':'fixture-raccoon','provider':'raccoon','accessToken':'fixture-token','userId':'fixture-user-raccoon','proxy':None},{'id':'fixture-loomy','provider':'loomy','session':'fixture-session','accessToken':'fixture-session','userId':'fixture-user-loomy','proxy':None}]
  status,response=request('/api/accounts/import',{'accounts':accounts});check('import_synthetic_accounts',status==200 and data_of(response).get('added')==2,response)
  for mode in ['direct','fixed','auto']:
   if mode=='fixed':
    status,response=request('/api/proxies/pool',{'name':'fixture-loopback','protocol':'http','host':'127.0.0.1','port':fixture.PROXY.server_port,'username':'fixture','password':'only-test'})
    check('create_fixture_pool_entry',status==200)
    pool=data_of(response);items=pool.get('items',[]);settings['proxyId']=items[0]['id']
   settings['mode']=mode
   status,_=request('/api/maintenance-policy',settings,'PUT');check('save_mode_'+mode,status==200)
   for account in accounts:
    fixture.EVENTS.clear();status,response=request('/api/accounts/usage?id='+account['id']);events=list(fixture.EVENTS)
    check('usage_'+mode+'_'+account['provider'],status==200 and bool(events) and all(x['route']==('direct' if mode=='direct' else 'http') for x in events),{'http':status,'routes':sorted(set(x['route'] for x in events)),'response':response if not events else None})
  for account in accounts:
   fixture.EVENTS.clear();status,response=request('/api/accounts/checkin',{'id':account['id']})
   check('checkin_auto_'+account['provider'],status==200 and bool(fixture.EVENTS) and all(x['route']=='http' for x in fixture.EVENTS),{'http':status,'routes':sorted(set(x['route'] for x in fixture.EVENTS)),'response':response if not fixture.EVENTS else None})
   fixture.EVENTS.clear();status,response=request('/api/accounts/'+account['id']+'/onboarding/claim',{})
   check('reward_auto_'+account['provider'],status==200 and bool(fixture.EVENTS) and all(x['route']=='http' for x in fixture.EVENTS),{'http':status,'routes':sorted(set(x['route'] for x in fixture.EVENTS)),'response':response if not fixture.EVENTS else None})
  _,response=request('/api/accounts/export');exported=data_of(response).get('accounts',[])
  check('account_proxy_fields_unchanged',len(exported)==2 and all(x.get('proxy') is None for x in exported))
  # A newly imported account inherits the existing global auto policy without a binding write.
  newer={**accounts[0],'id':'fixture-new','userId':'fixture-user-new','accessToken':'fixture-token-new'}
  status,response=request('/api/accounts/import',{'accounts':[newer]});check('import_new_account',status==200 and data_of(response).get('added')==1)
  fixture.EVENTS.clear();status,response=request('/api/accounts/usage?id=fixture-new');check('new_account_inherits',status==200 and bool(fixture.EVENTS) and all(x['route']=='http' for x in fixture.EVENTS))
  stop(p);p=start()
  op=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
  request('/api/panel/login',{'username':'fixture-admin','password':'fixture-password-not-a-secret'})
  status,response=request('/api/maintenance-policy');check('policy_persists_restart',status==200 and data_of(response)['settings']==settings)
  settings['enabled']=False;status,_=request('/api/maintenance-policy',settings,'PUT');check('disable_policy',status==200)
  fixture.EVENTS.clear();status,response=request('/api/accounts/usage?id=fixture-raccoon');check('disabled_legacy_direct',status==200 and bool(fixture.EVENTS) and all(x['route']=='direct' for x in fixture.EVENTS))
 finally:
  stop(p);log.close()
  for s in [fixture.UPSTREAM,fixture.PROXY,fixture.SOCKS]:s.shutdown();s.server_close()
print(json.dumps({'passed':len(results),'production_modified':False,'real_upstream':False}))

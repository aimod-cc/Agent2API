from proxy_health import AutoProxyHealth, ProxyUnavailable
import gzip
from ui_support import scope_snapshot, account_page, ProxyHealth
"""Independent Agent2API maintenance controller. Official provider binary is never patched."""
import sys
import base64, collections, concurrent.futures, contextlib, datetime, hashlib, hmac, http.cookies, http.cookiejar, json, math, os, pathlib, queue, random, re, secrets, signal, sqlite3, subprocess, threading, time, urllib.request, urllib.error, urllib.parse
from dashboard import enrich, queue_order, queue_view, history_view
from maintenance_policy import credential_meta, credential_due, balance_interval, balance_satisfied, balance_waits_for_checkin
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

ROOT=pathlib.Path(os.environ.get('MAINTENANCE_STATE_DIR','/state')); ROOT.mkdir(exist_ok=True)
MAIN=pathlib.Path(os.environ.get('MAINTENANCE_DATABASE','/gateway/agent2api.db')); KEY=pathlib.Path(os.environ.get('MAINTENANCE_ACCOUNT_KEY','/key/account.key')).read_bytes(); AES=AESGCM(KEY)
AUTH=json.loads(pathlib.Path(os.environ.get('MAINTENANCE_AUTH_FILE','/secrets/auth.json')).read_text()); PLUGIN=os.environ.get('MAINTENANCE_PLUGIN','/official/agent2api-providers')
GATEWAY=os.environ.get('MAINTENANCE_GATEWAY_URL','http://gateway:3065').rstrip('/')
PLUGIN_HOST_VERSION=os.environ.get('MAINTENANCE_HOST_VERSION','3.4.2')
PLUGIN_TARGET=os.environ.get('MAINTENANCE_PLUGIN_TARGET','x86_64-unknown-linux-gnu')
TZ=datetime.timezone(datetime.timedelta(hours=8)); STOP=threading.Event(); LOCK=threading.RLock(); SNAP={}; ACTIVE={}; SESSIONS={}; LOGIN_ATTEMPTS={}
SUPPORTED={'loomy','raccoon'}
BRIDGE_BUSY={}
ROUTING={'at':0,'entries':{},'error':'官方调度快照尚未加载','scope':'未指定模型的官方调度队列'}
ROUTING_LOCK=threading.RLock()
BRIDGE_KEY=pathlib.Path(os.environ.get('MAINTENANCE_BRIDGE_KEY_FILE','/secrets/bridge.key')).read_text().strip()
def now(): return int(time.time()*1000)
def today(): return datetime.datetime.now(TZ).strftime('%Y-%m-%d')
def dumps(v): return json.dumps(v,ensure_ascii=False,separators=(',',':'))
@contextlib.contextmanager
def db(main=False,ro=False):
 p=MAIN if main else ROOT/'maintenance.db'; c=sqlite3.connect('file:'+str(p)+('?mode=ro' if ro else '?mode=rw') if main else str(p),uri=main,timeout=8);c.row_factory=sqlite3.Row;c.execute('pragma busy_timeout=8000')
 if ro:c.execute('pragma query_only=on')
 try:yield c;c.commit()
 except: c.rollback();raise
 finally:c.close()
def decode(v):
 if not v.startswith('enc1:'):return json.loads(v)
 b=base64.b64decode(v[5:]);return json.loads(AES.decrypt(b[:12],b[12:],None))
def encode(v):
 n=os.urandom(12);return 'enc1:'+base64.b64encode(n+AES.encrypt(n,dumps(v).encode(),None)).decode()
def account(aid):
 with db(True,True) as c:r=c.execute('select * from accounts where id=?',(aid,)).fetchone()
 if not r:raise RuntimeError('账户已不存在')
 return dict(r),decode(r['data'])
def secret_of(d):
 if isinstance(d.get('pluginSecret'),dict):return d['pluginSecret']
 return {k:v for k,v in d.items() if k not in {'name','provider','priority','enabled','addedAt','updatedAt','usageQuery','limiters','limiterGroups','proxy','maxConcurrent','checkinAt','lowBalance','limiterMode'}}
def scrub(s):
 s=str(s);s=re.sub(r'https?://\S+','[上游地址]',s);s=re.sub(r'(?i)(bearer\s+|token[=: ]+|auth[=: ]+|session[=: ]+)[^\s,;]+',r'\1[隐藏]',s);s=re.sub(r'[A-Za-z0-9_+/=.-]{40,}','[隐藏]',s)
 return s[:220]
with db() as c:
 c.executescript('''PRAGMA journal_mode=WAL;
 CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
 CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY,account TEXT,provider TEXT,action TEXT,status TEXT,created INTEGER,started INTEGER,ended INTEGER,message TEXT,proxy TEXT,attempt INTEGER DEFAULT 0);
 CREATE INDEX IF NOT EXISTS jobs_lookup ON jobs(account,action,created);
 CREATE TABLE IF NOT EXISTS cooldowns(scope TEXT PRIMARY KEY,until INTEGER,failures INTEGER,reason TEXT);
 CREATE TABLE IF NOT EXISTS proxies(id TEXT PRIMARY KEY,label TEXT,secret TEXT,enabled INTEGER);
 CREATE TABLE IF NOT EXISTS ledger(key TEXT PRIMARY KEY,value TEXT);
 CREATE TABLE IF NOT EXISTS events(ts INTEGER,kind TEXT,message TEXT);
 CREATE TABLE IF NOT EXISTS bridge_audit(ts INTEGER,account TEXT,provider TEXT,proxy TEXT,status TEXT,duration_ms INTEGER);
 ''')
 columns={row[1] for row in c.execute('pragma table_info(jobs)')}
 for name,decl in [('origin',"TEXT NOT NULL DEFAULT 'manual'"),('priority','INTEGER NOT NULL DEFAULT 0'),('required_after','INTEGER NOT NULL DEFAULT 0')]:
  if name not in columns:c.execute('alter table jobs add column '+name+' '+decl)
 c.execute("update jobs set status='interrupted',ended=?,message='服务重启；未自动重试，请人工核对签到结果' where status='running'",(now(),))
with db() as c:
 row=c.execute("select value from settings where key='config'").fetchone()
 if row:
  val=json.loads(row[0]);val['paused']=True;c.execute("update settings set value=? where key='config'",(dumps(val),))
DEFAULT={'paused':True,'globalConcurrency':2,'timeout':45,'providers':{p:{'concurrency':1,'jitterMin':1,'jitterMax':3,'balance':{'concurrency':1,'enabled':False,'interval':3600,'positiveInterval':21600,'route':'direct'},'checkin':{'concurrency':1,'enabled':False,'interval':86400,'route':'direct'},'credential':{'concurrency':1,'enabled':False,'interval':3600,'route':'direct'}} for p in sorted(SUPPORTED)}}
def settings():
 with db() as c:r=c.execute("select value from settings where key='config'").fetchone()
 return json.loads(r[0]) if r else json.loads(dumps(DEFAULT))
def save_settings(v):
 old=settings();out=json.loads(dumps(old))
 if 'paused' in v:out['paused']=bool(v['paused'])
 for k,lo,hi in [('globalConcurrency',1,32),('globalBalanceConcurrency',1,16),('timeout',10,120),('queueBufferMultiplier',1,5)]:
  if k in v:out[k]=max(lo,min(hi,int(v[k])))
 for p,d in v.get('providers',{}).items():
  if p not in SUPPORTED:continue
  t=out['providers'][p]
  for k,lo,hi in [('concurrency',1,20),('jitterMin',0,60),('jitterMax',0,120)]:
   if k in d:t[k]=max(lo,min(hi,int(d[k])))
  t['jitterMax']=max(t['jitterMax'],t['jitterMin'])
  for a in ['balance','checkin','credential']:
   if a not in d:continue
   x=d[a];r=t[a]
   if 'enabled' in x:r['enabled']=bool(x['enabled'])
   if 'paused' in x:r['paused']=bool(x['paused'])
   if 'concurrency' in x:r['concurrency']=max(1,min({'balance':16,'checkin':4,'credential':2}[a],int(x['concurrency'])))
   if 'interval' in x:r['interval']=max(300,min(604800,int(x['interval'])))
   if 'positiveInterval' in x:r['positiveInterval']=max(300,min(604800,int(x['positiveInterval'])))
   if 'route' in x:
    route=str(x['route'])[:80]
    if route not in ('auto','direct') and route!=r.get('route'):
     with db() as c:found=c.execute('select 1 from proxies where id=? and enabled=1',(route,)).fetchone()
     if not found:raise ValueError('固定节点不存在或已停用')
    r['route']=route
   if a=='checkin':
    for k,default in [('start','00:10'),('end','23:00')]:
     if k in x:
      value=str(x[k])
      if not re.fullmatch(r'(?:[01]\d|2[0-3]):[0-5]\d',value):raise ValueError('签到时间格式应为 HH:MM')
      r[k]=value
    if r.get('start','00:10')>=r.get('end','23:00'):raise ValueError('签到结束时间必须晚于开始时间')
    if 'maxAttempts' in x:r['maxAttempts']=max(1,min(6,int(x['maxAttempts'])))
    if 'retryInterval' in x:r['retryInterval']=max(300,min(21600,int(x['retryInterval'])))
 with db() as c:c.execute('insert or replace into settings values(?,?)',('config',dumps(out)))
 return out
def event(k,msg):
 with db() as c:c.execute('insert into events values(?,?,?)',(now(),k,scrub(msg)))
class Official:
 def __init__(self):self.lock=threading.Lock();self.op=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()));self.logged=False
 def raw(self,path,body=None):
  req=urllib.request.Request(GATEWAY+path,data=dumps(body).encode() if body is not None else None,headers={'Content-Type':'application/json'})
  with self.op.open(req,timeout=75) as r:v=json.load(r)
  if isinstance(v,dict) and v.get('success') is False:raise RuntimeError('官方管理接口拒绝请求')
  return v.get('data',v) if isinstance(v,dict) else v
 def call(self,path,body=None):
  with self.lock:
   if not self.logged:self.raw('/api/panel/login',{'username':AUTH['username'],'password':AUTH['password']});self.logged=True
   try:return self.raw(path,body)
   except urllib.error.HTTPError as e:
    if e.code not in (401,403):raise
    self.logged=False;raise RuntimeError('官方管理登录失效，需要重试登录')
OFFICIAL=Official()
OFFICIAL_THREADS=threading.local()
def official_client():
 if not hasattr(OFFICIAL_THREADS,'client'):OFFICIAL_THREADS.client=Official()
 return OFFICIAL_THREADS.client

SNAP_BUILD_LOCK=threading.Lock()
def get_snapshot(force=False):
 with SNAP_BUILD_LOCK:return build_snapshot(force)

def build_snapshot(force=False):
 with LOCK:
  if not force and SNAP.get('expires',0)>time.time():return SNAP['data']
 with db(True,True) as c:
  raw=c.execute('select a.id,a.provider,a.enabled,a.data,u.remaining,u.unlimited,u.last_success_at,u.last_attempt_at,u.error from accounts a left join account_usage_records u on a.id=u.account_id').fetchall()
  signed={r[0]:r[1] for r in c.execute('select account_id,max(ts) from checkin_records where date=? and ok=1 group by account_id',(today(),))}
 with db() as c:
  cool={r['scope']:dict(r) for r in c.execute('select * from cooldowns where until>?',(now(),))}
  latest={r['account']:dict(r) for r in c.execute('select j.* from jobs j join (select account,max(created) as m from jobs group by account) x on j.account=x.account and j.created=x.m')}
 rows=[];needs=[]
 for r in raw:
  try:d=decode(r['data'])
  except:continue
  u=d.get('usageQuery');
  if r['provider'] in SUPPORTED and (not isinstance(u,dict) or u.get('enabled') is not False):needs.append(r['id'])
  j=latest.get(r['id'],{});cd=max(cool.get('account:'+r['id'],{}).get('until',0),cool.get('provider:'+r['provider'],{}).get('until',0));required=bool(d.get('needsRelogin')) or j.get('status')=='needs_login'
  rows.append({'id':r['id'],'name':str(d.get('name') or r['id'])[:100],'provider':r['provider'],'enabled':bool(r['enabled']),'balance':r['remaining'],'unlimited':bool(r['unlimited']),'lastSuccess':r['last_success_at'] or 0,'lastAttempt':r['last_attempt_at'] or 0,'error':scrub(r['error'] or ''),'signed':r['id'] in signed,'signedAt':signed.get(r['id'],0),'cooldownUntil':cd,'needsLogin':required,'supported':r['provider'] in SUPPORTED,'jobStatus':j.get('status',''),'jobMessage':j.get('message',''),'autoBalanceEnabled':r['id'] in needs,**credential_meta(r['provider'],secret_of(d))})
 result={'at':now(),'accounts':rows,'summary':{'total':len(rows),'positive':sum((r['balance'] or 0)>0 for r in rows),'knownBalance':sum(r['lastSuccess']>0 for r in rows),'signedToday':sum(r['signed'] for r in rows),'cooling':sum(r['cooldownUntil']>now() for r in rows),'needsLogin':sum(r['needsLogin'] for r in rows)},'autoBalanceIds':needs}
 with LOCK:SNAP.update(data=result,expires=time.time()+6)
 return result

def select_proxy(route):
 if route=='direct':return None
 rows=HEALTH.candidates(route)
 if not rows:raise ProxyUnavailable('MAINTENANCE_PROXY_WAIT: 无可用维护节点，保留排队；不会回退直连')
 return random.choice(rows)

class WriteConflict(Exception):pass
class RpcError(Exception):
 def __init__(self,obj):self.obj=obj;super().__init__(dumps(obj))
class Worker:
 def __init__(self,aid,proxy,timeout):
  self.aid=aid;self.row,self.initial=account(aid);self.timeout=timeout;self.q=queue.Queue();self.n=0;self.changed=0;self.temp=ROOT/'workers'/secrets.token_hex(10);self.temp.mkdir(parents=True,mode=0o700)
  env=os.environ.copy()
  for k in list(env):
   if 'proxy' in k.lower():env.pop(k,None)
  if proxy:
   val=decode(proxy['secret']);u=urllib.parse.urlsplit(val['url']);host=u.hostname
   if not re.fullmatch(r'[0-9.]+',host or ''):raise RuntimeError('第一版代理地址请填写 IPv4，避免解析歧义')
   user=urllib.parse.unquote(u.username or '');pw=urllib.parse.unquote(u.password or '')
   if any(ch.isspace() for ch in user+pw) or '\n' in user+pw:raise RuntimeError('代理凭证不能含空白字符')
   typ={'http':'http','socks5':'socks5','socks4':'socks4'}.get(u.scheme)
   if not typ or not u.port:raise RuntimeError('仅支持带端口的 HTTP/SOCKS5/SOCKS4 代理')
   config='strict_chain\nquiet_mode\nproxy_dns\ntcp_read_time_out 15000\ntcp_connect_time_out 8000\n[ProxyList]\n'+f'{typ} {host} {u.port}'+(f' {user} {pw}' if user else '')+'\n'
   cf=self.temp/'proxychains.conf';cf.write_text(config);cf.chmod(0o600);env['LD_PRELOAD']='/usr/lib/x86_64-linux-gnu/libproxychains.so.4';env['PROXYCHAINS_CONF_FILE']=str(cf)
  self.proc=subprocess.Popen([PLUGIN],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True,bufsize=1,env=env,cwd=self.temp,start_new_session=True)
  def read():
   try:
    for line in self.proc.stdout:
     try:self.q.put(json.loads(line))
     except:pass
   finally:self.q.put({'_eof':True})
  threading.Thread(target=read,daemon=True).start()
  try:self.rpc('init',{'env':{'data_dir':str(self.temp),'host_version':PLUGIN_HOST_VERSION,'target':PLUGIN_TARGET,'proxy':'','debug_traffic':False}},min(8,timeout))
  except:self.close();raise
 def callback(self,m,p):
  aid=p.get('account',self.aid)
  if m.startswith('store.') and aid and aid!=self.aid:raise RuntimeError('插件越过当前账户范围')
  if m=='store.get':
   r,d=account(self.aid);return {'id':self.aid,'provider':r['provider'],'secret':secret_of(d),'public':d.get('pluginPublic',{})}
  if m=='store.list':return [self.callback('store.get',{'account':self.aid})]
  if m=='store.update':
   with db(True) as c:
    c.execute('begin immediate');r=c.execute('select data from accounts where id=?',(aid,)).fetchone()
    if not r:return {'updated':False}
    d=decode(r[0]);cur=secret_of(d)
    if cur!=p.get('expect'):return {'updated':False}
    if 'pluginSecret' in d:d['pluginSecret']=p['secret']
    else:
     for k in cur:d.pop(k,None)
     d.update(p['secret'])
    if isinstance(p.get('public'),dict):d['pluginPublic']=p['public']
    d['updatedAt']=now();n=c.execute('update accounts set data=? where id=? and data=?',(encode(d),aid,r[0])).rowcount;self.changed+=n;return {'updated':bool(n)}
  if m in ['store.ledger_get','store.ledger_put','settings.get','settings.set']:
   k=dumps([self.row['provider'],aid,p.get('kind'),p.get('key')])
   with db() as c:
    if m.endswith(('get','.get')):
     r=c.execute('select value from ledger where key=?',(k,)).fetchone();return decode(r[0]).get('value') if r else None
    c.execute('insert or replace into ledger values(?,?)',(k,encode({'value':p.get('value')})));return None
  if m in ['log','notify']:return None
  raise RuntimeError('第一版未支持此官方回调：'+m)
 def rpc(self,m,p,timeout=None):
  self.n+=1;n=self.n;self.proc.stdin.write(dumps({'id':n,'method':m,'params':p})+'\n');self.proc.stdin.flush();end=time.monotonic()+(timeout or self.timeout)
  while time.monotonic()<end:
   try:f=self.q.get(timeout=max(.01,end-time.monotonic()))
   except queue.Empty:raise TimeoutError('官方插件调用超时')
   if f.get('_eof'):raise RuntimeError('官方插件进程提前退出')
   if f.get('method'):
    try:r={'id':f.get('id'),'result':self.callback(f['method'],f.get('params') or {})}
    except Exception as e:r={'id':f.get('id'),'error':{'code':-32603,'message':scrub(e)}}
    self.proc.stdin.write(dumps(r)+'\n');self.proc.stdin.flush()
   elif f.get('id')==n:
    if 'error' in f:raise RpcError(f['error'])
    return f.get('result')
  raise TimeoutError('官方插件调用超时')
 def close(self):
  if hasattr(self,'proc'):
   if self.proc.poll() is None:
    try:os.killpg(self.proc.pid,signal.SIGTERM);self.proc.wait(timeout=2)
    except:os.killpg(self.proc.pid,signal.SIGKILL);self.proc.wait()
   for s in [self.proc.stdin,self.proc.stdout]:
    if s:s.close()
  import shutil
  shutil.rmtree(self.temp,ignore_errors=True)

def write_balance(aid,u,attempt_at):
 if not isinstance(u,dict):raise RuntimeError('官方余额响应格式不正确')
 n=u.get('totalLeft',u.get('available'));un=bool(u.get('unlimited'));n=None if un or not isinstance(n,(int,float)) or not math.isfinite(n) else n;t=attempt_at
 with db(True) as c:
  written=c.execute('''INSERT INTO account_usage_records(account_id,usage,error,code,remaining,unlimited,last_success_at,last_attempt_at,updated_at) VALUES(?,?,NULL,NULL,?,?,?,?,?) ON CONFLICT(account_id) DO UPDATE SET usage=excluded.usage,error=NULL,code=NULL,remaining=excluded.remaining,unlimited=excluded.unlimited,last_success_at=excluded.last_success_at,last_attempt_at=excluded.last_attempt_at,updated_at=excluded.updated_at WHERE account_usage_records.last_attempt_at<=excluded.last_attempt_at''',(aid,dumps(u),n,int(un),t,t,t)).rowcount
  if not written:raise WriteConflict('发现更新的官方余额记录，本次未覆盖')
 return n

def verify_raccoon_daily(w,aid,proxy,day):
 r,d=account(aid);secret=secret_of(d)
 token=secret.get('accessToken') or d.get('accessToken') or ''
 identity=secret.get('officeIdentity') or secret.get('office_identity') or d.get('officeIdentity') or ''
 env=os.environ.copy()
 for k in list(env):
  if 'proxy' in k.lower():env.pop(k,None)
 if proxy:
  cf=w.temp/'proxychains.conf'
  if not cf.exists():raise RuntimeError('账单核对代理配置缺失；禁止回退直连')
  env['LD_PRELOAD']='/usr/lib/x86_64-linux-gnu/libproxychains.so.4';env['PROXYCHAINS_CONF_FILE']=str(cf)
 try:
  process=subprocess.run([sys.executable,'/app/raccoon_ledger.py'],input=dumps({'token':token,'identity':identity,'date':day,'timeout':120}),text=True,capture_output=True,env=env,timeout=130)
 except subprocess.TimeoutExpired:raise TimeoutError('账单核对超时，未确认成功')
 if process.returncode:raise RuntimeError('账单核对进程失败，未确认成功')
 result=json.loads(process.stdout)
 if result.get('error'):raise RuntimeError(result['error'])
 claim=result['claim'];proof=claim.get('evidence',{})
 if claim.get('verifiedDate')!=day or proof.get('date')!=day:raise RuntimeError('账单核对日期不一致，拒绝回写')
 if claim.get('outcome')=='done':
  from raccoon_ledger import daily_grant
  if not any(daily_grant(item,day) for item in proof.get('grants',[])):raise RuntimeError('缺少有效入账证据，拒绝回写')
 with db() as c:c.execute('insert or replace into ledger values(?,?)',('daily-grant-proof:'+day+':'+aid,encode({'value':proof})))
 return claim

def write_checkin(aid,claim):
 if claim.get('verifiedDate') and claim['verifiedDate']!=today():raise WriteConflict('账单核对跨日，未回写到新的一天')
 ok=not claim.get('error') and (claim.get('outcome') in ['claimed','done'] or claim.get('success') is True or claim.get('alreadyCompleted') is True)
 t=now();r,d=account(aid)
 with db(True) as c:
  c.execute('begin immediate')
  if ok:
   row=c.execute('select data from accounts where id=?',(aid,)).fetchone();d=decode(row[0]);d['checkinAt']=t;c.execute('update accounts set data=? where id=? and data=?',(encode(d),aid,row[0]))
  c.execute('insert into checkin_records(ts,date,source,reason,account_id,account_name,provider,outcome,ok,message,error,kept_active) values(?,?,?,?,?,?,?,?,?,?,?,0)',(t,today(),'maintenance-v1','账单核对（未重复领取）' if claim.get('evidence') else '独立容器／官方插件',aid,str(d.get('name',aid))[:100],r['provider'],str(claim.get('outcome','unknown')),int(bool(ok)),scrub(claim.get('message') or claim.get('msg') or ''),None if ok else scrub(claim.get('error') or '官方接口未确认签到完成')))
 return bool(ok)

def cooldown(scope,seconds,reason):
 with db() as c:
  old=c.execute('select failures from cooldowns where scope=?',(scope,)).fetchone()
  seconds=max(seconds,min(86400,seconds*(2**min(old[0] if old else 0,5))))
  c.execute('insert into cooldowns values(?,?,1,?) on conflict(scope) do update set until=max(until,excluded.until),failures=failures+1,reason=excluded.reason',(scope,now()+int(seconds*1000),reason))
def classify(e):
 text=str(e).lower()
 if '429' in text or 'rate limit' in text:return 'cooling','上游 429 限流；账户与平台已冷却',900
 if isinstance(e,TimeoutError) or 'timeout' in text or 'timed out' in text or '超时' in text:return 'cooling','请求超时；保留旧余额并冷却',300
 if '401' in text or 'authorization_verify_error' in text or '请重新登录' in text:return 'needs_login','鉴权失败，凭证需要检查或重新登录',3600
 if '403' in text:return 'cooling','上游 403；未判定账号失效，已冷却',1800
 return 'error',scrub(e),300

def execute(job):
 w=None;aid=job['account'];p=job['provider'];a=job['action'];conf=settings();pc=conf['providers'][p];status='error';msg='';px=None
 try:
  delay=random.uniform(pc['jitterMin'],pc['jitterMax']);STOP.wait(delay)
  if STOP.is_set():raise RuntimeError('服务正在停止')
  HEALTH.require_available(pc[a]['route'])
  params={'provider':p,'account':aid}
  if a!='balance':
   px=select_proxy(pc[a]['route'])
   with db() as c:c.execute('update jobs set proxy=? where id=?',(px['label'] if px else '直连',job['id']))
   w=Worker(aid,px,conf['timeout'])
  if a=='balance':
   current=next((r for r in get_snapshot(force=True)['accounts'] if r['id']==aid),{})
   if job.get('origin')=='auto' and balance_waits_for_checkin(current):raise WriteConflict('已确认零积分，等待下次签到成功后查询')
   if not (ROOT/'bridge-enabled.json').exists():raise RuntimeError('余额桥尚未验收启用；未发起查询')
   report=official_client().call('/api/accounts/usage?id='+urllib.parse.quote(aid,safe=''))
   row=next((x for x in report.get('results',[]) if x.get('id')==aid),None)
   if not row:raise RuntimeError('官方接口缺少当前账户结果')
   if row.get('error'):raise RpcError({'message':row['error'],'code':row.get('code') or 'upstream'})
   u=row.get('usage') or {};n=u.get('totalLeft',u.get('available'))
   status='ok';msg='官方已保存余额并同步调度缓存：'+str(n if n is not None else '无限/未给数值')
  elif a=='checkin':
   day=today()
   if p=='raccoon' and job.get('attempt',0)<0:
    created_day=datetime.datetime.fromtimestamp(job['created']/1000,TZ).strftime('%Y-%m-%d')
    if day!=created_day:raise WriteConflict('历史账单核对任务已跨日，未执行或回写')
    claim=verify_raccoon_daily(w,aid,px,day)
   else:
    claim=w.rpc('checkin.claim',{**params,'manual':False})
    message=str(claim.get('message') or claim.get('msg') or '')
    if p=='raccoon' and (claim.get('outcome')=='inactive' or '未见积分入账' in message or '未能核对今日入账' in message):claim=verify_raccoon_daily(w,aid,px,day)
   ok=write_checkin(aid,claim)
   status='ok' if ok else 'unconfirmed' if claim.get('outcome')=='inactive' else 'error'
   msg=('签到完成并已回写：' if ok else '未确认签到成功：')+scrub(claim.get('message') or claim.get('msg') or claim.get('outcome',''))
   if ok:
    queue_post_checkin_balance(aid,p,now());msg+='；已安排签到后余额刷新'
  else:
   if p=='loomy':status='unsupported';msg='Loomy 官方刷新不提供有效续期；未伪装成刷新成功'
   else:
    exp=w.rpc('token.expiring',params)
    if exp is not True:status='skipped';msg='官方判断凭证未临期或不支持续期，无需刷新'
    else:
     w.rpc('token.refresh',params)
     if w.changed:status='ok';msg='官方插件完成续期，凭证已条件写回'
     else:status='skipped';msg='官方接口未产生凭证变更；未计作续期成功'
 except WriteConflict as e:
  status='skipped';msg=str(e)
 except Exception as e:
  if isinstance(e,ProxyUnavailable) or 'MAINTENANCE_PROXY_WAIT' in str(e):
   status='queued';msg='等待可用维护节点；本次未发起上游维护请求'
  else:
   status,msg,sec=classify(e)
   if isinstance(e,RpcError):
    detail=e.obj.get('data') or {}
    if isinstance(detail,dict):
     retry=detail.get('retry_after',detail.get('retryAfter'))
     if isinstance(retry,(int,float)) and retry>0:sec=max(sec,retry)
   cooldown('account:'+aid,sec,msg)
   if '429' in str(e):cooldown('provider:'+p,sec,msg)
   if a=='checkin':
    try:write_checkin(aid,{'outcome':'error','error':msg,'message':'请求未得到成功确认；上游是否已执行需核对'})
    except Exception:pass
 finally:
  if w:w.close()
  with db() as c:
   if status=='queued':c.execute("update jobs set status='queued',started=null,ended=null,message=? where id=?",(msg,job['id']))
   else:c.execute('update jobs set status=?,ended=?,message=? where id=?',(status,now(),msg,job['id']))
  with LOCK:ACTIVE.pop(aid,None);SNAP.clear()

def queue_post_checkin_balance(aid,provider,required_after):
 # The account is still reserved in ACTIVE until this transaction finishes.
 with LOCK,db() as c:
  c.execute('begin immediate')
  existing=c.execute("select id,status from jobs where account=? and action='balance' and status in ('queued','running') order by created,id",(aid,)).fetchone()
  if existing:
   c.execute("update jobs set origin='post_checkin',priority=100,required_after=max(required_after,?),message='签到成功，优先刷新余额' where id=?",(required_after,existing['id']))
   return existing['id']
  jid=secrets.token_hex(12)
  c.execute("insert into jobs(id,account,provider,action,status,created,message,attempt,origin,priority,required_after) values(?,?,?,'balance','queued',?,'签到成功，优先刷新余额',0,'post_checkin',100,?)",(jid,aid,provider,required_after,required_after))
  return jid

def enqueue(ids,action,skip_positive=False,verify_only=False,origin='manual'):
 if action not in ['balance','checkin','credential']:raise ValueError('未知任务')
 if verify_only and action!='checkin':raise ValueError('只读账单核对仅用于签到')
 rows={r['id']:r for r in get_snapshot()['accounts']};added=0;skipped=0
 with LOCK,db() as c:
  c.execute('begin immediate')
  for aid in dict.fromkeys(ids):
   r=rows.get(aid)
   if not r or (verify_only and r['provider']!='raccoon') or not r['supported'] or (r['needsLogin'] and action!='credential') or r['cooldownUntil']>now() or (skip_positive and (r['balance'] or 0)>0) or (action=='checkin' and r['signed']) or (action=='credential' and not r.get('credentialRenewable')):skipped+=1;continue
   if c.execute("select 1 from jobs where account=? and action=? and status in ('queued','running')",(aid,action)).fetchone():skipped+=1;continue
   c.execute('insert into jobs(id,account,provider,action,status,created,message,attempt,origin) values(?,?,?,?,?,?,?,?,?)',(secrets.token_hex(12),aid,r['provider'],action,'queued',now(),'账单核对（不重复领取）' if verify_only else '等待执行',-1 if verify_only else 0,origin));added+=1
 return {'added':added,'skipped':skipped}

def auto_enqueue():
 cfg=settings()
 if cfg['paused']:return
 snap=get_snapshot();t=now()
 with db() as c:
  pending_rows=c.execute("select account,action,provider from jobs where status in ('queued','running')").fetchall()
  pending={(r[0],r[1]) for r in pending_rows}
  pending_load=collections.Counter((r[2],r[1]) for r in pending_rows)
  last={(r[0],r[1]):r[2] for r in c.execute('select account,action,max(created) from jobs group by account,action')}
  day_start=int(datetime.datetime.now(TZ).replace(hour=0,minute=0,second=0,microsecond=0).timestamp()*1000)
  attempts={r[0]:r[1] for r in c.execute("select account,count(*) from jobs where action='checkin' and attempt>=0 and created>=? and status not in ('cancelled','skipped') group by account",(day_start,))}
 for a in ['credential','checkin','balance']:
  candidates=collections.defaultdict(list)
  for r in snap['accounts']:
   if not r['supported'] or (r['needsLogin'] and a!='credential') or r['cooldownUntil']>t or (r['id'],a) in pending:continue
   cfgp=cfg['providers'][r['provider']][a]
   if not cfgp['enabled'] or cfgp.get('paused',False):continue
   prev=last.get((r['id'],a),0)
   if a=='credential':
    if credential_due(r,prev,t,cfgp):candidates[r['provider']].append((r.get('credentialExpiresAt') or 2**62,prev,r['id']))
    continue
   interval=cfgp['interval']
   if a=='balance':
    if balance_waits_for_checkin(r):continue
    interval=balance_interval(r,cfgp);prev=max(prev,r['lastAttempt'],r['lastSuccess'])
   if a=='checkin':
    clock=datetime.datetime.now(TZ).strftime('%H:%M')
    if r['signed'] or not cfgp.get('start','00:00')<=clock<cfgp.get('end','23:00'):continue
    if attempts.get(r['id'],0)>=cfgp.get('maxAttempts',3):continue
    interval=cfgp.get('retryInterval',1800)
    if prev<day_start:prev=0
   if (a=='balance' and r.get('signedAt',0)>r['lastSuccess']) or t-prev>=interval*1000:candidates[r['provider']].append((0 if a=='balance' and (r.get('balance') or 0)>0 else 1,prev,r['id']))
  # Separate queue budgets prevent balance backlog from blocking daily/renewal work.
  for provider,items in candidates.items():
   cap=cfg['providers'][provider][a].get('concurrency',1)
   count=max(0,cap*cfg.get('queueBufferMultiplier',3)-pending_load[(provider,a)])
   if count:enqueue([item[2] for item in sorted(items)[:count]],a,origin='auto')

def dispatch_jobs(cfg,pool):
 routes=HEALTH.available_routes()
 snap={r['id']:r for r in get_snapshot()['accounts']};timestamp=now()
 with LOCK,db() as c:
  active=collections.Counter(v[0] for v in ACTIVE.values());active_action=collections.Counter(ACTIVE.values());balance_active=sum(v[1]=='balance' for v in ACTIVE.values());cap=cfg['globalConcurrency']-len(ACTIVE)
  if cap<=0:return
  # Rank separately per platform AND operation; a balance backlog cannot hide other work.
  jobs=[dict(j) for j in c.execute("select * from (select j.*,row_number() over(partition by provider,action order by priority desc,created,id) as dispatch_rank from jobs j where status='queued')")]
  for j in sorted(jobs,key=lambda j:queue_order(j,snap)):
   aid=j['account'];p=j['provider'];a=j['action'];r=snap.get(aid)
   if not r:
    c.execute("update jobs set status='skipped',ended=?,message='账户已不存在' where id=?",(timestamp,j['id']));continue
   if aid in ACTIVE or cfg['providers'][p][a].get('paused',False):continue
   if a=='balance' and j.get('origin')=='auto' and balance_waits_for_checkin(r):
    c.execute("update jobs set status='skipped',ended=?,message='已确认零积分，等待下次签到成功后查询' where id=?",(timestamp,j['id']));continue
   if a=='balance' and balance_satisfied(j,r,timestamp,cfg['providers'][p]['balance']):
    c.execute("update jobs set status='skipped',ended=?,message='已有更新的成功余额，本轮跳过' where id=?",(timestamp,j['id']));continue
   if a=='checkin' and j['attempt']>=0:
    clock=datetime.datetime.now(TZ).strftime('%H:%M');cp=cfg['providers'][p][a]
    if not cp.get('start','00:00')<=clock<cp.get('end','23:00'):continue
    if r['signed']:
     c.execute("update jobs set status='skipped',ended=?,message='今日已签到，本轮跳过' where id=?",(timestamp,j['id']));continue
   if a=='credential' and not r.get('credentialRenewable'):
    c.execute("update jobs set status='unsupported',ended=?,message='当前凭证不支持官方续期' where id=?",(timestamp,j['id']));continue
   if cfg['providers'][p][a]['route'] not in routes:continue
   cd=c.execute('select 1 from cooldowns where scope in (?,?) and until>?',('account:'+aid,'provider:'+p,timestamp)).fetchone()
   if a=='balance' and balance_active>=cfg.get('globalBalanceConcurrency',16):continue
   if cd or active[p]>=cfg['providers'][p]['concurrency'] or active_action[(p,a)]>=cfg['providers'][p][a].get('concurrency',1):continue
   c.execute("update jobs set status='running',started=? where id=? and status='queued'",(timestamp,j['id']));c.commit();ACTIVE[aid]=(p,a);active[p]+=1;active_action[(p,a)]+=1;pool.submit(execute,j);cap-=1
   if a=='balance':balance_active+=1
   if cap<=0:break

def scheduler():
 pool=concurrent.futures.ThreadPoolExecutor(max_workers=32);last_auto=0
 while not STOP.wait(1):
  try:
   cfg=settings()
   if cfg['paused']:continue
   if time.time()-last_auto>10:auto_enqueue();last_auto=time.time()
   dispatch_jobs(cfg,pool)
  except Exception as e:event('scheduler',type(e).__name__+': '+scrub(e))
 pool.shutdown(wait=True)

def routing_observer():
 while not STOP.is_set():
  try:
   response=official_client().call('/api/routing/queue');entries={}
   for bucket in ['ordered','blocked']:
    for row in response.get(bucket,[]):
     aid=row.get('id')
     if aid:entries[aid]={'state':str(row.get('state','unknown')),'reason':scrub(row.get('reason','')),'bucket':bucket,'resetAt':row.get('resetAt')}
   with ROUTING_LOCK:ROUTING.update(at=now(),entries=entries,error='',strategy=response.get('strategy'),scope='未指定模型的官方调度队列')
  except Exception as e:
   with ROUTING_LOCK:ROUTING['error']='官方调度快照读取失败：'+scrub(type(e).__name__)
  if STOP.wait(30):break

def routing_snapshot():
 with ROUTING_LOCK:return dict(ROUTING,entries=dict(ROUTING['entries']))

def ownership_guard():
 # Only official settings APIs are used; credentials and proxy bindings are not changed.
 while not STOP.wait(10):
  try:
   snap=get_snapshot(True);ids=snap['autoBalanceIds']
   if ids:OFFICIAL.call('/api/accounts/batch',{'action':'usageQuery','ids':ids,'usageQuery':{'enabled':False,'interval':0}});event('ownership',f'关闭 {len(ids)} 个新增/变更账户的官方自动余额查询')
  except Exception as e:event('ownership','自动余额接管检查失败：'+type(e).__name__)

def bridge_usage(params):
 aid=str(params.get('account',''));provider=str(params.get('provider',''))
 if provider not in SUPPORTED:raise ValueError('该平台未启用余额桥')
 row,_=account(aid)
 if row['provider']!=provider:raise ValueError('账户平台不匹配')
 cfg=settings();pc=cfg['providers'][provider]
 with LOCK:
  if aid in BRIDGE_BUSY:raise RuntimeError('该账户余额查询正在执行')
  if len(BRIDGE_BUSY)>=cfg['globalConcurrency'] or sum(p==provider for p in BRIDGE_BUSY.values())>=min(pc['concurrency'],pc['balance'].get('concurrency',1)):raise RuntimeError('余额桥并发已满，请排队重试')
  BRIDGE_BUSY[aid]=provider
 w=None;start=now();label='直连';status='error'
 try:
  with db() as c:
   cd=c.execute('select 1 from cooldowns where scope in (?,?) and until>?',('account:'+aid,'provider:'+provider,now())).fetchone()
   if cd:raise RuntimeError('账户或平台处于维护冷却期')
  px=select_proxy(pc['balance']['route']);label=px['label'] if px else '直连'
  with db() as c:c.execute("update jobs set proxy=? where account=? and action='balance' and status='running'",(label,aid))
  w=Worker(aid,px,min(cfg['timeout'],45));result=w.rpc('usage.query',{'provider':provider,'account':aid});status='ok'
  return result
 finally:
  if w:w.close()
  with db() as c:c.execute('insert into bridge_audit values(?,?,?,?,?,?)',(start,aid,provider,label,status,now()-start))
  with LOCK:BRIDGE_BUSY.pop(aid,None)

with db() as c:
 c.execute('create index if not exists jobs_status_queue on jobs(status,provider,action,created)')
 c.execute('create index if not exists jobs_created on jobs(created desc)')
HEALTH=AutoProxyHealth(db,decode,settings)
UI_LOCK=threading.Lock();UI_WAKE=threading.Event();UI_STATE={'data':None,'error':''}
def build_ui_state():
 snap=enrich(scope_snapshot(get_snapshot()),settings(),db,now(),TZ,routing_snapshot())
 return {**snap,'config':settings(),'cacheSync':'余额返回官方主程序落库并更新调度缓存','supported':sorted(SUPPORTED),'builtAt':now()}
def ui_observer():
 while not STOP.is_set():
  try:
   data=build_ui_state()
   with UI_LOCK:UI_STATE.update(data=data,error='')
  except Exception as e:
   with UI_LOCK:UI_STATE['error']='维护视图更新失败：'+type(e).__name__
  UI_WAKE.wait(5);UI_WAKE.clear()
def ui_state():
 with UI_LOCK:
  data=UI_STATE['data'];error=UI_STATE['error']
 if data is None:return None
 return {**data,'viewError':error,'viewAgeSeconds':max(0,(now()-data['builtAt'])//1000)}

class Handler(BaseHTTPRequestHandler):
 server_version='Maintenance/1.0'
 def log_message(self,*args):pass
 def send(self,status,data,html=False,cookie=None,content_type=None):
  body=data.encode() if html else dumps(data).encode();compressed='gzip' in self.headers.get('Accept-Encoding','') and len(body)>1024
  if compressed:body=gzip.compress(body,compresslevel=3)
  self.send_response(status)
  if compressed:self.send_header('Content-Encoding','gzip');self.send_header('Vary','Accept-Encoding')
  self.send_header('Content-Type',content_type or ('text/html; charset=utf-8' if html else 'application/json; charset=utf-8'));self.send_header('Content-Length',str(len(body)));self.send_header('Cache-Control','no-store');self.send_header('X-Content-Type-Options','nosniff');self.send_header('X-Frame-Options','DENY');self.send_header('Referrer-Policy','no-referrer');self.send_header('Content-Security-Policy',"default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'")
  if cookie:self.send_header('Set-Cookie',cookie)
  self.end_headers();self.wfile.write(body)
 def authed(self):
  try:c=http.cookies.SimpleCookie(self.headers.get('Cookie',''));token=c['maintenance_session'].value;return SESSIONS.get(token,0)>time.time()
  except:return False
 def do_GET(self):
  try:
   path=urllib.parse.urlsplit(self.path).path
   if path=='/health':return self.send(200,{'ok':True,'version':'1.0','engine':'official-plugin-3.4.2'})
   if path=='/':return self.send(200,pathlib.Path('/app/index.html').read_text(),True)
   if path=='/ui.js':return self.send(200,pathlib.Path('/app/ui.js').read_text(),html=True,content_type='application/javascript; charset=utf-8')
   static={'/ux.js':('ux.js','application/javascript'),'/ui.css':('ui.css','text/css'),'/vendor/tom-select.min.js':('vendor/tom-select.min.js','application/javascript'),'/vendor/tom-select.css':('vendor/tom-select.css','text/css')}
   if path in static:
    f,mime=static[path];return self.send(200,(pathlib.Path('/app')/f).read_text(),html=True,content_type=mime+'; charset=utf-8')
   if not self.authed():return self.send(401,{'error':'请登录'})
   if path in ['/api/state','/api/accounts','/api/queue','/api/history','/api/proxies']:
    query=urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query)
    if path=='/api/proxies':return self.send(200,{'items':HEALTH.rows(),'health':HEALTH.status()})
    snap=ui_state()
    if snap is None:return self.send(503,{'error':'首次维护快照正在加载，请稍候'})
    if path=='/api/state':
     snap['config']=settings()
     if query.get('compact')==['1']:snap={k:v for k,v in snap.items() if k!='accounts'}
     return self.send(200,{**snap,'proxies':HEALTH.rows(),'proxyHealth':HEALTH.status()})
    if path=='/api/accounts':return self.send(200,account_page(snap,query))
    if path=='/api/queue':return self.send(200,queue_view(snap,settings(),db,now(),query,HEALTH.available_routes()))
    result=history_view(db,query);names={r['id']:r['name'] for r in snap['accounts']}
    for item in result['items']:item['name']=names.get(item['account'],item['account'])
    return self.send(200,result)
   return self.send(404,{'error':'不存在'})
  except (ValueError,KeyError) as e:self.send(400,{'error':scrub(e)})
  except (BrokenPipeError,ConnectionResetError):pass
  except Exception as e:self.send(500,{'error':scrub(type(e).__name__+': '+str(e))})
 def do_POST(self):
  try:
   origin=self.headers.get('Origin');host=self.headers.get('Host')
   if origin and urllib.parse.urlsplit(origin).netloc!=host:return self.send(403,{'error':'来源不匹配'})
   if self.headers.get('Content-Type','').split(';')[0]!='application/json':return self.send(415,{'error':'需要 JSON'})
   n=int(self.headers.get('Content-Length','0'))
   if n>1000000:return self.send(413,{'error':'请求过大'})
   d=json.loads(self.rfile.read(n) or b'{}');path=urllib.parse.urlsplit(self.path).path
   if path=='/internal/usage':
    token=self.headers.get('Authorization','')
    if not hmac.compare_digest(token.encode(),('Bearer '+BRIDGE_KEY).encode()):return self.send(401,{'error':{'message':'未授权','code':'unauthorized'}})
    try:return self.send(200,{'result':bridge_usage(d)})
    except RpcError as e:return self.send(200,{'error':{'message':scrub(e.obj.get('message',str(e))),'code':e.obj.get('code','upstream')}})
    except Exception as e:return self.send(200,{'error':{'message':scrub(e),'code':'maintenance_bridge'}})
   if path=='/api/login':
    ip=self.client_address[0];attempts=[x for x in LOGIN_ATTEMPTS.get(ip,[]) if x>time.time()-300]
    if len(attempts)>=10:return self.send(429,{'error':'登录尝试过多，等待5分钟'})
    if not (hmac.compare_digest(str(d.get('username','')).encode(),AUTH['username'].encode()) and hmac.compare_digest(str(d.get('password','')).encode(),AUTH['password'].encode())):
     LOGIN_ATTEMPTS[ip]=attempts+[time.time()];return self.send(401,{'error':'用户名或密码错误'})
    token=secrets.token_urlsafe(32);SESSIONS[token]=time.time()+28800;return self.send(200,{'ok':True},cookie='maintenance_session='+token+'; HttpOnly; SameSite=Strict; Path=/; Max-Age=28800')
   if not self.authed():return self.send(401,{'error':'请登录'})
   if path=='/api/logout':
    cookies=http.cookies.SimpleCookie(self.headers.get('Cookie',''));SESSIONS.pop(cookies['maintenance_session'].value,None)
    return self.send(200,{'ok':True},cookie='maintenance_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0')
   if path=='/api/settings':
    result=save_settings(d);UI_WAKE.set();return self.send(200,result)
   if path=='/api/proxies/health-settings':return self.send(200,HEALTH.save_policy(d))
   if path=='/api/proxies/test':return self.send(202,HEALTH.start(d.get('ids')))
   if path=='/api/proxies/toggle':return self.send(200,HEALTH.toggle(str(d.get('id','')),d.get('enabled')))
   if path=='/api/jobs':
    ids=d.get('ids',[])
    if not isinstance(ids,list) or len(ids)>100:return self.send(400,{'error':'第一版每次最多选择100个，避免误点全量'})
    return self.send(200,enqueue(ids,d.get('action'),bool(d.get('skipPositive')),bool(d.get('verifyOnly'))))
   if path=='/api/jobs/cancel':
    ids=d.get('jobIds')
    if not isinstance(ids,list) or not ids or len(ids)>100 or any(not isinstance(x,str) for x in ids):raise ValueError('请选择1—100条待执行任务；不取消执行中的请求')
    with LOCK,db() as c:
     n=c.execute("update jobs set status='cancelled',ended=?,message='用户取消待执行任务' where status='queued' and id in ("+','.join('?' for _ in ids)+')',(now(),*ids)).rowcount
    return self.send(200,{'cancelled':n})
   if path=='/api/proxies':
    url=str(d.get('url',''));u=urllib.parse.urlsplit(url)
    if u.scheme not in ['http','socks5','socks4'] or not re.fullmatch(r'[0-9.]+',u.hostname or '') or not u.port:raise ValueError('请输入 http://IPv4:端口 或 socks5://IPv4:端口，可含用户名密码')
    if any(x.isspace() for x in urllib.parse.unquote(u.username or '')+urllib.parse.unquote(u.password or '')):raise ValueError('代理凭证不支持空白字符')
    pid=secrets.token_hex(8)
    with db() as c:c.execute('insert into proxies values(?,?,?,1)',(pid,str(d.get('label') or '节点 '+pid[:4])[:60],encode({'url':url})))
    return self.send(200,{'id':pid,'ok':True})
   if path=='/api/proxies/delete':
    return self.send(200,HEALTH.delete(str(d.get('id',''))))
   return self.send(404,{'error':'不存在'})
  except (ValueError,KeyError) as e:self.send(400,{'error':scrub(e)})
  except Exception as e:self.send(500,{'error':scrub(type(e).__name__+': '+str(e))})

if __name__=='__main__':
 signal.signal(signal.SIGTERM,lambda *_:STOP.set())
 threading.Thread(target=HEALTH.run,args=(STOP,),daemon=True).start();threading.Thread(target=ui_observer,daemon=True).start();threading.Thread(target=routing_observer,daemon=True).start();threading.Thread(target=scheduler,daemon=True).start();threading.Thread(target=ownership_guard,daemon=True).start()
 server=ThreadingHTTPServer(('0.0.0.0',8080),Handler);server.timeout=1
 print('Maintenance v1 ready; official plugin; automatic tasks paused by default',flush=True)
 while not STOP.is_set():server.handle_request()
 server.server_close()

"""Read-only UI projections and bounded, explicit proxy health checks."""
import datetime,json,os,pathlib,subprocess,tempfile,threading,time,urllib.parse,re,ipaddress,concurrent.futures
SUPPORTED={'loomy','raccoon'}
def scope_snapshot(snapshot):
    rows=[x for x in snapshot['accounts'] if x['provider'] in SUPPORTED]
    s=dict(snapshot);s['accounts']=rows;s.pop('autoBalanceIds',None)
    s['summary']={'total':len(rows),'positive':sum((r.get('balance') or 0)>0 for r in rows),'knownBalance':sum(r.get('lastSuccess',0)>0 for r in rows),'signedToday':sum(r.get('signed',False) for r in rows),'cooling':sum(r.get('cooldownUntil',0)>s['at'] for r in rows),'needsLogin':sum(r.get('needsLogin',False) for r in rows)}
    return s

def account_page(state,query):
    value=lambda k:query.get(k,[''])[0]
    page=max(0,min(100000,int(value('page') or 0)));size=max(1,min(100,int(value('size') or 50)))
    provider=value('provider');term=value('search').strip().casefold();f=value('filter');now=state['at']
    def match(r):
        if provider and r['provider']!=provider:return False
        if term and term not in (r['name']+' '+r['id']).casefold():return False
        flags={'usable':r['routing']['state']=='usable','positive':r['balanceKind']=='positive','zero':r['balanceKind']=='zero','unknown':r['balanceKind']=='unknown','due':r['balanceDetail']['due'],'queued':r['balanceDetail']['state']=='queued','stale':r['lastSuccess']>0 and not r['balanceDetail']['fresh'],'needs_login':r['needsLogin'],'cooling':r['cooldownUntil']>now,'unsigned':not r['signed'],'renewable':r.get('credentialRenewable',False)}
        return not f or flags.get(f,False)
    rows=[r for r in state['accounts'] if match(r)];total=len(rows);page=min(page,max(0,(total-1)//size))
    return {'at':now,'total':total,'page':page,'size':size,'items':rows[page*size:(page+1)*size]}

class ProxyHealth:
    def __init__(self,db,decode,settings):
        self.db=db;self.decode=decode;self.settings=settings;self.lock=threading.Lock();self.active=set();self.pool=concurrent.futures.ThreadPoolExecutor(max_workers=3,thread_name_prefix='proxy-health')
        with db() as c:c.execute('create table if not exists proxy_health(id text primary key,checked_at integer,status text,latency_ms integer,http_status integer,message text)')
    def rows(self):
        with self.db() as c:rows=[dict(r) for r in c.execute('select p.id,p.label,p.enabled,h.checked_at,h.status,h.latency_ms,h.http_status,h.message from proxies p left join proxy_health h on p.id=h.id order by p.enabled desc,p.label,p.id')]
        with self.lock:active=set(self.active)
        for r in rows:
            r['testing']=r['id'] in active;r['status']=r['status'] or 'untested'
        return rows
    def refs(self,pid):
        return [p+'/'+a for p,pc in self.settings()['providers'].items() for a in ['balance','checkin','credential'] if pc[a].get('route')==pid]
    def toggle(self,pid,enabled):
        if not isinstance(enabled,bool):raise ValueError('enabled 必须为布尔值')
        if not enabled and self.refs(pid):raise ValueError('节点被固定出口设置引用，请先更换对应出口')
        with self.db() as c:
            if not c.execute('update proxies set enabled=? where id=?',(int(enabled),pid)).rowcount:raise ValueError('节点不存在')
        return {'ok':True,'enabled':enabled}
    def delete(self,pid):
        if self.refs(pid):raise ValueError('节点被固定出口设置引用，请先更换对应出口')
        with self.lock:
            if pid in self.active:raise ValueError('节点正在检测，请等待检测结束')
        with self.db() as c:
            c.execute('delete from proxies where id=?',(pid,));c.execute('delete from proxy_health where id=?',(pid,))
        return {'ok':True}
    def start(self,ids):
        if not isinstance(ids,list) or not 1<=len(ids)<=100 or any(not isinstance(i,str) for i in ids):raise ValueError('请选择1—100个节点')
        with self.db() as c:known={r['id']:dict(r) for r in c.execute('select * from proxies')}
        if any(i not in known for i in ids):raise ValueError('包含不存在的节点')
        added=[]
        with self.lock:
            if len(self.active)+len(set(ids)-self.active)>105:raise ValueError('检测队列已满')
            for i in dict.fromkeys(ids):
                if i in self.active:continue
                self.active.add(i);added.append(i)
        for i in added:self.pool.submit(self._run,known[i])
        return {'queued':len(added),'skipped':len(set(ids))-len(added),'concurrency':3,'timeoutSeconds':12}
    def _run(self,row):
        start=time.monotonic();status='failed';code=0;ms=None;message='代理连通检测失败'
        try:
            u=urllib.parse.urlsplit(self.decode(row['secret'])['url']);ipaddress.IPv4Address(u.hostname)
            typ={'http':'http','socks5':'socks5','socks4':'socks4'}[u.scheme];user=urllib.parse.unquote(u.username or '');pw=urllib.parse.unquote(u.password or '')
            if not u.port or any(ch.isspace() for ch in user+pw):raise ValueError('invalid proxy')
            env={k:v for k,v in os.environ.items() if 'proxy' not in k.lower()}
            with tempfile.TemporaryDirectory(prefix='node-health-') as tmp:
                cfg=pathlib.Path(tmp)/'proxychains.conf';cfg.write_text('strict_chain\nquiet_mode\nproxy_dns\ntcp_read_time_out 7000\ntcp_connect_time_out 5000\n[ProxyList]\n'+f'{typ} {u.hostname} {u.port}'+(f' {user} {pw}' if user else '')+'\n');cfg.chmod(0o600)
                env.update(LD_PRELOAD='/usr/lib/x86_64-linux-gnu/libproxychains.so.4',PROXYCHAINS_CONF_FILE=str(cfg))
                script="import urllib.request,json,time; t=time.monotonic(); o=urllib.request.build_opener(urllib.request.ProxyHandler({})); r=o.open('https://www.gstatic.com/generate_204',timeout=9); r.read(32); print(json.dumps({'code':r.status,'ms':round((time.monotonic()-t)*1000)}))"
                p=subprocess.run(['/usr/local/bin/python3','-c',script],env=env,capture_output=True,text=True,timeout=12)
                if p.returncode==0:
                    v=json.loads(p.stdout);code=int(v['code']);ms=int(v['ms']);status='ok' if code==204 else 'failed';message='HTTPS 204 连通成功' if status=='ok' else '检测端点未返回预期状态'
                else:message='代理连接、认证、DNS 或 TLS 失败（不代表账户失效）'
        except subprocess.TimeoutExpired:status='timeout';message='检测超过12秒'
        except Exception:message='节点配置或连通检测失败'
        finally:
            try:
                with self.db() as c:c.execute('insert or replace into proxy_health values(?,?,?,?,?,?)',(row['id'],int(time.time()*1000),status,ms,code,message))
                if hasattr(self,'on_result'):self.on_result(row['id'],status)
            finally:
                with self.lock:self.active.discard(row['id'])

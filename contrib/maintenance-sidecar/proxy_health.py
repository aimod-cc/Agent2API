"""Scheduled proxy quarantine, kept separate from the operator's enable switch."""
import hashlib,json,random,threading,time
from ui_support import ProxyHealth

class ProxyUnavailable(RuntimeError):
    pass

DEFAULT_POLICY={'enabled':False,'interval':900,'retryInterval':300,'confirmInterval':60,'failureThreshold':2,'recoveryThreshold':2,'concurrency':3}

class AutoProxyHealth(ProxyHealth):
    def __init__(self,db,decode,settings):
        super().__init__(db,decode,settings)
        self.last_tick=0;self.last_error='';self.last_error_log=0
        with db() as c:
            c.execute('create table if not exists proxy_health_state(id text primary key,fail_streak integer not null default 0,success_streak integer not null default 0,quarantined integer not null default 0,next_check_at integer not null default 0,changed_at integer not null default 0)')
    def policy(self):
        with self.db() as c:row=c.execute("select value from settings where key='proxyHealth'").fetchone()
        return {**DEFAULT_POLICY,**(json.loads(row[0]) if row else {})}
    def save_policy(self,data):
        if not isinstance(data,dict):raise ValueError('检测设置必须为对象')
        cfg=self.policy()
        if 'enabled' in data:
            if not isinstance(data['enabled'],bool):raise ValueError('enabled 必须为布尔值')
            cfg['enabled']=data['enabled']
        for key,lo,hi in [('interval',300,86400),('retryInterval',60,3600),('confirmInterval',30,600),('failureThreshold',2,5),('recoveryThreshold',2,5),('concurrency',1,3)]:
            if key in data:
                if isinstance(data[key],bool):raise ValueError('参数类型错误')
                n=int(data[key])
                if n<lo or n>hi:raise ValueError(key+' 超出允许范围')
                cfg[key]=n
        with self.db() as c:c.execute('insert or replace into settings(key,value) values(?,?)',('proxyHealth',json.dumps(cfg)))
        return cfg
    def rows(self):
        rows=super().rows()
        with self.db() as c:health={r['id']:dict(r) for r in c.execute('select * from proxy_health_state')}
        for r in rows:
            h=health.get(r['id'],{});r.update(autoBlocked=bool(h.get('quarantined')),failureStreak=h.get('fail_streak',0),successStreak=h.get('success_streak',0),nextCheckAt=h.get('next_check_at',0),healthChangedAt=h.get('changed_at',0))
            r['usable']=bool(r['enabled'] and not r['autoBlocked'])
        return rows
    def candidates(self,route):
        if route=='direct':return []
        with self.db() as c:
            sql='select p.* from proxies p left join proxy_health_state h on h.id=p.id where p.enabled=1 and coalesce(h.quarantined,0)=0'
            return [dict(r) for r in c.execute(sql+(' and p.id=?' if route!='auto' else ''),() if route=='auto' else (route,))]
    def available_routes(self):
        with self.db() as c:ids={r[0] for r in c.execute('select p.id from proxies p left join proxy_health_state h on p.id=h.id where p.enabled=1 and coalesce(h.quarantined,0)=0')}
        return {'direct',*ids,*({'auto'} if ids else set())}
    def require_available(self,route):
        if route not in self.available_routes():raise ProxyUnavailable('MAINTENANCE_PROXY_WAIT: 无可用维护节点，保留排队；不会回退直连')
    def delete(self,pid):
        result=super().delete(pid)
        with self.db() as c:c.execute('delete from proxy_health_state where id=?',(pid,))
        return result
    def status(self):
        cfg=self.policy();rows=self.rows()
        return {'config':cfg,'lastTickAt':self.last_tick,'error':self.last_error,'usable':sum(r['usable'] for r in rows),'quarantined':sum(r['autoBlocked'] and r['enabled'] for r in rows),'manuallyDisabled':sum(not r['enabled'] for r in rows),'testing':sum(r['testing'] for r in rows),'total':len(rows)}
    def on_result(self,pid,status):
        cfg=self.policy();ts=int(time.time()*1000);good=status=='ok'
        with self.db() as c:
            c.execute('begin immediate')
            if not c.execute('select 1 from proxies where id=?',(pid,)).fetchone():return
            old=c.execute('select * from proxy_health_state where id=?',(pid,)).fetchone();old=dict(old) if old else {}
            blocked=bool(old.get('quarantined',0));was=blocked
            failures=0 if good else old.get('fail_streak',0)+1
            successes=old.get('success_streak',0)+1 if good else 0
            if not good and failures>=cfg['failureThreshold']:blocked=True
            if good and blocked and successes>=cfg['recoveryThreshold']:blocked=False
            interval=cfg['confirmInterval'] if (good and blocked) or (not good and not blocked) else cfg['retryInterval'] if blocked else cfg['interval']
            next_at=ts+interval*1000+random.randint(0,min(30000,interval*100))
            changed=ts if blocked!=was else old.get('changed_at',0)
            c.execute('insert or replace into proxy_health_state values(?,?,?,?,?,?)',(pid,failures,successes,int(blocked),next_at,changed))
            if blocked!=was:
                label=c.execute('select label from proxies where id=?',(pid,)).fetchone()[0]
                c.execute('insert into events(ts,kind,message) values(?,?,?)',(ts,'proxy_health',('自动隔离节点：' if blocked else '节点检测恢复，重新加入自动候选：')+str(label)[:60]))
    def tick(self,timestamp=None):
        cfg=self.policy();ts=timestamp if timestamp is not None else int(time.time()*1000);self.last_tick=ts
        if not cfg['enabled']:return 0
        with self.db() as c:
            ids=[r[0] for r in c.execute('select id from proxies where enabled=1')]
            for pid in ids:
                spread=int(hashlib.sha256(pid.encode()).hexdigest()[:8],16)%120000
                c.execute('insert or ignore into proxy_health_state(id,next_check_at) values(?,?)',(pid,ts+spread))
            due=[r[0] for r in c.execute('select h.id from proxy_health_state h join proxies p on p.id=h.id where p.enabled=1 and h.next_check_at<=? order by h.next_check_at,h.id',(ts,))]
        with self.lock:
            slots=max(0,cfg['concurrency']-len(self.active));ids=[pid for pid in due if pid not in self.active][:slots]
        if not ids:return 0
        return self.start(ids)['queued']
    def run(self,stop):
        while not stop.wait(2):
            try:self.tick();self.last_error=''
            except Exception as e:
                self.last_error='节点巡检暂时异常：'+type(e).__name__
                if time.time()-self.last_error_log>=60:
                    self.last_error_log=time.time()
                    try:
                        with self.db() as c:c.execute('insert into events(ts,kind,message) values(?,?,?)',(int(time.time()*1000),'proxy_health_error',self.last_error))
                    except Exception:pass

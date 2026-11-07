"""Read-only operational metrics. Official routing state is not inferred from balance."""
import datetime,math
from collections import Counter
from maintenance_policy import balance_interval, credential_due, balance_satisfied, balance_waits_for_checkin
FAILED={'error','cooling','needs_login','interrupted'}

def queue_order(job,accounts):
    row=accounts.get(job['account'],{});action=job['action']
    return (0 if action=='credential' else 1 if action=='checkin' else 2 if action=='balance' and (row.get('balance') or 0)>0 else 3 if job.get('origin')=='post_checkin' else 4,
            (row.get('credentialExpiresAt') or 2**62) if action=='credential' else 0,
            job.get('dispatch_rank',0),job['created'],job['id'])

def filters(query):
    value=lambda k:query.get(k,[''])[0]
    page=max(0,min(100000,int(value('page') or 0)));size=max(1,min(100,int(value('size') or 50)))
    return value,page,size

def queue_view(snapshot,config,db,timestamp,query,available_routes=None):
    value,page,size=filters(query);accounts={r['id']:r for r in snapshot['accounts']}
    with db() as c:
        jobs=[dict(x) for x in c.execute("select j.*,row_number() over(partition by provider,action,status order by priority desc,created,id) dispatch_rank from jobs j where provider in ('raccoon','loomy') and status in ('queued','running')")]
    jobs.sort(key=lambda j:(0 if j['status']=='running' else 1,queue_order(j,accounts)))
    counts=dict(Counter(j['status'] for j in jobs));rank=0;selected=[]
    for job in jobs:
        row=accounts.get(job['account'],{});cfg=config['providers'].get(job['provider'],{}).get(job['action'],{})
        if job['status']=='queued':rank+=1
        job['position']=rank if job['status']=='queued' else None;job['name']=row.get('name',job['account']);job['waitSeconds']=max(0,(timestamp-job['created'])//1000)
        job['cooldownUntil']=row.get('cooldownUntil',0)
        job['holdReason']=''
        if job['status']=='running':job['holdReason']='执行中（暂停不会中断已发出的请求）'
        elif config['paused']:job['holdReason']='全局暂停'
        elif cfg.get('paused'):job['holdReason']='此平台操作队列暂停'
        elif row.get('cooldownUntil',0)>timestamp:job['holdReason']='冷却中'
        elif available_routes is not None and cfg.get('route','auto') not in available_routes:job['holdReason']='等待可用维护节点（不回退直连）'
        elif job['action']=='balance' and job.get('origin')=='auto' and balance_waits_for_checkin(row):job['holdReason']='已确认零积分，等待下次签到成功后查询'
        elif job['action']=='balance' and balance_satisfied(job,row,timestamp,cfg):job['holdReason']='已有新余额，执行前自动跳过'
        elif job['action']=='checkin' and job.get('attempt',0)>=0:
            clock=datetime.datetime.fromtimestamp(timestamp/1000,datetime.timezone(datetime.timedelta(hours=8))).strftime('%H:%M')
            if not cfg.get('start','00:00')<=clock<cfg.get('end','23:00'):job['holdReason']='签到窗口外'
        if value('provider') and job['provider']!=value('provider'):continue
        if value('action') and job['action']!=value('action'):continue
        if value('status') and job['status']!=value('status'):continue
        if value('search').lower() not in (job['name']+' '+job['account']).lower():continue
        selected.append(job)
    page=min(page,max(0,(len(selected)-1)//size))
    return {'at':timestamp,'total':len(selected),'page':page,'size':size,'counts':counts,'items':selected[page*size:(page+1)*size],'orderNote':'显示调度优先顺序；不同平台和操作可并行，冷却/暂停账户会被暂时跳过，不是严格串行名次。'}

def history_view(db,query):
    value,page,size=filters(query);clauses=["provider in ('raccoon','loomy')"];params=[]
    for k in ['provider','action','status']:
        if value(k):clauses.append(k+'=?');params.append(value(k))
    where=' where '+' and '.join(clauses) if clauses else ''
    with db() as c:
        total=c.execute('select count(*) from jobs'+where,params).fetchone()[0]
        page=min(page,max(0,(total-1)//size))
        rows=[dict(x) for x in c.execute('select * from jobs'+where+' order by created desc,rowid desc limit ? offset ?',(*params,size,page*size))]
    return {'items':rows,'total':total,'page':page,'size':size}

def enrich(snapshot,config,db,timestamp,tz,routing=None):
    routing=routing or {'at':0,'entries':{},'error':'尚未读取官方调度'}
    day=datetime.datetime.fromtimestamp(timestamp/1000,tz).replace(hour=0,minute=0,second=0,microsecond=0);start=int(day.timestamp()*1000)
    with db() as c:
        stats=[dict(x) for x in c.execute('select provider,action,status,count(*) attempts,count(distinct account) accounts from jobs where created>=? group by provider,action,status',(start,))]
        unique=[dict(x) for x in c.execute('select provider,action,count(distinct account) accounts from jobs where created>=? group by provider,action',(start,))]
        attempts={x[0]:x[1] for x in c.execute("select account,count(*) from jobs where action='checkin' and attempt>=0 and created>=? and status not in ('cancelled','skipped') group by account",(start,))}
        latest={(x['account'],x['action']):dict(x) for x in c.execute('select * from (select *,row_number() over(partition by account,action order by created desc,rowid desc) rn from jobs) where rn=1')}
        pending=[dict(x) for x in c.execute("select provider,action,status,count(*) attempts from jobs where provider in ('raccoon','loomy') and status in ('queued','running') group by provider,action,status")]
        rates=[dict(x) for x in c.execute("select provider,action,count(*) completed,sum(status='ok') succeeded,round(avg((ended-started)/1000.0),2) meanSeconds from jobs where ended>=? and started is not null and status in ('ok','error','cooling','needs_login','unconfirmed') group by provider,action",(timestamp-600000,))]
    fresh_routing=bool(routing.get('at') and timestamp-routing['at']<=120000 and not routing.get('error'))
    rows=[]
    for original in snapshot['accounts']:
        r=dict(original);balance=r['balance'];known=bool(r['lastSuccess'] and isinstance(balance,(int,float)) and math.isfinite(balance))
        r['balanceKind']='unlimited' if r['unlimited'] else 'unknown' if not known else 'positive' if balance>0 else 'zero' if balance==0 else 'negative'
        cfgp=config['providers'].get(r['provider'],{});cp=cfgp.get('checkin',{});bp=cfgp.get('balance',{'interval':3600});cred=cfgp.get('credential',{})
        j=latest.get((r['id'],'checkin'),{});b=latest.get((r['id'],'balance'),{});cr=latest.get((r['id'],'credential'),{})
        count=attempts.get(r['id'],0);maximum=cp.get('maxAttempts',3);next_sign=0
        if r['signed']:sign='signed'
        elif not r['supported']:sign='unsupported'
        elif j.get('status') in ('queued','running'):sign=j['status']
        elif r['needsLogin']:sign='needs_login'
        elif not cp.get('enabled'):sign='disabled'
        elif count>=maximum:sign='exhausted'
        else:
            prev=j.get('created',0);next_sign=max(timestamp,r['cooldownUntil'],prev+cp.get('retryInterval',1800)*1000 if prev>=start else 0)
            sh,sm=map(int,cp.get('start','00:00').split(':'));eh,em=map(int,cp.get('end','23:00').split(':'))
            ws=int(day.replace(hour=sh,minute=sm).timestamp()*1000);we=int(day.replace(hour=eh,minute=em).timestamp()*1000)
            if next_sign>=we:next_sign=max(next_sign,ws+86400000);sign='outside_window'
            elif next_sign<ws:next_sign=ws;sign='outside_window'
            else:sign='retry_wait' if next_sign>timestamp else 'eligible'
        if not r['signed'] and (config['paused'] or cp.get('paused')) and j.get('status')!='running':sign='paused'
        interval=balance_interval(r,bp);previous=max(b.get('created',0),r['lastAttempt'],r['lastSuccess']);due=previous+interval*1000 if previous else timestamp
        if r.get('signedAt',0)>r['lastSuccess']:due=timestamp
        waiting_sign=balance_waits_for_checkin(r) and not (b.get('origin')=='post_checkin' and b.get('required_after',0)>r['lastSuccess'] and b.get('status') in ('queued','running'))
        balance_state='awaiting_checkin' if waiting_sign else 'waiting' if due>timestamp else 'due'
        if r['cooldownUntil']>timestamp:balance_state='cooling'
        if not bp.get('enabled'):balance_state='disabled'
        if r['needsLogin']:balance_state='needs_login'
        if b.get('status')=='running' or (b.get('status')=='queued' and not waiting_sign):balance_state=b['status']
        if config['paused'] or bp.get('paused'):
            if balance_state!='running':balance_state='paused'
        r['checkinDetail']={'state':sign,'attempts':count,'maxAttempts':maximum,'nextEligibleAt':next_sign,'lastStatus':j.get('status',''),'lastMessage':j.get('message',''),'failedToday':not r['signed'] and j.get('created',0)>=start and j.get('status') in FAILED}
        r['balanceDetail']={'state':balance_state,'status':b.get('status',''),'message':b.get('message',''),'nextEligibleAt':None if waiting_sign else max(due,r['cooldownUntil']),'waitingForCheckin':waiting_sign,'dueAt':None if waiting_sign else due,'interval':interval,'lastSuccess':r['lastSuccess'],'origin':b.get('origin',''),'jobId':b.get('id'),'fresh':known and timestamp-r['lastSuccess']<=interval*1000,'due':not waiting_sign and due<=timestamp and not config['paused'] and r['enabled'] and not r['needsLogin'] and r['cooldownUntil']<=timestamp and bp.get('enabled',False) and not bp.get('paused',False) and balance_state not in ('queued','running')}
        route=routing.get('entries',{}).get(r['id'],{});r['routing']={'state':route.get('state','absent') if fresh_routing else 'unknown','reason':route.get('reason','未进入官方调度队列') if fresh_routing else routing.get('error') or '调度快照已过期','at':routing.get('at',0)}
        r['credentialDetail']={'status':cr.get('status',''),'message':cr.get('message',''),'due':credential_due(r,cr.get('created',0),timestamp,cred),'renewable':r.get('credentialRenewable',False),'expiresAt':r.get('credentialExpiresAt',0),'reason':r.get('credentialReason','未知')}
        rows.append(r)
    platforms={}
    for p in sorted({r['provider'] for r in rows}):
        rr=[r for r in rows if r['provider']==p];finite=[r for r in rr if r['balanceKind'] not in ('unknown','unlimited')];usable=[r for r in rr if r['routing']['state']=='usable']
        platforms[p]={'total':len(rr),'enabled':sum(r['enabled'] for r in rr),'signed':sum(r['signed'] for r in rr),'unsigned':sum(not r['signed'] for r in rr),'balance':dict(Counter(r['balanceKind'] for r in rr)),'checkin':dict(Counter(r['checkinDetail']['state'] for r in rr)),'balanceStates':dict(Counter(r['balanceDetail']['state'] for r in rr)),'positivePoints':sum(max(0,r['balance']) for r in finite),'routable':len(usable) if fresh_routing else None,'routablePoints':sum(max(0,r['balance']) for r in usable if r['balanceKind'] not in ('unknown','unlimited')) if fresh_routing else None,'routableUnknown':sum(r['balanceKind']=='unknown' for r in usable),'routableUnlimited':sum(r['unlimited'] for r in usable),'routingStates':dict(Counter(r['routing']['state'] for r in rr)),'fresh':sum(r['balanceDetail']['fresh'] for r in rr),'due':sum(r['balanceDetail']['due'] for r in rr),'stale':sum(r['lastSuccess']>0 and not r['balanceDetail']['fresh'] for r in rr),'cooling':sum(r['cooldownUntil']>timestamp for r in rr),'needsLogin':sum(r['needsLogin'] for r in rr),'renewable':sum(r.get('credentialRenewable',False) for r in rr),'failedUnfinished':sum(r['checkinDetail']['failedToday'] for r in rr)}
    result=dict(snapshot);result['accounts']=rows
    result['dashboard']={'date':day.strftime('%Y-%m-%d'),'timezone':'Asia/Shanghai','platforms':platforms,'todayAttempts':stats,'todayUnique':unique,'pending':pending,'throughput10m':rates,'routing':{k:v for k,v in routing.items() if k!='entries'},'routingFresh':fresh_routing,'balanceNote':'积分为最近成功查询的记录，不是实时扣费余额；不同平台不合计。','retryNote':'到期时间是最早入队时间，不是完成承诺；失败受冷却与重试间隔约束。','recordLimit':150}
    return result

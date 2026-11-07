"""Scheduling metadata only; renewal decisions stay with the official plugin."""
import base64, json

def credential_meta(provider, secret):
    if provider != 'raccoon':
        return {'credentialRenewable': False, 'credentialExpiresAt': 0, 'credentialReason': '官方插件不支持续期'}
    token = next((secret.get(k) for k in ('accessToken','token','access_token') if isinstance(secret.get(k),str) and secret[k]), '')
    if token.lower().startswith('bearer '): token = token[7:].strip()
    claims = {}
    try:
        part=token.split('.')[1]; claims=json.loads(base64.urlsafe_b64decode(part+'='*(-len(part)%4)))
        if not isinstance(claims,dict): claims={}
    except (ValueError,IndexError,TypeError): pass
    refresh=next((secret.get(k) for k in ('refreshToken','refresh_token') if isinstance(secret.get(k),str) and secret[k].strip()), '')
    iam=str(claims.get('iss','')).endswith('/iam')
    expiry=0
    for value,seconds in [(secret.get('expiresAt'),False),(claims.get('exp'),True),(secret.get('tokenExpiresAt'),False)]:
        try:
            value=float(value)
            if value>0: expiry=int(value*1000 if seconds or value<100000000000 else value);break
        except (ValueError,TypeError,OverflowError): pass
    renewable=bool(token and refresh and not iam)
    return {'credentialRenewable':renewable,'credentialExpiresAt':expiry,'credentialReason': '支持官方续期' if renewable else 'IAM凭证不支持刷新令牌续期' if iam else '缺少可续期凭证'}

def balance_interval(row, config):
    return config.get('positiveInterval',config['interval']) if (row.get('balance') or 0)>0 else config['interval']

def credential_due(row, previous, timestamp, config):
    if not row.get('credentialRenewable'):return False
    expires=row.get('credentialExpiresAt',0)
    if expires and expires>timestamp+300000:return False
    interval=config.get('interval',300) if expires else max(3600,config.get('interval',300))
    return timestamp-previous>=interval*1000

def balance_satisfied(job,row,timestamp,config):
    success=row.get('lastSuccess',0)
    if not success:return False
    required=job.get('required_after',0)
    if required:return success>=required
    if row.get('signedAt',0)>success:return False
    return job.get('origin')=='auto' and timestamp-success<balance_interval(row,config)*1000


def balance_waits_for_checkin(row):
    """A confirmed zero leaves periodic polling until a newer successful checkin."""
    return (not row.get('unlimited') and row.get('balance') == 0
            and row.get('lastSuccess', 0) > 0
            and row.get('signedAt', 0) <= row.get('lastSuccess', 0))

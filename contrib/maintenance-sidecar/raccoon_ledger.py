"""Read-only Raccoon daily-grant verification; never calls the grant trigger."""
import datetime
import json
import math
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

TZ = datetime.timezone(datetime.timedelta(hours=8))
PAGE_SIZE = 100
MAX_PAGES = 100

def date_of(value):
    if not isinstance(value, str):
        return None
    try:
        parsed = datetime.datetime.fromisoformat(value.replace('Z', '+00:00'))
        if parsed.tzinfo is None:
            parsed = parsed.replace(tzinfo=TZ)
        return parsed.astimezone(TZ).date().isoformat()
    except ValueError:
        try:
            return datetime.date.fromisoformat(value[:10]).isoformat()
        except ValueError:
            return None

def daily_grant(item, day):
    kind = item.get('biz_type')
    if not (kind == 'daily_grant' or kind is None and item.get('event_name') == '每日积分发放'):
        return None
    points = item.get('points')
    if isinstance(points, bool):
        return None
    try:
        points = float(points)
    except (ValueError, TypeError):
        return None
    if not math.isfinite(points) or points <= 0 or date_of(item.get('grant_at') or item.get('created_at')) != day:
        return None
    return {k: item[k] for k in ('biz_type', 'event_name', 'points', 'grant_at', 'created_at', 'expire_at') if k in item}

def scan(get_page, day, max_pages=MAX_PAGES):
    datetime.date.fromisoformat(day)
    cursor = ''
    seen = set()
    records = 0
    reason = 'page_limit'
    pages = 0
    for page_no in range(1, max_pages + 1):
        data = get_page(cursor, PAGE_SIZE)
        if not isinstance(data, dict) or not isinstance(data.get('items'), list):
            raise ValueError('账单响应缺少 items，不能确认签到')
        items = data['items']
        if any(not isinstance(item, dict) for item in items):
            raise ValueError('账单条目格式错误，不能确认签到')
        pages = page_no
        for offset, item in enumerate(items):
            grant = daily_grant(item, day)
            if grant is not None:
                evidence = {'date': day, 'pages': pages, 'recordsScanned': records + offset + 1, 'grants': [grant], 'source': 'raccoon-points-bills', 'readOnly': True}
                return {'outcome': 'done', 'message': '账单确认今日已入账 +' + str(grant['points']) + ' 积分（未重复领取）', 'verifiedDate': day, 'evidence': evidence}
        records += len(items)
        # Billing order follows creation time, not grant_at (which may be backdated).
        dates = [date_of(item.get('created_at')) for item in items]
        if not items or any(date is not None and date < day for date in dates):
            reason = 'end_of_day'
            break
        paging = data.get('paging') or {}
        if not isinstance(paging, dict):
            paging = {}
        nxt = data.get('next_cursor') or data.get('cursor') or paging.get('next_cursor') or paging.get('cursor')
        more = data.get('has_more', paging.get('has_more'))
        if more is False:
            reason = 'end_of_history'
            break
        if not nxt:
            reason = 'missing_cursor' if more is True else 'end_of_history'
            break
        if not isinstance(nxt, str) or nxt in seen:
            reason = 'cursor_loop'
            break
        seen.add(nxt)
        cursor = nxt
    complete = reason in ('end_of_day', 'end_of_history')
    return {'outcome': 'inactive', 'message': ('当天账单核对未发现每日积分入账' if complete else '账单核对未完成，不能确认今日入账') + '（未重复领取）', 'verifiedDate': day, 'evidence': {'date': day, 'pages': pages, 'recordsScanned': records, 'grants': [], 'source': 'raccoon-points-bills', 'readOnly': True, 'complete': complete, 'reason': reason}}

def main():
    payload = json.load(sys.stdin)
    token = payload['token'].removeprefix('Bearer ').strip()
    if not token:
        raise ValueError('HTTP 401: 账户缺少可用凭证')
    identity = payload.get('identity') or ''
    headers = {'Accept': 'application/json', 'Authorization': 'Bearer ' + token}
    if identity and identity != 'personal':
        if not identity.isascii() or any(ord(c) < 32 or ord(c) == 127 for c in identity):
            raise ValueError('组织身份无效')
        headers['X-Org-Code'] = identity
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    deadline = time.monotonic() + min(170, max(10, payload.get('timeout', 120)))
    def get_page(cursor, limit):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError('账单核对超时，未确认成功')
        query = {'paging.limit': str(limit), 'type': 'all'}
        if cursor:
            query['cursor'] = cursor
        request = urllib.request.Request('https://xiaohuanxiong.com/api/web/points/v2/bills?' + urllib.parse.urlencode(query), headers=headers)
        with opener.open(request, timeout=min(25, remaining)) as response:
            result = json.load(response)
        if not isinstance(result, dict):
            raise ValueError('账单响应格式错误')
        code = result.get('code', 0)
        if code not in (0, None):
            if code in (401, '401'):
                raise ValueError('HTTP 401: 账单凭证无效')
            raise ValueError('上游账单返回非成功业务状态')
        return result.get('data', result)
    try:
        claim = scan(get_page, payload['date'])
        print(json.dumps({'claim': claim}, ensure_ascii=False))
    except urllib.error.HTTPError as error:
        print(json.dumps({'error': '账单核对 HTTP ' + str(error.code)}, ensure_ascii=False))
    except Exception as error:
        # Never emit request headers, tokens, proxy secrets or complete response bodies.
        message = str(error) if isinstance(error, (ValueError, TimeoutError)) else '账单核对连接失败：' + type(error).__name__
        print(json.dumps({'error': message}, ensure_ascii=False))

if __name__ == '__main__':
    main()

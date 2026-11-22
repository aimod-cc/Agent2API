"""Exercise the actual health policy using temporary state; never probe the network."""
import contextlib
import json
import pathlib
import sqlite3
import sys
import tempfile
import time
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from proxy_health import AutoProxyHealth, ProxyUnavailable


with tempfile.TemporaryDirectory() as tmp:
    @contextlib.contextmanager
    def db():
        c = sqlite3.connect(pathlib.Path(tmp) / 'fixture.db', timeout=3)
        c.row_factory = sqlite3.Row
        try:
            yield c
            c.commit()
        finally:
            c.close()

    with db() as c:
        c.executescript('''
            CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT);
            CREATE TABLE proxies(id TEXT PRIMARY KEY,label TEXT,secret TEXT,enabled INT);
            CREATE TABLE events(ts INT,kind TEXT,message TEXT);
        ''')
        for ident, enabled in [('a', 1), ('b', 1), ('manual-off', 0)]:
            c.execute('INSERT INTO proxies VALUES(?,?,?,?)', (ident, ident, '{}', enabled))

    config = {'providers': {}}
    h = AutoProxyHealth(db, json.loads, lambda: config)
    try:
        assert not h.policy()['enabled']
        h.save_policy({'enabled': True})
        h.on_result('a', 'failed')
        assert not next(x for x in h.rows() if x['id'] == 'a')['autoBlocked']
        h.on_result('a', 'timeout')
        assert next(x for x in h.rows() if x['id'] == 'a')['autoBlocked']
        assert [x['id'] for x in h.candidates('auto')] == ['b']
        assert not h.candidates('a')
        try:
            h.require_available('a')
            raise AssertionError('Quarantined fixed route was allowed')
        except ProxyUnavailable:
            pass
        h.require_available('direct')
        h.on_result('a', 'ok')
        assert next(x for x in h.rows() if x['id'] == 'a')['autoBlocked']
        h.on_result('a', 'ok')
        assert not next(x for x in h.rows() if x['id'] == 'a')['autoBlocked']
        h.toggle('a', False)
        h.on_result('a', 'ok')
        h.on_result('a', 'ok')
        assert 'a' not in h.available_routes()  # success never overrides manual off
        timestamp = int(time.time() * 1000)
        with patch.object(h, 'start', return_value={'queued': 1}) as start:
            h.tick(timestamp)
            with db() as c:
                assert not c.execute("SELECT 1 FROM proxy_health_state WHERE id='manual-off'").fetchone()
                c.execute("UPDATE proxy_health_state SET next_check_at=? WHERE id='b'", (timestamp - 1,))
            assert h.tick(timestamp) == 1
            assert start.call_args.args[0] == ['b']
        print(json.dumps({'quarantine': True, 'recovery': True, 'manual_off_preserved': True,
                          'fixed_route_blocked': True, 'staggered_schedule': True, 'real_probes': 0}))
    finally:
        h.pool.shutdown(wait=True)

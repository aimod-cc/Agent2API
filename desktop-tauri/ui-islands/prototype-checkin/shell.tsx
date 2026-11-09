import * as React from 'react'
import { Badge, BadgeDot, Button, NavItem, NavList, SegmentedControl } from '@ui'
import { loadCenter, loadRecords, store, subscribe } from '../src/islands/checkin-store'
import { CenterTab } from '../src/islands/checkin-center-tab'
import { ScheduleTab } from '../src/islands/checkin-schedule-tab'
import { MakeupTab } from '../src/islands/checkin-makeup-tab'
import { RecordsTab } from '../src/islands/checkin-records-tab'

/** 预览外壳：真实页面的容器结构（左侧主导航 + 顶部标签 + 页面区）。 */

type Tab = 'center' | 'schedule' | 'makeup' | 'records'

const APP_NAV = [
  { group: '概览', items: ['对话', '报表', '账号'] },
  { group: '转发链路', items: ['路由', '模型管理', 'Agent 配置', 'MCP & Skills', '网络代理', '网关 Key'] },
  { group: '诊断', items: ['请求日志', '日志', '自动维护'] },
  { group: '系统', items: ['插件', '设置'] },
]

function useStore(): void {
  const [, bump] = React.useReducer((n: number) => n + 1, 0)
  React.useEffect(() => subscribe(() => bump()), [])
}

export default function Shell() {
  useStore()
  const [tab, setTab] = React.useState<Tab>('center')

  React.useEffect(() => {
    if (!store.loaded) void loadCenter()
    if (!store.recordsLoaded) void loadRecords()
  }, [])

  const center = store.center

  return (
    <div className='proto-app'>
      <aside className='proto-app-side'>
        <div className='proto-brand'>
          <img src='/assets/app-icon-192.png' alt='' />
          <div><strong>Agent2API</strong><span>多提供商本地网关</span></div>
        </div>
        <div className='proto-app-nav'>
          {APP_NAV.map(group => <div className='proto-nav-group' key={group.group}>
            <div className='proto-nav-label'>{group.group}</div>
            {group.items.map(label => <div key={label} className='proto-app-item'>
              <span className='proto-app-dot' /><span>{label}</span>
            </div>)}
            {group.group === '概览' ? <div className='proto-app-item active'>
              <span className='proto-app-dot' /><span>签到中心</span>
            </div> : null}
          </div>)}
        </div>
        <div className='proto-side-foot'>
          <span className='proto-status-dot' />
          <div><strong>网关运行中</strong><span>127.0.0.1:3065</span></div>
        </div>
      </aside>

      <div className='proto-main'>
        <header className='proto-topbar'>
          <div className='proto-crumb'><span>Agent2API</span><b>/</b><strong>签到中心</strong></div>
          <div className='proto-top-actions'>
            <Badge variant='success' shape='tag'><BadgeDot />网关运行中</Badge>
            <Badge variant='outline' shape='tag'><BadgeDot />43 个账号可用</Badge>
          </div>
        </header>
        <main className='proto-content'>
          <div className='ck-page'>
            <div className='ck-tabs'>
              <SegmentedControl
                options={[
                  { value: 'center', label: '签到中心' },
                  { value: 'schedule', label: '定时签到' },
                  { value: 'makeup', label: '定时补签' },
                  { value: 'records', label: '签到记录' },
                ]}
                value={tab}
                onValueChange={value => setTab(value as Tab)}
                aria-label='签到中心标签页'
              />
              <span className='ck-tabs-spacer' />
              <Badge variant='success' shape='tag'><BadgeDot />自动执行已开启</Badge>
              <Button size='default' variant='ghost' type='button' className='ck-refresh'
                onClick={() => { void loadCenter(); void loadRecords() }}>刷新</Button>
            </div>

            {!store.loaded ? <div className='ck-loading'>正在读取签到数据…</div> : <>
              {tab === 'center' && center ? <CenterTab center={center} /> : null}
              {tab === 'schedule' && center ? <ScheduleTab center={center} /> : null}
              {tab === 'makeup' && center ? <MakeupTab center={center} /> : null}
              {tab === 'records' ? <RecordsTab /> : null}
            </>}
          </div>
        </main>
      </div>
    </div>
  )
}

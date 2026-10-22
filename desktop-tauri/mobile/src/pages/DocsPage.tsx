import { PageIntro, SectionHead, copyWithToast } from '../components/ui'
import { Icon } from '../components/Icon'
import { apiBase } from '../lib/servers'
import { t } from '../i18n'

/**
 * 接口文档。
 *
 * 一页看全，每行一个复制按钮。手机上最常见的用法是把地址复制到另一台设备
 * 的客户端里 —— 所以「复制」是这一页的主操作，说明文字反而要短。
 *
 * 三种协议并列展示、不做折叠：它们本来就是三选一，收起来只会多一次点击。
 */
export function DocsPage() {
  const origin = apiBase()
  const endpoints = [
    { name: 'Base URL', value: `${origin}/v1`, note: t('填进客户端的「API 地址 / Base URL」') },
    { name: 'OpenAI Chat Completions', value: `${origin}/v1/chat/completions`, note: t('多数兼容客户端的默认协议') },
    { name: 'OpenAI Responses', value: `${origin}/v1/responses`, note: t('Codex、新版 SDK') },
    { name: 'Anthropic Messages', value: `${origin}/v1/messages`, note: t('Claude Code / Anthropic SDK') },
    { name: '模型列表', value: `${origin}/v1/models`, note: t('不受鉴权限制，配 Key 前就能拉') },
  ]

  return (
    <div className='m-page'>
      <PageIntro />

      <div className='m-section' style={{ marginTop: 0 }}>
        <SectionHead title={t('接口地址')} />
        <div className='m-card'>
          {endpoints.map(item => (
            <div
              key={item.name}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 9,
                padding: '12px 14px',
                borderBottom: '1px solid var(--m-line)',
              }}
            >
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 12.5, fontWeight: 550 }}>{item.name}</div>
                <div
                  className='m-mono'
                  style={{
                    fontSize: 11.3,
                    color: 'var(--m-text-3)',
                    marginTop: 3,
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {item.value}
                </div>
                <div style={{ fontSize: 11, color: 'var(--m-text-3)', marginTop: 2 }}>{item.note}</div>
              </div>
              <button
                type='button'
                onClick={() => void copyWithToast(item.value, item.name)}
                aria-label={t('复制')}
                style={{ display: 'flex', border: 0, background: 'none', color: 'var(--m-primary)', padding: 4, flex: 'none' }}
              >
                <Icon name='copy' style={{ width: 16, height: 16 }} />
              </button>
            </div>
          ))}
        </div>
      </div>

      <div className='m-section'>
        <SectionHead title={t('鉴权')} />
        <div className='m-card m-card-pad'>
          <div style={{ fontSize: 12.5, lineHeight: 1.7, color: 'var(--m-text-2)' }}>
            {t('请求带 ')}
            <span className='m-mono' style={{ color: 'var(--m-text)' }}>
              Authorization: Bearer &lt;Key&gt;
            </span>
            {t(' 或 ')}
            <span className='m-mono' style={{ color: 'var(--m-text)' }}>
              x-api-key
            </span>
            {t('。用「网关 Key」页里任一启用的 Key；一把都没有时不鉴权。')}
          </div>
        </div>
      </div>

      <div className='m-section'>
        <SectionHead title={t('快速验证')} />
        <div className='m-card m-card-pad'>
          <div
            className='m-mono'
            style={{
              fontSize: 11.5,
              lineHeight: 1.75,
              color: 'var(--m-text-2)',
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-all',
            }}
          >
            {`curl ${origin}/v1/models \\\n  -H "Authorization: Bearer sk-local"`}
          </div>
          <button
            type='button'
            className='m-btn m-ghost'
            style={{ marginTop: 12 }}
            onClick={() =>
              void copyWithToast(
                `curl ${origin}/v1/models -H "Authorization: Bearer sk-local"`,
                t('curl 命令'),
              )
            }
          >
            <Icon name='copy' />
            {t('复制命令')}
          </button>
        </div>
      </div>

      <div className='m-section' style={{ paddingBottom: 8 }}>
        <div className='m-card m-card-pad' style={{ fontSize: 11.5, color: 'var(--m-text-3)', lineHeight: 1.7 }}>
          {t('手机端只读文档。完整的使用说明与部署指引在仓库的 docs/ 目录。')}
        </div>
      </div>
    </div>
  )
}

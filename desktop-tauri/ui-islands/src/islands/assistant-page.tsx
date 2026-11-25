/**
 * AI 助手页（侧边栏「智能」组的一级菜单）。
 *
 * ── 页面结构 ─────────────────────────────────────────────────
 * 平铺无外框双栏：左侧可收起的历史会话栏（窄屏抽屉化） + 右侧对话主区
 * （极简顶栏 / 居中空态或 740px 阅读列 / 隔离草稿的输入区）+ 右侧设置抽屉。
 */

import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import { Button, TooltipProvider } from '@ui'
import { t } from '../i18n'
import {
  DEFAULT_SYSTEM_PROMPT,
  callJson,
  gatewayBase,
  loadSession,
  loadSessions,
  newSessionId,
  toast,
  type AssistantConfig,
  type ChatMessage,
  type PermissionMode,
} from './assistant-state'
import {
  ASSISTANT_TOOLS,
  errorText,
  toolsPayload,
} from './assistant-tools'
import { selectedModel, unavailableModelMessage } from './assistant-models'
import { useAssistantModels } from './assistant-model-state'
import { useAssistantConfig } from './assistant-config-state'
import { titleFrom, useAssistantSessions } from './assistant-session-state'
import { AssistantComposer, type AssistantComposerHandle } from './assistant-composer'
import { useAssistantAttachments } from './assistant-attachments'
import { MessageList, type UiMessage } from './assistant-messages'
import { AssistantSettingsDrawer } from './assistant-settings-drawer'
import { AssistantEmpty, AssistantSidebar, AssistantTopbar } from './assistant-shell'
import { useAssistantMessageOperations } from './assistant-message-operations'
import { useAssistantChatLoop } from './assistant-chat-loop'
import { ArrowDownIcon } from './assistant-icons'
import { AssistantTitleSettings } from '../../../ui-shared/assistant-title-settings'
import { replayMessages } from './assistant-replay'
import { useModelApiKeys } from '../../../ui-shared/use-model-api-keys'
import { ModelApiKeyField } from '../../../ui-shared/model-api-key-field'
import { modelApiKeyLabel, type ModelApiKey } from '../../../ui-shared/model-api-key'

const SIDEBAR_COLLAPSED_STORAGE_KEY = 'wb.assistant.sidebarCollapsed'

function readSidebarCollapsed(): boolean {
  try { return window.localStorage.getItem(SIDEBAR_COLLAPSED_STORAGE_KEY) === '1' } catch { return false }
}

function writeSidebarCollapsed(next: boolean): void {
  try { window.localStorage.setItem(SIDEBAR_COLLAPSED_STORAGE_KEY, next ? '1' : '0') } catch { /* ignore */ }
}

function AssistantPage() {
  const { config, configRef, status, failed: loadingFailed, load: loadConfig, save: saveConfig } = useAssistantConfig()
  const fetchApiKeys = React.useCallback(() => callJson('GET', '/api/model-access/keys'), [])
  const keyState = useModelApiKeys(fetchApiKeys)
  const selectedKey = React.useMemo(
    () => keyState.keys.find(key => key.id === config?.apiKeyId),
    [keyState.keys, config?.apiKeyId],
  )
  const requestKeyRef = React.useRef<ModelApiKey | undefined>(undefined)
  const modelState = useAssistantModels(selectedKey?.key)
  const { models, ready: modelsReady, loading: modelsLoading, error: modelsError, refresh: refreshModels } = modelState

  const [ui, setUi] = React.useState<UiMessage[]>([])
  const [busy, setBusy] = React.useState(false)
  const [awaiting, setAwaiting] = React.useState(false)
  const [activeId, setActiveId] = React.useState<string>('')
  const [sidebarOpen, setSidebarOpen] = React.useState(false)
  const [sidebarCollapsed, setSidebarCollapsed] = React.useState<boolean>(() => readSidebarCollapsed())
  const [settingsOpen, setSettingsOpen] = React.useState(false)
  const [titleSettingsOpen, setTitleSettingsOpen] = React.useState(false)
  const [historyBusy, setHistoryBusy] = React.useState(false)
  const [showScrollBottom, setShowScrollBottom] = React.useState(false)

  const uiRef = React.useRef<UiMessage[]>([])
  const wireRef = React.useRef<ChatMessage[]>([])
  const scrollRef = React.useRef<HTMLDivElement | null>(null)
  const nearBottomRef = React.useRef(true)
  const composerRef = React.useRef<AssistantComposerHandle | null>(null)
  const loadedRef = React.useRef(false)
  const activeIdRef = React.useRef<string>('')
  const titleRef = React.useRef<string>('')
  const historyBusyRef = React.useRef(false)
  const requestModelRef = React.useRef<string>('')

  const { images, setImages, files, setFiles, addFiles, reading: readingAttachments } = useAssistantAttachments(activeIdRef)
  const imagesRef = React.useRef(images)
  imagesRef.current = images
  const filesRef = React.useRef(files)
  filesRef.current = files

  const { sessions, setSessions, persist, summarize, acceptLoaded, remove: removeSavedSession, clear: clearSavedSessions } = useAssistantSessions({
    config: configRef, messages: wireRef, ui: uiRef, activeId: activeIdRef, title: titleRef, apiKey: requestKeyRef,
  })
  const persistRef = React.useRef(persist)
  persistRef.current = persist
  const summarizeRef = React.useRef(summarize)
  summarizeRef.current = summarize

  const refreshModelOptions = React.useCallback(() => { void refreshModels().catch(() => {}) }, [refreshModels])
  const refreshKeyOptions = React.useCallback(() => { void keyState.refresh().catch(() => {}) }, [keyState.refresh])
  const clearComposerDraft = React.useCallback(() => { composerRef.current?.clear() }, [])

  const toolEnabled = React.useMemo(() => {
    const map: Record<string, boolean> = {}
    for (const tool of ASSISTANT_TOOLS) map[tool.id] = config?.tools?.[tool.id] ?? false
    return map
  }, [config?.tools])
  const enabledTools = React.useMemo(() => toolsPayload(toolEnabled), [toolEnabled])
  const enabledCount = React.useMemo(() => Object.values(toolEnabled).filter(Boolean).length, [toolEnabled])

  const chatLoop = useAssistantChatLoop({
    configRef, keyState, modelState, requestKeyRef, requestModelRef,
    activeIdRef, uiRef, wireRef, enabledTools, persistRef, summarizeRef,
    historyBusyRef, busy, awaiting, readingAttachments,
    setUi, setBusy, setAwaiting, setActiveId, setImages, setFiles,
    onClearDraft: clearComposerDraft,
  })
  const { send, stop, resolveConfirm, resetLoopState, flushSyncUI, isSending } = chatLoop

  /* 贴底跟踪（仅在接近底部时自动跟随，用户上翻阅读时不打断） */
  React.useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    if (ui.length === 0) {
      nearBottomRef.current = true
      setShowScrollBottom(false)
      return
    }
    if (nearBottomRef.current) el.scrollTop = el.scrollHeight
  }, [ui])

  const handleScroll = React.useCallback(() => {
    const el = scrollRef.current
    if (!el) return
    const distance = el.scrollHeight - el.scrollTop - el.clientHeight
    nearBottomRef.current = distance < 120
    const shouldShow = distance > 90
    setShowScrollBottom(prev => (prev === shouldShow ? prev : shouldShow))
  }, [])

  const scrollToBottom = React.useCallback(() => {
    const el = scrollRef.current
    if (!el) return
    nearBottomRef.current = true
    setShowScrollBottom(false)
    el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' })
  }, [])

  const activate = React.useCallback(async () => {
    void loadConfig()
    void refreshModels().catch(() => {})
    if (loadedRef.current) return
    loadedRef.current = true
    try {
      setSessions(await loadSessions())
    } catch (error) {
      toast(t('读取历史会话失败：{message}', { message: errorText(error) }), 'err')
    }
    if (!activeIdRef.current) {
      const id = newSessionId()
      activeIdRef.current = id
      titleRef.current = ''
      setActiveId(id)
    }
  }, [loadConfig, refreshModels, setSessions])

  React.useEffect(() => {
    const api = window as unknown as { wbAssistant?: { activate?: () => void } }
    api.wbAssistant = { activate: () => { void activate() } }
    void activate()
    return () => { delete api.wbAssistant }
  }, [activate])

  const currentModel = config?.model ?? null
  const currentOption = React.useMemo(() => selectedModel(models, currentModel), [models, currentModel])
  const modelSupportsImages = currentOption?.supportsImages === true
  const modelWarning = modelsError
    ? t('读取模型清单失败：{message}', { message: modelsError })
    : !modelsReady ? t('正在读取可用模型…')
    : currentModel && !currentOption ? unavailableModelMessage(currentModel) : ''
  const mode: PermissionMode = config?.mode ?? 'confirm'
  const modelNames = React.useMemo(() => models.map(item => item.name), [models])

  const resetToBlank = React.useCallback((id: string) => {
    resetLoopState()
    uiRef.current = []
    wireRef.current = []
    activeIdRef.current = id
    titleRef.current = ''
    setActiveId(id)
    setAwaiting(false)
    setBusy(false)
    setImages([])
    setFiles([])
    flushSyncUI()
  }, [flushSyncUI, resetLoopState, setFiles, setImages])

  const newChat = React.useCallback(() => {
    if (historyBusyRef.current || isSending()) return
    void persistRef.current(false)
    resetToBlank(newSessionId())
    setSidebarOpen(false)
    clearComposerDraft()
    composerRef.current?.focus()
  }, [clearComposerDraft, isSending, resetToBlank])

  const openSession = React.useCallback(async (id: string) => {
    if (busy || awaiting || isSending() || historyBusyRef.current) return
    if (id === activeIdRef.current) { setSidebarOpen(false); return }
    historyBusyRef.current = true
    setHistoryBusy(true)
    try {
      await persistRef.current(false)
      const detail = await loadSession(id)
      acceptLoaded(detail)
      resetLoopState()
      uiRef.current = replayMessages(detail.messages, detail.model)
      wireRef.current = detail.messages
      activeIdRef.current = detail.id
      titleRef.current = detail.title
      nearBottomRef.current = true
      setActiveId(detail.id)
      setImages([])
      setFiles([])
      clearComposerDraft()
      setAwaiting(false)
      setBusy(false)
      flushSyncUI()
      const currentCfg = configRef.current
      if (currentCfg && (detail.model || detail.reasoning)) {
        const nextModel = detail.model ?? currentCfg.model
        const nextReasoning = detail.reasoning
        if (nextModel !== currentCfg.model || nextReasoning !== currentCfg.reasoning) {
          saveConfig({ ...currentCfg, model: nextModel, reasoning: nextReasoning })
          refreshModelOptions()
        }
      }
    } catch (error) {
      toast(t('打开会话失败：{message}', { message: errorText(error) }), 'err')
    } finally {
      historyBusyRef.current = false
      setHistoryBusy(false)
    }
    setSidebarOpen(false)
  }, [acceptLoaded, awaiting, busy, clearComposerDraft, configRef, flushSyncUI, isSending, refreshModelOptions, resetLoopState, saveConfig, setFiles, setImages])

  const removeSession = React.useCallback(async (id: string) => {
    if (busy || awaiting || isSending() || historyBusyRef.current) return
    historyBusyRef.current = true
    setHistoryBusy(true)
    try {
      await removeSavedSession(id)
      if (id === activeIdRef.current) {
        resetToBlank(newSessionId())
        clearComposerDraft()
      }
    } catch (error) {
      toast(t('删除会话失败：{message}', { message: errorText(error) }), 'err')
    } finally {
      historyBusyRef.current = false
      setHistoryBusy(false)
    }
  }, [awaiting, busy, clearComposerDraft, isSending, removeSavedSession, resetToBlank])

  const clearHistory = React.useCallback(async () => {
    if (busy || awaiting || isSending() || historyBusyRef.current) return
    historyBusyRef.current = true
    setHistoryBusy(true)
    try {
      await clearSavedSessions()
      resetToBlank(newSessionId())
      clearComposerDraft()
    } catch (error) {
      toast(t('清空历史失败：{message}', { message: errorText(error) }), 'err')
    } finally {
      historyBusyRef.current = false
      setHistoryBusy(false)
    }
  }, [awaiting, busy, clearComposerDraft, clearSavedSessions, isSending, resetToBlank])

  const pickModel = React.useCallback((name: string) => {
    const current = configRef.current
    if (!name) return
    if (!current) {
      toast(t('助手设置没读出来，暂时存不了对话模型。'), 'err')
      return
    }
    saveConfig({ ...current, model: name })
  }, [configRef, saveConfig])

  const pickReasoning = React.useCallback((level: string | null) => {
    const current = configRef.current
    if (!current) {
      toast(t('助手设置没读出来，暂时存不了思考等级。'), 'err')
      return
    }
    saveConfig({ ...current, reasoning: level })
  }, [configRef, saveConfig])

  const pickMode = React.useCallback((next: PermissionMode) => {
    const current = configRef.current
    if (!current) return
    saveConfig({ ...current, mode: next }, next === 'full' ? t('已切换到完全访问') : t('已切换到变更前确认'))
  }, [configRef, saveConfig])

  const pickApiKey = React.useCallback((apiKeyId: string) => {
    const current = configRef.current
    if (!current || !apiKeyId || current.apiKeyId === apiKeyId) return
    saveConfig({ ...current, apiKeyId }, t('网关 Key 已切换'))
  }, [configRef, saveConfig])

  const toggleSidebarCollapse = React.useCallback(() => {
    setSidebarCollapsed(prev => {
      const next = !prev
      writeSidebarCollapsed(next)
      return next
    })
  }, [])

  const messageOperations = useAssistantMessageOperations({
    messages: wireRef, ui: uiRef, activeId: activeIdRef,
    busy: () => busy || awaiting || isSending() || historyBusyRef.current,
    setBusy: value => { historyBusyRef.current = value; setHistoryBusy(value) },
    model: () => configRef.current?.model ?? null,
    sync: flushSyncUI,
    persist: () => persistRef.current(),
    open: detail => {
      acceptLoaded(detail)
      resetLoopState()
      wireRef.current = detail.messages
      uiRef.current = replayMessages(detail.messages, detail.model)
      activeIdRef.current = detail.id
      titleRef.current = detail.title
      nearBottomRef.current = true
      setActiveId(detail.id)
      setSessions(previous => [detail, ...previous])
      setImages([])
      setFiles([])
      clearComposerDraft()
      flushSyncUI()
      setSidebarOpen(false)
    },
  })

  const messageActions = React.useMemo(() => ({
    ...messageOperations,
    disabled: busy || awaiting || historyBusy,
  }), [awaiting, busy, historyBusy, messageOperations])

  const handleOpenSession = React.useCallback((id: string) => { void openSession(id) }, [openSession])
  const handleRemoveSession = React.useCallback((id: string) => { void removeSession(id) }, [removeSession])
  const handleClearHistory = React.useCallback(() => { void clearHistory() }, [clearHistory])
  const handleOpenTitleSettings = React.useCallback(() => {
    setTitleSettingsOpen(true)
    refreshModelOptions()
  }, [refreshModelOptions])
  const handleRemoveImage = React.useCallback((index: number) => {
    setImages(prev => prev.filter((_, i) => i !== index))
  }, [setImages])
  const handleRemoveFile = React.useCallback((index: number) => {
    setFiles(prev => prev.filter((_, i) => i !== index))
  }, [setFiles])
  const handleComposerSend = React.useCallback((text: string) => {
    void send(text, imagesRef.current, filesRef.current)
  }, [send])
  const handleQuickSend = React.useCallback((text: string) => {
    void send(text, [], [])
  }, [send])
  const handleConfirmTool = React.useCallback((run: boolean) => {
    void resolveConfirm(run)
  }, [resolveConfirm])
  const handleToggleHistoryDrawer = React.useCallback(() => setSidebarOpen(open => !open), [])
  const handleOpenSettings = React.useCallback(() => setSettingsOpen(true), [])

  const activeTitle = titleRef.current || titleFrom(wireRef.current) || t('新对话')
  const empty = ui.length === 0
  const composerNode = (
    <AssistantComposer
      ref={composerRef}
      images={images}
      files={files}
      onAddFiles={addFiles}
      onRemoveImage={handleRemoveImage}
      onRemoveFile={handleRemoveFile}
      modelSupportsImages={modelSupportsImages}
      imageToolsEnabled={toolEnabled.edit_images === true}
      modelAvailable={modelsReady && Boolean(currentOption) && keyState.ready && Boolean(selectedKey)}
      modelsReady={modelsReady}
      modelsLoading={modelsLoading}
      modelWarning={modelWarning}
      onRefreshModels={refreshModelOptions}
      models={models}
      model={currentModel}
      onPickModel={pickModel}
      reasoning={config?.reasoning ?? null}
      onPickReasoning={pickReasoning}
      mode={mode}
      onModeChange={pickMode}
      busy={busy || readingAttachments}
      awaiting={awaiting || historyBusy}
      onSend={handleComposerSend}
      onStop={stop}
    />
  )

  return (
    <TooltipProvider delay={300}>
      <div className='asst-layout'>
        <AssistantSidebar
          open={sidebarOpen}
          onOpenChange={setSidebarOpen}
          collapsed={sidebarCollapsed}
          onToggleCollapse={toggleSidebarCollapse}
          sessions={sessions}
          activeId={activeId}
          disabled={busy || awaiting || historyBusy}
          settingsDisabled={!config}
          onNew={newChat}
          onOpen={handleOpenSession}
          onRemove={handleRemoveSession}
          onClear={handleClearHistory}
          onSettings={handleOpenTitleSettings}
        />

        <div className='asst-main'>
          <AssistantTopbar
            title={activeTitle}
            hasMessages={!empty}
            sidebarCollapsed={sidebarCollapsed}
            onExpandSidebar={toggleSidebarCollapse}
            onNewChat={newChat}
            keys={keyState.keys}
            selectedKeyId={config?.apiKeyId ?? null}
            keyLabel={selectedKey ? modelApiKeyLabel(selectedKey) : t('请先选择一把 API Key')}
            keysLoading={keyState.loading}
            onSelectKey={pickApiKey}
            onRefreshKeys={refreshKeyOptions}
            onHistory={handleToggleHistoryDrawer}
            onSettings={handleOpenSettings}
            disableNew={busy || awaiting || historyBusy}
            disableKeySwitch={!config || busy || awaiting}
          />

          {empty ? (
            <AssistantEmpty
              composer={composerNode}
              noTools={config !== null && enabledCount === 0}
              failed={loadingFailed}
              status={status}
              onSend={handleQuickSend}
            />
          ) : (
            <>
              <div className='asst-scroll' ref={scrollRef} onScroll={handleScroll}>
                <div className='asst-flow'>
                  <MessageList
                    messages={ui}
                    model={currentModel || ''}
                    actions={messageActions}
                    onConfirm={handleConfirmTool}
                  />
                </div>
              </div>

              {showScrollBottom ? (
                <div className='asst-scroll-bottom-wrap'>
                  <Button
                    type='button'
                    variant='outline'
                    className='asst-scroll-bottom-btn'
                    onClick={scrollToBottom}
                  >
                    <ArrowDownIcon size={12} />
                    <span>{t('回到底部')}</span>
                  </Button>
                </div>
              ) : null}

              {composerNode}
            </>
          )}
        </div>

        <AssistantTitleSettings
          open={titleSettingsOpen}
          onOpenChange={setTitleSettingsOpen}
          model={config?.titleModel ?? null}
          prompt={config?.titlePrompt ?? ''}
          models={modelNames}
          t={t}
          disabled={!config}
          onSave={(titleModel, titlePrompt) => {
            if (config) saveConfig({ ...config, titleModel, titlePrompt }, t('标题总结设置已保存'))
          }}
        />
        <AssistantSettingsDrawer
          open={settingsOpen}
          onOpenChange={setSettingsOpen}
          config={config}
          models={modelNames}
          modelAvailable={modelsReady && Boolean(currentOption)}
          modelsReady={modelsReady}
          modelWarning={modelWarning}
          onRefreshModels={refreshModelOptions}
          busy={busy}
          failed={loadingFailed}
          status={status}
          onSave={saveConfig}
          apiKeyField={
            <ModelApiKeyField
              {...keyState}
              value={config?.apiKeyId ?? null}
              disabled={!config || busy || awaiting}
              rememberSelection
              onChange={apiKeyId => { if (config) saveConfig({ ...config, apiKeyId }) }}
              onRefresh={refreshKeyOptions}
              t={t}
            />
          }
        />
      </div>
    </TooltipProvider>
  )
}

/* ─── 挂载：接管 index.html 里的 assistant 页 section ─── */

const PAGE_SELECTOR = '.page[data-page="assistant"]'

let root: ReturnType<typeof createRoot> | null = null

function mount(): void {
  if (root) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  section.replaceChildren()
  root = createRoot(section)
  root.render(<AssistantPage />)
  void gatewayBase().catch(() => {})
}

mountOnPage('assistant', PAGE_SELECTOR, mount)

export { DEFAULT_SYSTEM_PROMPT }

/* Agent2API · 「预览对话」的解析与按条渲染（请求 / 响应原文 → 结构化条目 / 对话气泡 HTML） */
/* global wbApp, wbI18n, wbMarkdown */

/**
 * 请求详情弹窗「预览对话」的解析与渲染逻辑：把后端 `GET
 * /api/stats/requests/raw` 给的两段**原文**（下游请求体、最终下发的响应）
 * 解析并还原成一轮对话。
 *
 * ── 为什么拆成「先解析成条目清单，再按可见窗口逐条渲染」──────────
 * 长上下文（如 Claude Code 200+ 条消息、数 MB 响应）下，一次性把全部消息
 * 转成 Markdown + HTML 再整体挂载会长时间阻塞主线程甚至导致界面崩溃。
 * 现在分成两步：
 *   1. `parseAsync(requestBody, responseBody, options)`：
 *      把请求拆成轻量的条目清单，把大型 SSE 响应按时间片异步扫描，避免锁死主线程；
 *   2. `renderItem(item, options)`：
 *      React 侧默认只渲染最后一部分条目，向上滚动时再按批补齐更早的条目；
 *      单条超长正文默认按安全字符上限截断，点开后再展开完整内容。
 *
 * 同时保留 `render(requestBody, responseBody)` 兼容既有调用方式。
 */
(() => {
  const { esc } = wbApp;

  const DEFAULT_BLOCK_LIMIT = 16000;
  const MARKDOWN_SAFE_LIMIT = 48000;
  const PRETTY_JSON_LIMIT = 65536;
  const SSE_YIELD_BUDGET_MS = 10;
  const SSE_CHECK_INTERVAL = 128;

  // ─── 小工具 ──────────────────────────────────

  /** 普通对象判断（JSON.parse 出来的可能是数组 / null / 标量） */
  function isPlainObject(value) {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
  }

  /** 安全截断：不在 UTF-16 代理对中间切断，避免 emoji / 补充平面字符乱码 */
  function sliceSafe(text, limit) {
    const source = String(text ?? '');
    if (limit <= 0 || source.length <= limit) return source;
    let end = limit;
    const prev = source.charCodeAt(end - 1);
    if (prev >= 0xd800 && prev <= 0xdbff && end < source.length) {
      const next = source.charCodeAt(end);
      if (next >= 0xdc00 && next <= 0xdfff) end -= 1;
    }
    return source.slice(0, end);
  }

  /** 按当前渲染预算裁剪一段文本，并把总字符数 / 是否截断反馈给调用方 */
  function clipText(raw, budget) {
    const source = String(raw ?? '');
    if (!source) return '';
    budget.totalChars += source.length;
    if (budget.full || !budget.limit || source.length <= budget.limit) {
      return source;
    }
    budget.truncated = true;
    return `${sliceSafe(source, budget.limit)}\n…`;
  }

  /**
   * 富文本 → HTML。优先交给 markdown.js；超过 Markdown 安全长度上限时直接退回
   * 转义纯文本，避免几十万字正则扫描拖慢主线程。
   */
  function richText(text) {
    const source = String(text ?? '');
    if (!source.trim()) return '';
    if (source.length <= MARKDOWN_SAFE_LIMIT) {
      try {
        const html = window.wbMarkdown?.render?.(source);
        if (typeof html === 'string' && html) return html;
      } catch {
        // 渲染器抛错不该带走整块预览，下面退回纯文本
      }
    }
    return `<div class="cv-plain">${esc(source)}</div>`;
  }

  /**
   * 图片地址白名单：只放行 `data:`（且必须声明为图片）与 http(s)。
   * 不做 decodeURIComponent，避免把编码字符还原成可执行协议。
   */
  function safeImgSrc(url) {
    const value = String(url ?? '').trim();
    return /^(data:image\/|https?:\/\/)/i.test(value) ? value : '';
  }

  /** 单张图片；地址不过白名单时返回空串（整段忽略，不渲染破图） */
  function imgHtml(url, alt) {
    const src = safeImgSrc(url);
    if (!src) return '';
    return `<img class="cv-img" src="${esc(src)}" alt="${esc(alt || wbI18n.t('图片'))}" loading="lazy">`;
  }

  /** `arguments` 常是 JSON 文本：体积可控时缩进展示，过大或解析不了则原样 */
  function prettyJson(text, allowLarge) {
    const source = String(text ?? '');
    if (!source.trim()) return '';
    if (!allowLarge && source.length > PRETTY_JSON_LIMIT) return source;
    try {
      return JSON.stringify(JSON.parse(source), null, 2);
    } catch {
      return source;
    }
  }

  function safeStringify(value, pretty) {
    if (value === null || value === undefined) return '';
    if (typeof value === 'string') return value;
    try {
      return pretty ? JSON.stringify(value, null, 2) : JSON.stringify(value);
    } catch {
      return '';
    }
  }

  // ─── 内容段（content 的两种形态）──────────────

  const newParts = () => ({ text: '', images: [], calls: [], results: [], thinking: [] });

  /**
   * 轻量判断一条请求消息是否含可展示内容，避免在拆分条目阶段跑 Markdown / JSON 缩进。
   */
  function contentHasVisible(content) {
    if (typeof content === 'string') return content.trim() !== '';
    if (!Array.isArray(content)) return false;
    for (const part of content) {
      if (typeof part === 'string' && part.trim() !== '') return true;
      if (!isPlainObject(part)) continue;
      const type = String(part.type || '');
      if (type === 'text' && typeof part.text === 'string' && part.text.trim() !== '') return true;
      if ((type === 'image_url' || type === 'input_image') && (part.image_url?.url ?? part.image_url ?? part.url)) return true;
      if (type === 'image') {
        const source = part.source;
        if ((source?.type === 'base64' && source.data && /^image\//.test(String(source.media_type || '')))
          || (source?.type === 'url' && source.url)) return true;
      }
      if (type === 'tool_use') return true;
      if (type === 'tool_result' && contentHasVisible(part.content)) return true;
      if (type === 'thinking' && typeof part.thinking === 'string' && part.thinking.trim() !== '') return true;
    }
    return false;
  }

  function hasMessageContent(message) {
    if (!isPlainObject(message)) return false;
    const role = String(message.role || 'user');
    if (role === 'assistant') {
      if (Array.isArray(message.tool_calls) && message.tool_calls.length > 0) return true;
      if (typeof message.reasoning_content === 'string' && message.reasoning_content.trim() !== '') return true;
    }
    return contentHasVisible(message.content);
  }

  function collectContent(content, out, budget) {
    if (typeof content === 'string') {
      if (content) out.text += (out.text ? '\n' : '') + content;
      return out;
    }
    if (!Array.isArray(content)) return out;
    for (const part of content) {
      if (typeof part === 'string') {
        if (part) out.text += (out.text ? '\n' : '') + part;
        continue;
      }
      if (!isPlainObject(part)) continue;
      const type = String(part.type || '');
      if (type === 'text' && typeof part.text === 'string') {
        if (part.text) out.text += (out.text ? '\n' : '') + part.text;
      } else if (type === 'image_url' || type === 'input_image') {
        const url = part.image_url?.url ?? part.image_url ?? part.url;
        if (url) out.images.push(String(url));
      } else if (type === 'image') {
        const source = part.source;
        if (source?.type === 'base64' && source.data && /^image\//.test(String(source.media_type || ''))) {
          out.images.push(`data:${source.media_type};base64,${source.data}`);
        } else if (source?.type === 'url' && source.url) {
          out.images.push(String(source.url));
        }
      } else if (type === 'tool_use') {
        const rawArgs = safeStringify(part.input ?? {}, false);
        const args = budget?.full || rawArgs.length <= PRETTY_JSON_LIMIT
          ? prettyJson(rawArgs, Boolean(budget?.full))
          : rawArgs;
        out.calls.push({ name: String(part.name || 'tool'), args });
      } else if (type === 'tool_result') {
        const result = collectContent(part.content, newParts(), budget);
        out.results.push({
          label: part.tool_use_id ? wbI18n.t('工具结果 · {id}', { id: part.tool_use_id }) : wbI18n.t('工具结果'),
          text: result.text,
          images: result.images,
        });
      } else if (type === 'thinking' && typeof part.thinking === 'string') {
        out.thinking.push(part.thinking);
      }
    }
    return out;
  }

  // ─── 请求侧：messages 数组 → 气泡 ─────────────

  function userInner(parts, budget) {
    const shownText = clipText(parts.text, budget);
    const text = shownText ? `<div class="cv-plain">${esc(shownText)}</div>` : '';
    const images = parts.images.map(url => imgHtml(url, wbI18n.t('请求图片'))).join('');
    return text + images;
  }

  function reasoningsHtml(reasoning, budget) {
    const raw = String(reasoning ?? '').trim();
    if (!raw) return '';
    const text = clipText(raw, budget);
    return `<details class="cv-details">${wbI18n.t('<summary>思考过程</summary>')}`
      + `<div class="cv-reasoning md-body">${richText(text)}</div></details>`;
  }

  function callHtml(call, budget) {
    const name = String(call?.name || 'tool');
    const args = clipText(String(call?.args ?? ''), budget);
    return '<div class="cv-toolcall">'
      + wbI18n.t('<div class="cv-toolcall-name">调用工具 <code class="md-code">{name}</code></div>', { name: esc(name) })
      + (args ? `<pre class="req-detail-pre cv-pre">${esc(args)}</pre>` : '')
      + '</div>';
  }

  function toolDetailsHtml(label, parts, budget) {
    const images = (parts.images || []).map(url => imgHtml(url, wbI18n.t('工具结果图片'))).join('');
    const shownText = clipText(parts.text, budget);
    const body = (shownText ? `<div class="cv-plain">${esc(shownText)}</div>` : '') + images;
    if (!body) return '';
    return `<details class="cv-tool"><summary>${label}</summary>${body}</details>`;
  }

  function assistantInner(parts, reasoning, budget) {
    const combinedThinking = [
      String(reasoning ?? '').trim(),
      ...(Array.isArray(parts.thinking) ? parts.thinking.map(item => String(item ?? '').trim()) : []),
    ].filter(Boolean).join('\n');
    const shownText = clipText(parts.text, budget);
    return reasoningsHtml(combinedThinking, budget)
      + richText(shownText)
      + parts.calls.map(call => callHtml(call, budget)).join('')
      + parts.results.map(result => toolDetailsHtml(
        esc(result.label),
        { text: result.text, images: result.images || [], calls: [] },
        budget,
      )).join('');
  }

  function messageHtml(message, budget) {
    if (!isPlainObject(message)) return '';
    const parts = collectContent(message.content, newParts(), budget);
    if (Array.isArray(message.tool_calls)) {
      for (const call of message.tool_calls) {
        const fn = isPlainObject(call?.function) ? call.function : {};
        const rawArgs = fn.arguments ?? call?.arguments ?? '';
        parts.calls.push({
          name: String(fn.name || call?.name || call?.type || 'tool'),
          args: prettyJson(rawArgs, Boolean(budget.full)),
        });
      }
    }
    const role = String(message.role || 'user');
    const reasoning = typeof message.reasoning_content === 'string' ? message.reasoning_content : '';

    if (role === 'system' || role === 'developer') {
      const shown = clipText(parts.text, budget);
      const body = richText(shown);
      if (!body) return '';
      return `<div class="cv-row sys"><div class="cv-msg sys md-body">${body}</div></div>`;
    }
    if (role === 'assistant') {
      const inner = assistantInner(parts, reasoning, budget);
      if (!inner) return '';
      return `<div class="cv-row"><div class="cv-msg assistant md-body">${inner}</div></div>`;
    }
    if (role === 'tool') {
      if (!parts.text && !parts.images.length) return '';
      const label = message.tool_call_id
        ? wbI18n.t('工具结果 · {id}', { id: esc(String(message.tool_call_id)) })
        : wbI18n.t('工具结果');
      return toolDetailsHtml(label, parts, budget);
    }
    // user（含带 tool_result 的 Anthropic user 消息）：把 tool_result 也展开成工具折叠块
    const hasResults = parts.results.length > 0;
    if (!parts.text && !parts.images.length && !hasResults) return '';
    let html = '';
    if (parts.text || parts.images.length) {
      html += `<div class="cv-row user"><div class="cv-msg user">${userInner(parts, budget)}</div></div>`;
    }
    if (hasResults) {
      html += parts.results.map(result => toolDetailsHtml(
        esc(result.label),
        { text: result.text, images: result.images || [], calls: [] },
        budget,
      )).join('');
    }
    return html;
  }

  function parseRequestItems(raw) {
    const source = String(raw ?? '');
    if (!source.trim()) return [];
    let body = null;
    try {
      body = JSON.parse(source);
    } catch {
      body = null;
    }
    if (isPlainObject(body) && Array.isArray(body.messages)) {
      const items = [];
      if (typeof body.system === 'string' && body.system.trim()) {
        items.push({ id: 'req-sys', kind: 'system', text: body.system });
      } else if (Array.isArray(body.system)) {
        const system = collectContent(body.system, newParts(), { full: true, totalChars: 0, truncated: false });
        if (system.text.trim()) items.push({ id: 'req-sys', kind: 'system', text: system.text });
      }
      for (let i = 0; i < body.messages.length; i += 1) {
        const msg = body.messages[i];
        if (hasMessageContent(msg)) {
          items.push({ id: `req-msg-${i}`, kind: 'message', message: msg });
        }
      }
      if (items.length > 0) return items;
    }
    if (isPlainObject(body) && body.input !== undefined) {
      const items = [];
      if (typeof body.instructions === 'string' && body.instructions.trim()) {
        items.push({ id: 'req-sys', kind: 'system', text: body.instructions });
      }
      if (typeof body.input === 'string' && body.input.trim()) {
        items.push({ id: 'req-in-0', kind: 'message', message: { role: 'user', content: body.input } });
      } else if (Array.isArray(body.input)) {
        for (let i = 0; i < body.input.length; i += 1) {
          const item = body.input[i];
          if (hasMessageContent(item)) {
            items.push({ id: `req-in-${i}`, kind: 'message', message: item });
          }
        }
      }
      if (items.length > 0) return items;
    }
    return [{
      id: 'req-raw',
      kind: 'raw-fallback',
      notice: wbI18n.t('无法解析请求体（不是含 messages 数组的 JSON）'),
      summary: wbI18n.t('请求原文'),
      rawText: source,
    }];
  }

  // ─── 响应侧：SSE / JSON → 助手气泡 ────────────

  /**
   * 仅扫描开头若干行判定是否为 SSE（支持 `: heartbeat` 注释与 `event:` 头行），
   * 避免对几 MB 文本做整段 split。
   */
  function looksSse(raw) {
    const source = String(raw ?? '');
    let pos = 0;
    let checked = 0;
    let sawEventHeader = false;
    while (pos < source.length && checked < 32) {
      let end = source.indexOf('\n', pos);
      if (end === -1) end = source.length;
      const line = source.slice(pos, end).trim();
      pos = end + 1;
      if (!line || line.startsWith(':')) continue;
      checked += 1;
      if (line.startsWith('data:')) return true;
      if (line.startsWith('event:') || line.startsWith('id:') || line.startsWith('retry:')) {
        sawEventHeader = true;
        continue;
      }
      return false;
    }
    return sawEventHeader && source.includes('\ndata:');
  }

  function createSseAccumulator() {
    return {
      content: '',
      reasoning: '',
      calls: new Map(),
      errorText: '',
    };
  }

  function consumeSseFrame(acc, frame) {
    if (!frame || frame === '[DONE]') return;
    let parsed = null;
    try {
      parsed = JSON.parse(frame);
    } catch {
      return;
    }
    if (!isPlainObject(parsed)) return;

    // 1. OpenAI Chat Completions SSE
    const choice = Array.isArray(parsed.choices) ? parsed.choices[0] : null;
    const delta = isPlainObject(choice) ? choice.delta : null;
    if (isPlainObject(delta)) {
      if (typeof delta.content === 'string') acc.content += delta.content;
      if (typeof delta.reasoning_content === 'string') acc.reasoning += delta.reasoning_content;
      if (typeof delta.reasoning === 'string') acc.reasoning += delta.reasoning;
      if (Array.isArray(delta.tool_calls)) {
        for (const piece of delta.tool_calls) {
          if (!isPlainObject(piece)) continue;
          const index = Number.isFinite(piece.index) ? piece.index : acc.calls.size;
          const entry = acc.calls.get(index) || { name: '', args: '' };
          const fn = isPlainObject(piece.function) ? piece.function : {};
          if (typeof fn.name === 'string' && fn.name) entry.name += fn.name;
          if (typeof fn.arguments === 'string') entry.args += fn.arguments;
          acc.calls.set(index, entry);
        }
      }
    }

    // 2. Anthropic Messages SSE
    const eventType = typeof parsed.type === 'string' ? parsed.type : '';
    if (eventType === 'content_block_start' && isPlainObject(parsed.content_block)) {
      const block = parsed.content_block;
      const idx = Number.isFinite(parsed.index) ? `anth-${parsed.index}` : `anth-${acc.calls.size}`;
      if (block.type === 'tool_use') {
        const entry = acc.calls.get(idx) || { name: '', args: '' };
        if (typeof block.name === 'string' && block.name) entry.name = block.name;
        if (isPlainObject(block.input) && Object.keys(block.input).length > 0 && !entry.args) {
          entry.args = safeStringify(block.input, false);
        }
        acc.calls.set(idx, entry);
      } else if (block.type === 'text' && typeof block.text === 'string' && block.text) {
        acc.content += block.text;
      } else if (block.type === 'thinking' && typeof block.thinking === 'string' && block.thinking) {
        acc.reasoning += block.thinking;
      }
    } else if (eventType === 'content_block_delta' && isPlainObject(parsed.delta)) {
      const blockDelta = parsed.delta;
      if (blockDelta.type === 'text_delta' && typeof blockDelta.text === 'string') {
        acc.content += blockDelta.text;
      } else if (blockDelta.type === 'thinking_delta' && typeof blockDelta.thinking === 'string') {
        acc.reasoning += blockDelta.thinking;
      } else if (blockDelta.type === 'input_json_delta' && typeof blockDelta.partial_json === 'string') {
        const idx = Number.isFinite(parsed.index) ? `anth-${parsed.index}` : `anth-${Math.max(0, acc.calls.size - 1)}`;
        const entry = acc.calls.get(idx) || { name: '', args: '' };
        entry.args += blockDelta.partial_json;
        acc.calls.set(idx, entry);
      }
    }

    // 3. OpenAI Responses SSE
    if (eventType === 'response.output_text.delta' && typeof parsed.delta === 'string') {
      acc.content += parsed.delta;
    } else if ((eventType === 'response.reasoning_summary_text.delta' || eventType === 'response.reasoning_text.delta')
      && typeof parsed.delta === 'string') {
      acc.reasoning += parsed.delta;
    } else if (eventType === 'response.output_item.added' && isPlainObject(parsed.item)) {
      const item = parsed.item;
      if (item.type === 'function_call' || item.type === 'custom_tool_call') {
        const key = String(item.id || item.call_id || `resp-${parsed.output_index ?? acc.calls.size}`);
        const entry = acc.calls.get(key) || { name: '', args: '' };
        if (typeof item.name === 'string' && item.name) entry.name = item.name;
        if (typeof item.arguments === 'string' && item.arguments && !entry.args) entry.args = item.arguments;
        acc.calls.set(key, entry);
      }
    } else if ((eventType === 'response.function_call_arguments.delta' || eventType === 'response.custom_tool_call_input.delta')
      && typeof parsed.delta === 'string') {
      const key = String(parsed.item_id || `resp-${parsed.output_index ?? Math.max(0, acc.calls.size - 1)}`);
      const entry = acc.calls.get(key) || { name: '', args: '' };
      entry.args += parsed.delta;
      acc.calls.set(key, entry);
    }

    // 上游错误帧
    if (parsed.error && !acc.errorText) {
      acc.errorText = String(isPlainObject(parsed.error) ? (parsed.error.message || safeStringify(parsed.error, false)) : parsed.error);
    } else if (eventType === 'response.failed' && isPlainObject(parsed.response?.error) && !acc.errorText) {
      acc.errorText = String(parsed.response.error.message || safeStringify(parsed.response.error, false));
    }
  }

  function finalizeSseAccumulator(acc) {
    const finalCalls = [...acc.calls.values()].map(call => ({
      name: call.name || 'tool',
      args: prettyJson(call.args, false),
    }));
    if (!acc.content && !acc.reasoning && !finalCalls.length && !acc.errorText) return null;
    return { text: acc.content, reasoning: acc.reasoning, calls: finalCalls, errorText: acc.errorText };
  }

  /** 游标式同步解析 SSE（不一次性 split 整段大字符串） */
  function parseSseResponse(raw) {
    const source = String(raw ?? '');
    const acc = createSseAccumulator();
    let pos = 0;
    let dataLines = [];
    while (pos < source.length) {
      let end = source.indexOf('\n', pos);
      if (end === -1) end = source.length;
      const line = source.slice(pos, end).replace(/\r$/, '');
      pos = end + 1;
      if (!line) {
        if (dataLines.length > 0) {
          consumeSseFrame(acc, dataLines.join('\n'));
          dataLines = [];
        }
        continue;
      }
      if (line.startsWith('data:')) {
        dataLines.push(line.slice(5).replace(/^ /, ''));
      }
    }
    if (dataLines.length > 0) consumeSseFrame(acc, dataLines.join('\n'));
    return finalizeSseAccumulator(acc);
  }

  /** 时间片异步解析 SSE：每 ~10ms 让出一次主线程，超大流式报文也不卡界面 */
  async function parseSseResponseAsync(raw, shouldCancel) {
    const source = String(raw ?? '');
    const acc = createSseAccumulator();
    let pos = 0;
    let dataLines = [];
    let framesSinceCheck = 0;
    let sliceStart = typeof performance !== 'undefined' ? performance.now() : Date.now();
    const now = () => (typeof performance !== 'undefined' ? performance.now() : Date.now());

    while (pos < source.length) {
      let end = source.indexOf('\n', pos);
      if (end === -1) end = source.length;
      const line = source.slice(pos, end).replace(/\r$/, '');
      pos = end + 1;
      if (!line) {
        if (dataLines.length > 0) {
          consumeSseFrame(acc, dataLines.join('\n'));
          dataLines = [];
          framesSinceCheck += 1;
        }
      } else if (line.startsWith('data:')) {
        dataLines.push(line.slice(5).replace(/^ /, ''));
      }

      if (framesSinceCheck >= SSE_CHECK_INTERVAL) {
        framesSinceCheck = 0;
        if (now() - sliceStart >= SSE_YIELD_BUDGET_MS) {
          await new Promise(resolve => setTimeout(resolve, 0));
          if (shouldCancel?.()) return null;
          sliceStart = now();
        }
      }
    }
    if (dataLines.length > 0) consumeSseFrame(acc, dataLines.join('\n'));
    return finalizeSseAccumulator(acc);
  }

  /** 非流式 JSON 响应 → 助手消息 */
  function parseJsonResponse(raw) {
    let body = null;
    try {
      body = JSON.parse(raw);
    } catch {
      return null;
    }
    if (!isPlainObject(body)) return null;
    if (body.error) {
      const message = isPlainObject(body.error) ? body.error.message : body.error;
      return { errorText: String(message ?? wbI18n.t('上游返回错误')) };
    }
    if (Array.isArray(body.output)) {
      const parts = newParts();
      let reasoning = '';
      for (const item of body.output) {
        if (!isPlainObject(item)) continue;
        if (item.type === 'reasoning' && Array.isArray(item.summary)) {
          for (const piece of item.summary) {
            if (isPlainObject(piece) && typeof piece.text === 'string' && piece.text) {
              reasoning += (reasoning ? '\n' : '') + piece.text;
            }
          }
        } else if (item.type === 'message' && Array.isArray(item.content)) {
          for (const piece of item.content) {
            if (isPlainObject(piece) && typeof piece.text === 'string' && piece.text) {
              parts.text += (parts.text ? '\n' : '') + piece.text;
            }
          }
        } else if (item.type === 'function_call' || item.type === 'custom_tool_call') {
          parts.calls.push({
            name: String(item.name || 'tool'),
            args: prettyJson(item.arguments ?? item.input ?? '', false),
          });
        }
      }
      if (!parts.text && typeof body.output_text === 'string') parts.text = body.output_text;
      if (!parts.text && !parts.calls.length && !reasoning) return null;
      return { text: parts.text, images: [], reasoning, calls: parts.calls, errorText: '' };
    }
    if (body.type === 'message' && Array.isArray(body.content)) {
      const parts = collectContent(body.content, newParts(), { full: false, totalChars: 0, truncated: false });
      const reasoning = Array.isArray(parts.thinking) ? parts.thinking.join('\n') : String(parts.thinking || '');
      if (!parts.text && !parts.images.length && !parts.calls.length && !reasoning) return null;
      return { text: parts.text, images: parts.images, reasoning, calls: parts.calls, errorText: '' };
    }
    const choice = Array.isArray(body.choices) ? body.choices[0] : null;
    const message = isPlainObject(choice) ? choice.message : null;
    if (!isPlainObject(message)) return null;
    const parts = collectContent(message.content, newParts(), { full: false, totalChars: 0, truncated: false });
    if (Array.isArray(message.tool_calls)) {
      for (const call of message.tool_calls) {
        const fn = isPlainObject(call?.function) ? call.function : {};
        parts.calls.push({ name: String(fn.name || 'tool'), args: prettyJson(fn.arguments ?? '', false) });
      }
    }
    const reasoning = typeof message.reasoning_content === 'string' ? message.reasoning_content : '';
    if (!parts.text && !parts.images.length && !parts.calls.length && !reasoning) return null;
    return { text: parts.text, images: parts.images, reasoning, calls: parts.calls, errorText: '' };
  }

  function responseResultHtml(result, budget) {
    if (result.errorText) {
      const shown = clipText(result.errorText, budget);
      return `<div class="cv-row sys"><div class="cv-msg sys cv-error md-body">${richText(shown)}</div></div>`;
    }
    const parts = {
      text: result.text || '',
      images: result.images || [],
      calls: result.calls || [],
      results: [],
      thinking: [],
    };
    if (!parts.text && !parts.images.length && !parts.calls.length && !result.reasoning) return '';
    return `<div class="cv-row"><div class="cv-msg assistant md-body">${assistantInner(parts, result.reasoning, budget)}</div></div>`;
  }

  function wrapResponseParsed(source, parsed) {
    if (parsed) {
      const hasAny = Boolean(
        parsed.errorText || parsed.text || parsed.reasoning
        || (Array.isArray(parsed.images) && parsed.images.length)
        || (Array.isArray(parsed.calls) && parsed.calls.length),
      );
      if (hasAny) return [{ id: 'res-msg', kind: 'response', result: parsed }];
    }
    return [{
      id: 'res-raw',
      kind: 'raw-fallback',
      notice: wbI18n.t('无法解析响应内容'),
      summary: wbI18n.t('响应原文'),
      rawText: source,
    }];
  }

  function buildConversationItems(request, response, requestItems, responseItems) {
    const items = [];
    if (requestItems.length > 0) {
      items.push(...requestItems);
    } else if (response.trim()) {
      items.push({ id: 'req-empty', kind: 'notice', text: wbI18n.t('无请求原文') });
    }
    if (responseItems.length > 0) {
      items.push(...responseItems);
    } else if (request.trim()) {
      items.push({ id: 'res-empty', kind: 'notice', text: wbI18n.t('无响应原文') });
    }
    return { items, totalItems: items.length };
  }

  // ─── 对外 API ────────────────────────────────

  /** 同步解析为条目清单 */
  function parse(requestBody, responseBody) {
    const request = String(requestBody ?? '');
    const response = String(responseBody ?? '');
    if (!request.trim() && !response.trim()) return { items: [], totalItems: 0 };
    const reqItems = parseRequestItems(request);
    const resItems = response.trim()
      ? wrapResponseParsed(response, looksSse(response) ? parseSseResponse(response) : parseJsonResponse(response))
      : [];
    return buildConversationItems(request, response, reqItems, resItems);
  }

  /** 异步分片解析为条目清单（大流式响应期间持续让出主线程） */
  async function parseAsync(requestBody, responseBody, options) {
    const shouldCancel = options?.shouldCancel;
    const request = String(requestBody ?? '');
    const response = String(responseBody ?? '');
    if (!request.trim() && !response.trim()) return { items: [], totalItems: 0 };

    const reqItems = parseRequestItems(request);
    if (shouldCancel?.()) return null;

    let resItems = [];
    if (response.trim()) {
      if (looksSse(response)) {
        const parsed = await parseSseResponseAsync(response, shouldCancel);
        if (shouldCancel?.()) return null;
        resItems = wrapResponseParsed(response, parsed);
      } else {
        if (response.length > 262144) {
          await new Promise(resolve => setTimeout(resolve, 0));
          if (shouldCancel?.()) return null;
        }
        resItems = wrapResponseParsed(response, parseJsonResponse(response));
      }
    }
    return buildConversationItems(request, response, reqItems, resItems);
  }

  /** 按需渲染单条对话；支持限制每段字符数，返回是否被截断及总字符数 */
  function renderItem(item, options) {
    const full = Boolean(options?.full);
    const limit = typeof options?.maxBlockChars === 'number' ? options.maxBlockChars : DEFAULT_BLOCK_LIMIT;
    const budget = { full, limit, totalChars: 0, truncated: false };
    if (!item || typeof item !== 'object') return { html: '', truncated: false, totalChars: 0 };

    if (item.kind === 'system') {
      const shown = clipText(item.text, budget);
      const body = richText(shown);
      return {
        html: body ? `<div class="cv-row sys"><div class="cv-msg sys md-body">${body}</div></div>` : '',
        truncated: budget.truncated,
        totalChars: budget.totalChars,
      };
    }
    if (item.kind === 'message') {
      return {
        html: messageHtml(item.message, budget),
        truncated: budget.truncated,
        totalChars: budget.totalChars,
      };
    }
    if (item.kind === 'response') {
      return {
        html: responseResultHtml(item.result, budget),
        truncated: budget.truncated,
        totalChars: budget.totalChars,
      };
    }
    if (item.kind === 'notice') {
      return {
        html: `<div class="cv-parse-empty">${esc(item.text || '')}</div>`,
        truncated: false,
        totalChars: String(item.text || '').length,
      };
    }
    if (item.kind === 'raw-fallback') {
      const shown = clipText(item.rawText, budget);
      const html = `<div class="cv-parse-empty">${esc(item.notice || '')}</div>`
        + `<details class="cv-details"><summary>${esc(item.summary || '')}</summary>`
        + `<pre class="req-detail-pre">${esc(shown)}</pre></details>`;
      return { html, truncated: budget.truncated, totalChars: budget.totalChars };
    }
    return { html: '', truncated: false, totalChars: 0 };
  }

  /** 兼容既有整段渲染入口 */
  function render(requestBody, responseBody) {
    const parsed = parse(requestBody, responseBody);
    if (!parsed.items.length) return '';
    const blocks = parsed.items.map(item => renderItem(item, { full: true }).html).filter(Boolean);
    return blocks.length ? `<div class="cv">${blocks.join('')}</div>` : '';
  }

  window.wbConversationPreview = { render, parse, parseAsync, renderItem };
})();

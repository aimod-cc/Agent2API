// Run with the path to a freshly built agent2api-server executable. Both listeners are loopback-only.
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { once } from 'node:events'
import { mkdtemp, rm } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'

assert(process.argv[2], 'Usage: node scripts/test-privacy-masking.mjs <agent2api-server>')
const key = randomUUID()
const upstreamKey = randomUUID()
const sentinel = 'never-send-' + randomUUID()
const fakeKey = ['sk', 'proj', 'FixtureCredential123456789'].join('-')
const email = 'fixture@relay.internal'
const password = 'fake"quoted\\value'
const content = [sentinel, fakeKey, email, fakeKey].join(' ')
const seen = []
let foreign = ''
let echoPrompt = false
let echoToolResult = false
let compactBudgetReply = false
let reviewMode = ''
const toolHistoryId = 'call_local'
const encodedWord = 'EncodedMediaFixture0'
const lookupId = ['736a9b08', '170a', '4b11', '97ef', '02329fbd984a'].join('-')
const lookupDigest = ['5d41402a', 'bc4b2a76'].join('')
const elementToken = 's0000002a:14'
const toolResult = {
  Authorization: 'TestRealmFixture52Y', Cookie: 'fixture-session=TestCookie54J',
  key: 'ExampleLocalFixture3A', pass: 'ExampleLocalFixture4B',
  access_token_expires_at: '2030-12-30T16:48:25+08:00',
  env: { api_token: ['$', '{', 'FIXTURE_AUTH_TOKEN', '}'].join('') },
  media: 'data:application/octet-stream;base64,' + encodedWord,
  rows: 2, nextPageToken: lookupId, secret_hash: lookupDigest, element_token: elementToken,
  sessionToken: lookupId,
}
const toolSecretKeys = ['Authorization', 'Cookie', 'key', 'pass', 'sessionToken']
let holdReply
const promptOnly = 'prompt-only-' + randomUUID()
const reviewFault = process.env.AGENT2API_PRIVACY_TEST_NEGATIVE === '1'
const syntheticPhone = '+86' + ['139', '0000', '9931'].join('')
const paths = { chat_completions: '/v1/chat/completions', responses: '/v1/responses', anthropic: '/v1/messages' }
const textPart = text => ({ type: 'text', text })
const message = (role, content) => ({ role, content })
const frame = (kind, body) => (kind ? 'event: ' + kind + '\r\n' : '') + 'data: ' + JSON.stringify(body) + '\r\n\r\n'
const calls = value => [{ index: 0, id: 'call_local', type: 'function', function: { name: 'echo', arguments: value } }]
const args = value => JSON.stringify({ password: value })
const cut = value => {
  const tokenStart = value.indexOf('{{A2A_')
  const at = tokenStart < 0 ? 13 : tokenStart + 13
  return [value.slice(0, at), value.slice(at)]
}

const upstream = createServer(async (req, res) => {
  try {
    const protocol = Object.keys(paths).find(p => req.url === '/' + p + paths[p])
    if (req.method !== 'POST' || !protocol) { res.writeHead(404).end(); return }
    const chunks = []
    for await (const chunk of req) chunks.push(chunk)
    const body = JSON.parse(Buffer.concat(chunks))
    const messages = protocol === 'responses' ? body.input : body.messages
    const value = messages.find(m => m.role === 'user')?.content
    const text = typeof value === 'string' ? value : value?.find(x => x.type === 'text' || x.type === 'input_text')?.text
    const tool = ['title','vision'].includes(reviewMode) ? { password: 'fixture' } : protocol === 'anthropic' ? messages.flatMap(m => Array.isArray(m.content) ? m.content : []).find(part => part.type === 'tool_use').input
      : JSON.parse(protocol === 'responses' ? messages.find(m => m.type === 'function_call').arguments
        : messages.find(m => m.tool_calls?.length).tool_calls[0].function.arguments)
    assert.equal(typeof text, 'string')
    const actualAuth = protocol === 'anthropic' ? req.headers['x-api-key'] : req.headers.authorization
    const prompt = protocol === 'responses' ? body.instructions : messages.filter(m => m.role === 'system' || m.role === 'developer').map(m => m.content).join(' ')
    seen.push({ protocol, body, text, tool, actualAuth, prompt })
    if (holdReply) await holdReply
    const history = protocol === 'chat_completions' ? messages.find(m => m.role === 'tool')
      : protocol === 'responses' ? messages.find(m => m.type === 'function_call_output')
        : messages.flatMap(m => Array.isArray(m.content) ? m.content : []).find(part => part.type === 'tool_result')
    const toolOutput = protocol === 'responses' ? history?.output : history?.content
    const resultText = typeof toolOutput === 'string' ? toolOutput : toolOutput?.find(part => part.type === 'text' || part.type === 'input_text')?.text
    const reply = reviewMode === 'structured' ? JSON.stringify({ value: text }) : compactBudgetReply ? 'budget accepted' : echoToolResult ? resultText : (foreign ? foreign + ' | ' : '') + text + (echoPrompt ? ' | ' + prompt : '')
    const msg = { id: 'msg_local', type: 'message', role: 'assistant', model: body.model,
      content: [textPart(reply), { type: 'tool_use', name: 'echo', id: 'call_local', input: tool }],
      stop_reason: 'tool_use', stop_sequence: null, usage: { input_tokens: 3, output_tokens: 2 } }
    const response = { id: 'resp_local', object: 'response', model: body.model, status: 'completed',
      instructions: body.instructions ?? null, tools: body.tools ?? [],
      output: [{ type: 'message', role: 'assistant', id: 'msg_local', content: [{ type: 'output_text', text: reply, annotations: [] }] },
        { type: 'function_call', name: 'echo', call_id: 'call_local', id: 'fc_local', arguments: args(tool.password) }],
      usage: { input_tokens: 3, output_tokens: 2, total_tokens: 5 } }
    const chat = (object, choice) => ({ id: 'chatcmpl_local', object, model: body.model, created: 1, choices: [choice] })
    if (['title','vision'].includes(reviewMode)) {
      const chunk = delta => frame(null, chat('chat.completion.chunk', { index: 0, delta, finish_reason: null }))
      if (body.stream) {
        res.writeHead(200, { 'content-type': 'text/event-stream' }).end(chunk({ content: text }) + frame(null, chat('chat.completion.chunk', { index: 0, delta: {}, finish_reason: 'stop' })) + 'data: [DONE]' + String.fromCharCode(10,10))
      } else {
        res.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify(chat('chat.completion', { index: 0, message: message('assistant', text), finish_reason: 'stop' })))
      }
      return
    }
    if (reviewMode === 'mcp-events') {
      const documents = text.split(' | ').map(value => JSON.stringify({ value }))
      assert.equal(documents.length, 2)
      const pieces = documents.map(cut)
      const event = (type, fields) => frame(type, { type, ...fields })
      let wire = ''
      for (const part of [0,1]) for (const output_index of [0,1]) {
        wire += event('response.mcp_call_arguments.delta', { output_index, item_id: 'mcp_' + output_index, delta: pieces[output_index][part] })
      }
      for (const output_index of [0,1]) wire += event('response.mcp_call_arguments.done', { output_index, item_id: 'mcp_' + output_index, arguments: documents[output_index] })
      wire += event('response.completed', { response: { ...response, output: [] } })
      res.writeHead(200, { 'content-type': 'text/event-stream' }).end(wire)
      return
    }
    if (!body.stream) {
      res.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify(
        protocol === 'anthropic' ? msg : protocol === 'responses' ? response
          : chat('chat.completion', { index: 0, message: { ...message('assistant', reply), ...(reviewMode === 'structured' ? { reasoning_content: 'reasoning ' + text } : {}), tool_calls: calls(args(tool.password)) }, finish_reason: 'tool_calls' })))
      return
    }
    const pieces = cut(reply)
    const argPieces = cut(args(tool.password))
    let wire
    if (protocol === 'chat_completions') {
      const delta = d => frame(null, chat('chat.completion.chunk', { index: 0, delta: d, finish_reason: null }))
      wire = pieces.map(t => delta({ content: t })).join('')
        + argPieces.map(a => delta({ tool_calls: calls(a) })).join('')
        + frame(null, chat('chat.completion.chunk', { index: 0, delta: {}, finish_reason: 'tool_calls' })) + 'data: [DONE]\r\n\r\n'
    } else if (protocol === 'anthropic') {
      const event = (type, fields) => frame(type, { type, ...fields })
      wire = event('message_start', { message: { ...msg, content: [], stop_reason: null } })
        + event('content_block_start', { index: 0, content_block: textPart('') })
        + pieces.map(text => event('content_block_delta', { index: 0, delta: { type: 'text_delta', text } })).join('')
        + event('content_block_stop', { index: 0 })
        + event('content_block_start', { index: 1, content_block: { ...msg.content[1], input: {} } })
        + argPieces.map(partial_json => event('content_block_delta', { index: 1, delta: { type: 'input_json_delta', partial_json } })).join('')
        + event('content_block_stop', { index: 1 }) + event('message_stop', {})
    } else {
      const event = (type, fields) => frame(type, { type, ...fields })
      wire = event('response.created', { response: { ...response, output: [], status: 'in_progress' } })
        + pieces.map(delta => event('response.output_text.delta', { item_id: 'msg_local', output_index: 0, content_index: 0, delta })).join('')
        + argPieces.map(delta => event('response.function_call_arguments.delta', { item_id: 'fc_local', output_index: 1, delta })).join('')
        + event('response.completed', { response })
    }
    if (reviewMode === 'bom') wire = String.fromCodePoint(0xfeff) + wire
    if (reviewMode === 'lone-cr') wire = wire.replaceAll('\r\n', '\r')
    if (reviewMode === 'wait-cap') {
      const delta = text => frame(null, chat('chat.completion.chunk', { index: 0, delta: { content: text }, finish_reason: null }))
      wire = delta(pieces[0]) + ': keepalive\r\n\r\n'.repeat(64) + delta(pieces[1]) + 'data: [DONE]\r\n\r\n'
    }
    res.writeHead(200, { 'content-type': 'text/event-stream' })
    const bytes = Buffer.from(wire)
    for (let at = 0; at < bytes.length; at += 113) { res.write(bytes.subarray(at, at + 113)); await sleep(1) }
    res.end()
  } catch (error) {
    console.error('Loopback fixture:', req.url, error.message)
    res.writeHead(500).end('loopback upstream fixture failed')
  }
})

async function listening(server) {
  server.listen(0, '127.0.0.1')
  await once(server, 'listening')
  return server.address().port
}
function end(server) { return new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve())) }
const home = await mkdtemp(join(tmpdir(), 'agent2api-privacy-test-'))
let child, output = ''
try {
  const upstreamUrl = 'http://127.0.0.1:' + await listening(upstream)
  const reserve = createServer()
  const port = await listening(reserve)
  await end(reserve)
  const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !/^(AGENT2API_|WORKBUDDY_)/i.test(name)))
  child = spawn(resolve(process.argv[2]), [], { windowsHide: true, env: { ...env,
    AGENT2API_HOST: '127.0.0.1', AGENT2API_PROXY_PORT: String(port), AGENT2API_PROXY_HOME: home,
    AGENT2API_PROXY_API_KEY: key, AGENT2API_CAPTCHA_ENABLED: '0', RUST_MIN_STACK: String(16 * 1024 * 1024), NO_PROXY: '127.0.0.1,localhost', no_proxy: '127.0.0.1,localhost' },
    stdio: ['ignore', 'pipe', 'pipe'] })
  child.stdout.on('data', x => { output = (output + x).slice(-8000) })
  child.stderr.on('data', x => { output = (output + x).slice(-8000) })
  let processError
  child.on('error', error => { processError = error })
  const call = (path, method = 'GET', payload, authorized = true, extra = {}) => fetch('http://127.0.0.1:' + port + path, {
    method, headers: { 'content-type': 'application/json', ...(authorized ? { 'x-api-key': key } : {}), ...extra },
    body: payload === undefined ? undefined : JSON.stringify(payload), signal: AbortSignal.timeout(10000),
  })
  const json = async (...values) => {
    const result = await call(...values), text = await result.text()
    assert(result.ok, values[0] + ' returned ' + result.status + ': ' + text.slice(0, 600))
    return JSON.parse(text)
  }
  let ready = false
  for (let i = 0; i < 100 && !ready; i++) {
    if (processError) throw processError
    if (child.exitCode !== null) throw new Error('Gateway exited ' + child.exitCode)
    ready = await call('/health').then(r => r.ok, () => false)
    if (!ready) await sleep(100)
  }
  assert(ready, 'gateway was not healthy')
  const endpoint = '/api/privacy-masking'
  const settingsOf = ({ revision, ...policy }) => policy
  const savePrivacy = async policy => {
    const latest = policy.revision === undefined ? (await json(endpoint)).data.revision : policy.revision
    return json(endpoint, 'PUT', { ...policy, ...(latest === undefined ? {} : { revision: latest }) })
  }
  assert.equal((await call(endpoint, 'GET', undefined, false)).status, 401)
  const disabled = { secrets: false, personal: false, terms: [] }
  assert.deepEqual(settingsOf((await json(endpoint)).data), disabled)
  await json('/api/sanitize', 'PUT', { sanitizeBlacklistFingerprints: false })
  const providerIds = {}
  for (const protocol of Object.keys(paths)) {
    const baseUrl = upstreamUrl + '/' + protocol + (protocol === 'anthropic' ? '' : '/v1')
    const id = (await json('/api/custom-providers', 'POST', { name: 'privacy ' + protocol, baseUrl, protocol, apiKey: upstreamKey })).data.provider.id
    providerIds[protocol] = id
    await json('/api/custom-providers/models', 'POST', { providerId: id, models: [{ id: 'privacy-' + protocol, enabled: true }], mappings: [] })
  }
  function body(protocol, stream) {
    const model = 'privacy-' + protocol
    if (protocol === 'responses') return { model, stream, input: [
      message('user', content), { type: 'function_call', id: 'fc_local', name: 'echo', call_id: 'call_local', arguments: args(password) },
      { type: 'function_call_output', call_id: 'call_local', output: 'ok' },
    ] }
    if (protocol === 'anthropic') return { model, stream, max_tokens: 128,
      messages: [message('user', content), message('assistant', [{ type: 'tool_use', name: 'echo', id: 'call_local', input: { password } }]),
        message('user', [{ type: 'tool_result', tool_use_id: 'call_local', content: 'ok' }])] }
    return { model, stream, messages: [message('user', content), { ...message('assistant', null), tool_calls: calls(args(password)) },
      { ...message('tool', 'ok'), tool_call_id: 'call_local' }] }
  }
  function checkUpstream(captured) {
    assert(!captured.text.includes(sentinel) && !captured.text.includes(fakeKey) && !captured.text.includes(email), 'unmasked upstream text')
    const tokens = captured.text.match(/\{\{A2A_[0-9a-f]{32}\}\}/g)
    assert.equal(tokens?.length, 4)
    assert.equal(tokens[1], tokens[3])
    assert.match(captured.tool.password, /^\{\{A2A_[0-9a-f]{32}\}\}$/)
    assert.equal(captured.actualAuth, captured.protocol === 'anthropic' ? upstreamKey : 'Bearer ' + upstreamKey)
    return tokens[0]
  }
  const read = async (protocol, stream) => {
    const r = await call(paths[protocol], 'POST', body(protocol, stream))
    const data = await r.text()
    assert(r.ok, protocol + ' returned ' + r.status + ': ' + data.slice(0, 700))
    if (!stream) {
      const value = JSON.parse(data)
      if (protocol === 'chat_completions') return [value.choices[0].message.content, JSON.parse(value.choices[0].message.tool_calls[0].function.arguments).password]
      if (protocol === 'anthropic') return [value.content[0].text, value.content[1].input.password]
      return [value.output[0].content[0].text, JSON.parse(value.output[1].arguments).password]
    }
    assert.match(r.headers.get('content-type'), /text\/event-stream/)
    const frames = data.split(/\r?\n/).filter(l => l.startsWith('data:')).map(l => l.slice(5).trim()).filter(l => l && l !== '[DONE]').map(JSON.parse)
    if (protocol === 'chat_completions') {
      assert(data.includes('data: [DONE]'))
      return [frames.map(x => x.choices?.[0]?.delta?.content ?? '').join(''), JSON.parse(frames.map(x => x.choices?.[0]?.delta?.tool_calls?.[0]?.function?.arguments ?? '').join('')).password]
    }
    if (protocol === 'anthropic') {
      assert(data.includes('event: message_stop'))
      return [frames.map(x => x.delta?.text ?? '').join(''), JSON.parse(frames.map(x => x.delta?.partial_json ?? '').join('')).password]
    }
    const complete = frames.find(x => x.type === 'response.completed').response
    const tool = frames.filter(x => x.type === 'response.function_call_arguments.delta').map(x => x.delta).join('')
    assert.equal(JSON.parse(complete.output[1].arguments).password, password)
    return [frames.filter(x => x.type === 'response.output_text.delta').map(x => x.delta).join(''), JSON.parse(tool).password]
  }
  for (const stream of [false, true]) assert.deepEqual(await read('chat_completions', stream), [content, password])
  assert.equal(seen.at(-1).text, content)
  assert.throws(() => checkUpstream(seen.at(-1)), /unmasked upstream text/, 'privacy assertion must fail with masking disabled')
  const enabled = { secrets: true, personal: true, terms: [sentinel] }
  assert.equal((await call(endpoint, 'PUT', enabled, false)).status, 401)
  assert.equal((await call(endpoint, 'PUT', { secrets: 'wrong', revision: (await json(endpoint)).data.revision })).status, 400)
  assert.deepEqual(settingsOf((await savePrivacy(enabled)).data), enabled)
  for (const protocol of Object.keys(paths)) {
    for (const stream of [false, true]) {
      assert.deepEqual(await read(protocol, stream), [content, password])
      checkUpstream(seen.at(-1))
    }
    console.log('PASS', protocol, 'JSON and SSE, masked wire and restored text/tools')
  }
  foreign = checkUpstream(seen.at(-1))
  assert.deepEqual(await read('chat_completions', false), [foreign + ' | ' + content, password])
  assert.notEqual(checkUpstream(seen.at(-1)), foreign)
  foreign = ''
  if (reviewFault) {
    await savePrivacy(disabled)
    await read('chat_completions', false)
    checkUpstream(seen.at(-1)) // The real wire assertion must turn red when masking is disabled.
  }
  const before = seen.length
  for (const state of [{ previous_response_id: 'resp_fixture' }, { conversation: { id: 'conv_fixture' } }]) {
    const blocked = await call(paths.responses, 'POST', { ...body('responses', false), ...state })
    assert.equal(blocked.status, 400)
    assert.equal((await blocked.json()).error.code, 'privacy_masking_stateful_responses')
  }
  assert.equal(seen.length, before, 'stateful requests must never reach the mock upstream')
  const terms100 = [...Array.from({ length: 99 }, (_, i) => 'privacy-case-' + i.toString().padStart(3, '0')), sentinel]
  const normalized = (await savePrivacy({ ...enabled, terms: ['', ' ', ...terms100, '  ' + sentinel + '  ', ''] })).data
  assert.equal(normalized.terms.length, 100)
  assert.deepEqual(new Set(normalized.terms), new Set(terms100))
  assert.equal((await call(endpoint, 'PUT', { ...enabled, terms: [...terms100, 'one-too-many'], revision: (await json(endpoint)).data.revision })).status, 400)
  await savePrivacy({ ...enabled, terms: [sentinel, promptOnly] })
  await json('/api/prompt', 'PUT', { promptMode: 'append', promptText: promptOnly })
  echoPrompt = true
  for (const protocol of ['chat_completions', 'responses']) {
    const [reply, restoredPassword] = await read(protocol, false)
    assert.equal(reply, content + ' | ' + promptOnly, 'gateway-injected prompt must be restored')
    assert.equal(restoredPassword, password)
    assert.match(seen.at(-1).prompt, /^\{\{A2A_[0-9a-f]{32}\}\}$/)
    checkUpstream(seen.at(-1))
  }
  echoPrompt = false
  await json('/api/prompt', 'PUT', { promptMode: 'passthrough', promptText: '' })
  const sample = '请联系手机' + syntheticPhone
  const phoneBody = body('chat_completions', false)
  phoneBody.messages[0].content = sample
  const phoneReply = await json(paths.chat_completions, 'POST', phoneBody)
  assert.equal(phoneReply.choices[0].message.content, sample)
  assert(!seen.at(-1).text.includes(syntheticPhone), '+86 without separator was forwarded unmasked')
  for (const protocol of Object.keys(paths)) {
    const oldCount = seen.length
    let release
    holdReply = new Promise(resolve => { release = resolve })
    const [first, second] = await (async () => {
      const observed = promise => promise.then(value => ({ ok: true, value }), error => ({ ok: false, error }))
      const a = observed(read(protocol, false))
      let b
      try {
        for (let i = 0; i < 60 && seen.length === oldCount; i++) await sleep(25)
        assert.equal(seen.length, oldCount + 1)
        b = observed(read(protocol, false))
        await sleep(250)
        assert.equal(seen.length, oldCount + 1, 'unchanged original bodies must wait in the same dedupe slot: ' + protocol)
      } finally { holdReply = undefined; release() }
      const results = await Promise.all([a, b])
      for (const result of results) if (!result.ok) throw result.error
      return results.map(result => result.value)
    })()
    assert.deepEqual(first, [content, password]); assert.deepEqual(second, [content, password])
    assert.notEqual(checkUpstream(seen[oldCount]), checkUpstream(seen[oldCount + 1]), 'dedupe must not share masking maps')
  }
  console.log('PASS review regression: original-byte dedupe (3 protocols), prompt egress (Chat/Responses), +86, stateful guard, normalized 100 terms')
  const forcePrefix = 'fixture_' + 'kL92jL750p'
  const forceRegex = 'Q4711X'
  const keepEmail = ['keep', '@', 'mailprobe.internal'].join('')
  const hideEmail = ['paired', '@', 'mailprobe.internal'].join('')
  const keepBucket = 's3://my-bucket/test'
  const hideBucket = 's3://custom-fixture-bucket/test'
  const birth = ['1990', '05', '20'].join('-')
  const dateVariant = ['1990', '5', '20'].join('/')
  const caseVariant = 'FIXTURECODEWORD'
  await savePrivacy({ ...enabled, kinds: { bucket: true, mac: false }, terms: [birth, 'fixturecodeword'],
    terms_any_case: true, terms_whole: true, allow: [keepEmail.toUpperCase()],
    rules: [{ kind: 'DEMO', prefix: 'fixture_' }, { kind: 'DEMO', regex: 'match:([0-9A-Z]{6})' }] })
  const input = [forcePrefix, 'match:' + forceRegex, keepEmail, hideEmail, keepBucket, hideBucket, dateVariant,
    caseVariant, caseVariant + 'extra'].join(' | ')
  const advanced = body('chat_completions', false)
  advanced.messages[0].content = input
  const advancedReply = await json(paths.chat_completions, 'POST', advanced)
  assert.equal(advancedReply.choices[0].message.content, input)
  const wire = seen.at(-1).text.split(' | ')
  for (const i of [0, 1, 3, 5, 6, 7]) assert.match(wire[i], /\{\{A2A_[0-9a-f]{32}\}\}/, 'missing advanced rule index ' + i)
  for (const i of [2, 4, 8]) assert.equal(wire[i], input.split(' | ')[i])
  console.log('PASS Magpie custom rules, allowlist, date aliases and category opt-in via actual upstream wire')
  await savePrivacy({ ...enabled, terms: [sentinel, encodedWord, toolHistoryId] })
  echoToolResult = true
  for (const protocol of Object.keys(paths)) {
    const request = body(protocol, false)
    const raw = JSON.stringify(toolResult)
    if (protocol === 'chat_completions') request.messages.at(-1).content = raw
    else if (protocol === 'responses') request.input.at(-1).output = [{ type: 'input_text', text: raw }]
    else request.messages.at(-1).content[0].content = [textPart(raw)]
    const reply = await json(paths[protocol], 'POST', request)
    const text = protocol === 'chat_completions' ? reply.choices[0].message.content
      : protocol === 'responses' ? reply.output[0].content[0].text : reply.content[0].text
    assert.deepEqual(JSON.parse(text), toolResult, 'echoed tool JSON must restore exactly: ' + protocol)
    const upstreamBody = seen.at(-1).body
    let history, masked, id
    if (protocol === 'responses') {
      history = upstreamBody.input.find(item => item.type === 'function_call_output')
      masked = history.output[0].text
      id = history.call_id
    } else if (protocol === 'chat_completions') {
      history = upstreamBody.messages.find(item => item.role === 'tool')
      masked = history.content
      id = history.tool_call_id
    } else {
      history = upstreamBody.messages.flatMap(item => Array.isArray(item.content) ? item.content : []).find(item => item.type === 'tool_result')
      masked = history.content[0].text
      id = history.tool_use_id
    }
    const fields = JSON.parse(masked)
    for (const field of toolSecretKeys) assert.match(fields[field], /^\{\{A2A_[0-9a-f]{32}\}\}$/, 'unmasked named tool result field: ' + protocol + ' ' + field)
    for (const field of ['access_token_expires_at', 'env', 'media', 'rows', 'nextPageToken', 'secret_hash', 'element_token']) assert.deepEqual(fields[field], toolResult[field], 'preserve non-secret tool result field: ' + protocol + ' ' + field)
    assert.equal(id, toolHistoryId)
  }
  echoToolResult = false
  console.log('PASS Magpie tool-result fields: credential names, JSON round-trip, templates, expiry, attachments and protocol IDs (3 protocols)')
  // Magpie keeps numeric measurements and versions but masks labelled numeric keys.
  await savePrivacy({ ...disabled, secrets: true })
  const numericCases = [
    ['密钥为“', '835294', '”'], ['API Key：', '540076', ''],
    ['秘密鍵は ', '946150', ''], ['비밀키는 ', '260893', ''],
  ]
  const quantities = ['密钥 835294', '密钥为 2048 位', '令牌是 582333 毫秒',
    'token 是 582333 ms', '秘密鍵は 582333 バイト', '비밀키는 582333 바이트',
    'token 是 0.15.0-rc1', '令牌是 2026-01-01T11:44:31', '私钥 ~/.ssh/id_ed25519',
  ]
  const numericText = [...numericCases.map(parts => parts.join('')), ...quantities].join('\n')
  const numeric = body('chat_completions', false)
  numeric.messages[0].content = numericText
  const numericReply = await json(paths.chat_completions, 'POST', numeric)
  assert.equal(numericReply.choices[0].message.content, numericText, 'restore numeric keys without changing surrounding labels')
  const numericWire = seen.at(-1).text.split('\n')
  assert.equal(numericWire.length, numericCases.length + quantities.length)
  for (const [i, [label, original, suffix]] of numericCases.entries()) {
    const line = numericWire[i]
    assert(line.startsWith(label) && line.endsWith(suffix) && !line.includes(original), 'unmasked multilingual numeric key: ' + i)
    assert.match(line.slice(label.length, line.length - suffix.length), /^\{\{A2A_[0-9a-f]{32}\}\}$/)
  }
  assert.deepEqual(numericWire.slice(numericCases.length), quantities, 'do not mask key lengths, durations, versions or paths')
  console.log('PASS Magpie multilingual numeric keys and quantity exclusions on the upstream wire')
  const idLikeCard = ['abcd4111', '1111', '1111', '9007', 'abcdef123456'].join('-')
  const keptIds = [
    'order ' + idLikeCard, 'next_page_token: ' + lookupId,
    'secret_id: ' + lookupId, 'secret_hash: ' + lookupDigest,
    '"key": "' + lookupDigest + '"', 'element_token=' + elementToken,
  ]
  const credentialIds = [['refresh_token=', lookupId], ['api_key=', lookupDigest], ['password=', elementToken]]
  const identifierInput = [...keptIds, ...credentialIds.map(parts => parts.join(''))].join('\n')
  await savePrivacy(enabled)
  const identifierRequest = body('chat_completions', false)
  identifierRequest.messages[0].content = identifierInput
  const identifierReply = await json(paths.chat_completions, 'POST', identifierRequest)
  assert.equal(identifierReply.choices[0].message.content, identifierInput)
  const identifierWire = seen.at(-1).text.split('\n')
  assert.deepEqual(identifierWire.slice(0, keptIds.length), keptIds, 'identifier and digest context changed')
  for (const [i, [label, value]] of credentialIds.entries()) {
    const line = identifierWire[keptIds.length + i]
    assert(line.startsWith(label) && !line.includes(value), 'labelled credential escaped identifier guard: ' + i)
    assert.match(line.slice(label.length), /^\{\{A2A_[0-9a-f]{32}\}\}$/)
  }
  await savePrivacy({ ...enabled, terms: [lookupId] })
  await json(paths.chat_completions, 'POST', identifierRequest)
  assert(!seen.at(-1).text.includes(lookupId), 'explicit custom terms must override identifier exclusions')
  console.log('PASS Magpie identifier guards: UUID-like cards, hashes, cursors, element IDs and real credential fields')
  await savePrivacy({ ...enabled, terms: [sentinel, encodedWord] })
  const declaration = {
    type: 'function', name: 'read_' + encodedWord, description: 'tool ' + sentinel,
    parameters: { type: 'object', properties: { [encodedWord]: {
      type: 'string', description: 'field ' + sentinel, enum: [encodedWord],
    } }, required: [encodedWord], additionalProperties: false },
  }
  const namespace = { type: 'namespace', name: 'group_' + encodedWord, description: 'group ' + sentinel,
    tools: [declaration],
  }
  const grammar = { type: 'custom', name: 'grammar_' + encodedWord, description: 'custom ' + sentinel,
    format: { type: 'grammar', syntax: 'lark', definition: 'start: "' + encodedWord + '"' },
  }
  const declarationRequest = body('responses', false)
  declarationRequest.tools = [namespace, grammar]
  declarationRequest.input.push(
    { type: 'additional_tools', tools: [{ ...namespace, name: 'inline_' + encodedWord }] },
    { type: 'tool_search_output', call_id: 'search_local', tools: [declaration] },
  )
  const declarationReply = await json(paths.responses, 'POST', declarationRequest)
  assert.equal(declarationReply.output[0].content[0].text, content)
  const sentDefinitions = seen.at(-1).body
  const inline = sentDefinitions.input.find(item => item.type === 'additional_tools')
  const searchResult = sentDefinitions.input.find(item => item.type === 'tool_search_output')
  assert(inline && searchResult, 'native Responses must preserve inline declaration envelopes')
  const assertDefinition = (tool, label) => {
    assert.match(tool.description, /\{\{A2A_[0-9a-f]{32}\}\}/, 'unmasked declaration description: ' + label)
    assert(!tool.description.includes(sentinel), 'unmasked declaration description: ' + label)
    assert.equal(tool.name, declaration.name)
    assert.deepEqual(tool.parameters.required, declaration.parameters.required)
    const property = tool.parameters.properties[encodedWord]
    assert.match(property.description, /\{\{A2A_[0-9a-f]{32}\}\}/, 'unmasked declaration schema description: ' + label)
    assert.deepEqual(property.enum, [encodedWord])
  }
  assertDefinition(sentDefinitions.tools[0].tools[0], 'namespace child')
  assertDefinition(inline.tools[0].tools[0], 'additional_tools namespace child')
  assertDefinition(searchResult.tools[0], 'tool_search_output')
  assert.equal(sentDefinitions.tools[0].name, namespace.name)
  assert.equal(inline.tools[0].name, 'inline_' + encodedWord)
  for (const tool of [sentDefinitions.tools[0], inline.tools[0], sentDefinitions.tools[1]]) {
    assert.match(tool.description, /\{\{A2A_[0-9a-f]{32}\}\}/)
    assert(!tool.description.includes(sentinel))
  }
  assert.deepEqual(sentDefinitions.tools[1].format, grammar.format, 'custom tool grammar must stay as defined')
  assert.equal(searchResult.call_id, 'search_local')
  console.log('PASS Magpie tool definitions: namespaces, inline/search tools, schema descriptions and opaque grammar')
  const reviewFailures = []
  async function reviewRegression(name, run) {
    try { await run(); console.log('PASS REVIEW', name) }
    catch (error) { reviewFailures.push({ name, error: error.message }); console.log('FAIL REVIEW', name) }
    finally { echoToolResult = false; reviewMode = '' }
  }
  const sendText = async (text, policy) => {
    await savePrivacy(policy)
    const request = body('chat_completions', false)
    request.messages[0].content = text
    const response = await json(paths.chat_completions, 'POST', request)
    assert.equal(response.choices[0].message.content, text, 'client text must restore exactly')
    return seen.at(-1).text
  }
  const secretsOnly = { ...disabled, secrets: true }
  const placeholderOnly = /^\{\{A2A_[0-9a-f]{32}\}\}$/
  await reviewRegression('explicit alphabetic and numeric passwords', async () => {
    const values = [['Orchid','Fir','Seagull'].join(''), ['9415','8206'].join('')]
    const input = values.map(value => 'password="' + value + '"').join('\n')
    const wire = (await sendText(input, secretsOnly)).split('\n')
    for (const [i, line] of wire.entries()) {
      assert.match(line.slice('password="'.length, -1), placeholderOnly)
      assert(!line.includes(values[i]))
    }
    const harmless = ['process.env.FIXTURE_PASSWORD', '2030-12-30T16:48:25+08:00', ['$', '{FIXTURE_PASSWORD}'].join('')]
    const prose = [...values, ...harmless.map(value => 'password=' + value)].join('\n')
    assert.equal(await sendText(prose, secretsOnly), prose, 'unlabelled text and references stay unchanged')
  })
  await reviewRegression('entire colon-bearing credential value', async () => {
    const value = ['LocalCredentialFixture93', ':14'].join('')
    const sent = await sendText('password=' + value, secretsOnly)
    assert.match(sent.slice('password='.length), placeholderOnly)
    assert(!sent.includes(':14'))
  })
  await reviewRegression('example-domain label boundaries', async () => {
    const addresses = [['alice','notexample.com'].join('@'), ['alice','example.com'].join('@'), ['alice','docs.example.com'].join('@')]
    const input = [...addresses, 'hostname: notexample.com', 'hostname: docs.example.com'].join('\n')
    const wire = (await sendText(input, { ...disabled, personal: true, kinds: { userhost: true } })).split('\n')
    assert.match(wire[0], placeholderOnly)
    assert.deepEqual(wire.slice(1,3), addresses.slice(1))
    assert.match(wire[3].slice('hostname: '.length), placeholderOnly)
    assert.equal(wire[4], 'hostname: docs.example.com')
  })
  await reviewRegression('file_password overrides generic identifier words', async () => {
    const value = ['96a34f05', 'cd0243a7', '90b2dd48', '91c5fa36'].join('')
    const input = ['file_password=' + value, 'file_hash=' + value, 'page_token=' + value].join('\n')
    const wire = (await sendText(input, secretsOnly)).split('\n')
    assert.match(wire[0].slice('file_password='.length), placeholderOnly)
    assert.deepEqual(wire.slice(1), input.split('\n').slice(1))
  })
  await reviewRegression('all mnemonic spans within one detector match', async () => {
    const first = 'abandon ability able about above absent absorb abstract absurd abuse access accident'
    const second = 'zoo zone zero zebra youth young yellow year wrap write wrist wrong'
    const wire = await sendText('mnemonic: ' + first + ' backup ' + second, secretsOnly)
    const parts = wire.slice('mnemonic: '.length).split(' backup ')
    assert.equal(parts.length, 2)
    for (const part of parts) assert.match(part, placeholderOnly)
    const joined = await sendText('mnemonic: ' + first + ' ' + second, secretsOnly)
    assert.match(joined.slice('mnemonic: '.length), placeholderOnly)
  })
  await reviewRegression('JSON scalar tool text applies custom terms', async () => {
    const cases = [['42424242','42424242'], ['true','true'], ['null','null'], [JSON.stringify('scalarFixture'),'scalarFixture']]
    for (const protocol of Object.keys(paths)) for (const [raw, term] of cases) {
      await savePrivacy({ ...disabled, terms: [term] })
      const request = body(protocol, false)
      if (protocol === 'chat_completions') request.messages.at(-1).content = raw
      else if (protocol === 'responses') request.input.at(-1).output = raw
      else request.messages.at(-1).content[0].content = raw
      echoToolResult = true
      const response = await json(paths[protocol], 'POST', request)
      const returned = protocol === 'responses' ? response.output[0].content[0].text
        : protocol === 'anthropic' ? response.content[0].text : response.choices[0].message.content
      assert.equal(returned, raw)
      const payload = seen.at(-1).body
      const output = protocol === 'responses' ? payload.input.find(item => item.type === 'function_call_output').output
        : protocol === 'anthropic' ? payload.messages.flatMap(item => Array.isArray(item.content) ? item.content : []).find(item => item.type === 'tool_result').content
          : payload.messages.find(item => item.role === 'tool').content
      assert(!output.includes(term), 'raw scalar leaked in ' + protocol)
      assert.match(output, /\{\{A2A_[0-9a-f]{32}\}\}/)
    }
  })
  await reviewRegression('owned placeholders do not consume fresh match budget', async () => {
    // Exercise the full 40k-match request and a new gateway prompt. The upstream
    // answers briefly; echoing 1.6 MB is unrelated to the request matching budget.
    await savePrivacy({ ...disabled, terms: ['A'] })
    await json('/api/prompt', 'PUT', { promptMode: 'append', promptText: 'A' })
    compactBudgetReply = true
    try {
      const request = body('chat_completions', false)
      request.messages[0].content = 'A '.repeat(40_000)
      const response = await json(paths.chat_completions, 'POST', request)
      assert.equal(response.choices[0].message.content, 'budget accepted')
      const captured = seen.at(-1)
      const tokens = captured.text.match(/\{\{A2A_[0-9a-f]{32}\}\}/g)
      assert.equal(tokens?.length, 40_000)
      assert.equal(captured.text, (tokens[0] + ' ').repeat(40_000))
      assert.equal(captured.prompt, tokens[0], 'mask newly injected plaintext without remasking owned tokens')
    } finally {
      compactBudgetReply = false
      await json('/api/prompt', 'PUT', { promptMode: 'passthrough', promptText: '' })
    }
  })
  await reviewRegression('Responses echo restores instructions and tool descriptions', async () => {
    await savePrivacy({ ...disabled, terms: [sentinel] })
    for (const stream of [false,true]) {
      const request = body('responses', stream)
      request.instructions = 'instructions ' + sentinel
      request.tools = [{ type: 'function', name: 'fixture_echo', description: 'tool ' + sentinel,
        parameters: { type: 'object', properties: {}, additionalProperties: false } }]
      const response = await call(paths.responses, 'POST', request)
      assert(response.ok)
      const raw = await response.text()
      const envelopes = stream ? raw.split(/\r?\n/).filter(line => line.startsWith('data:'))
        .map(line => JSON.parse(line.slice(5).trim())).filter(event => event.response).map(event => event.response) : [JSON.parse(raw)]
      assert(envelopes.length > 0)
      for (const envelope of envelopes) {
        assert.equal(envelope.instructions, request.instructions)
        assert.deepEqual(envelope.tools, request.tools)
      }
      assert(!seen.at(-1).body.instructions.includes(sentinel))
      assert(!seen.at(-1).body.tools[0].description.includes(sentinel))
    }
  })
  await reviewRegression('Unicode escaped JSON string scalars are masked', async () => {
    const term = String.fromCodePoint(0x5f20, 0x4e09)
    const slash = String.fromCharCode(92)
    const escaped = '"' + slash + 'u5f20' + slash + 'u4e09"'
    await savePrivacy({ ...disabled, terms: [term] })
    for (const protocol of Object.keys(paths)) {
      const request = body(protocol, false)
      if (protocol === 'chat_completions') request.messages.at(-1).content = escaped
      else if (protocol === 'responses') request.input.at(-1).output = escaped
      else request.messages.at(-1).content[0].content = escaped
      echoToolResult = true
      const response = await json(paths[protocol], 'POST', request)
      const returned = protocol === 'responses' ? response.output[0].content[0].text
        : protocol === 'anthropic' ? response.content[0].text : response.choices[0].message.content
      assert.equal(JSON.parse(returned), term, 'decoded JSON value must restore')
      const payload = seen.at(-1).body
      const wire = protocol === 'responses' ? payload.input.find(item => item.type === 'function_call_output').output
        : protocol === 'anthropic' ? payload.messages.flatMap(item => Array.isArray(item.content) ? item.content : []).find(item => item.type === 'tool_result').content
          : payload.messages.find(item => item.role === 'tool').content
      assert.match(JSON.parse(wire), placeholderOnly, 'decoded Unicode term leaked in ' + protocol)
    }
  })
  await reviewRegression('privacy settings reject stale compare-and-swap saves', async () => {
    const pageA = (await json(endpoint)).data
    const pageB = await savePrivacy({ ...enabled, revision: pageA.revision })
    const stale = await call(endpoint, 'PUT', { ...disabled, terms: ['stale-page-draft'], revision: pageA.revision })
    assert.equal(stale.status, 409, 'a stale page must not turn protection off')
    assert.match(pageA.revision ?? '', /^[0-9a-f]{64}$/, 'settings require an opaque revision')
    assert.notEqual(pageB.data.revision, pageA.revision)
    const current = (await json(endpoint)).data
    assert.deepEqual(current, pageB.data, 'conflicting save must leave config and revision unchanged')
    assert.deepEqual(await read('chat_completions', false), [content, password])
    checkUpstream(seen.at(-1))
    const attempts = await Promise.all([
      call(endpoint, 'PUT', { ...enabled, terms: [sentinel, 'writer-one'], revision: current.revision }),
      call(endpoint, 'PUT', { ...enabled, terms: [sentinel, 'writer-two'], revision: current.revision }),
    ])
    assert.deepEqual(attempts.map(response => response.status).sort(), [200,409], 'revision compare must be atomic under simultaneous writes')
  })
  const eventValues = raw => raw.trimStart().split(/\r\n|\r|\n/)
    .filter(line => line.startsWith('data:')).map(line => line.slice(5).trim())
    .filter(line => line && line !== '[DONE]').map(JSON.parse)
  await reviewRegression('whole-word dates respect surrounding letters', async () => {
    const day = ['1990','05','20'].join('-')
    const wire = await sendText('item' + day + 'draft | ' + day, { ...disabled, terms: [day], terms_whole: true })
    const [kept, masked] = wire.split(' | ')
    assert.equal(kept, 'item' + day + 'draft'); assert.match(masked, placeholderOnly)
  })
  await reviewRegression('valid grouped card prefix survives a separate code', async () => {
    const card = ['4111','1111','1111','1111'].join(' ')
    const wire = await sendText(card + ' 123', { ...disabled, personal: true })
    assert.equal(wire.slice(-4), ' 123'); assert.match(wire.slice(0,-4), placeholderOnly)
  })
  await reviewRegression('adjacent repaired IBANs are both masked', async () => {
    const iban = ['GB82','WEST','1234','5698','7654','32'].join('')
    const wire = await sendText(iban + ' ' + iban, { ...disabled, personal: true })
    const values = wire.split(' ')
    assert.equal(values.length, 2); values.forEach(value => assert.match(value, placeholderOnly))
  })
  await reviewRegression('short regex values apply and empty capture groups do not', async () => {
    assert.match(await sendText('abc', { ...secretsOnly, rules: [{ regex: '^abc$' }] }), placeholderOnly)
    assert.equal(await sendText('b', { ...secretsOnly, rules: [{ regex: '(a*)b' }] }), 'b')
  })
  await reviewRegression('numeric array values remain unchanged at scale', async () => {
    const input = '{"phone_numbers":[' + Array(4096).fill(syntheticPhone.slice(3)).join(',') + ']}'
    assert.equal(await sendText(input, { ...disabled, personal: true }), input)
  })
  await reviewRegression('MCP approval arguments mask values but preserve identifiers', async () => {
    await savePrivacy({ ...disabled, terms: [sentinel] })
    const request = body('responses', false)
    const approval = { type: 'mcp_approval_request', id: 'approval_' + sentinel, name: 'tool_' + sentinel,
      server_label: 'server_' + sentinel, arguments: JSON.stringify({ note: sentinel }) }
    request.input.push(approval)
    await json(paths.responses, 'POST', request)
    const actual = seen.at(-1).body.input.find(item => item.type === 'mcp_approval_request')
    assert(actual, 'native approval item was removed')
    assert.deepEqual({ ...actual, arguments: approval.arguments }, approval)
    assert.match(JSON.parse(actual.arguments).note, placeholderOnly)
  })
  await reviewRegression('interleaved MCP deltas and done arguments restore separately', async () => {
    const originals = ['first"quoted\\fixture', 'second"quoted\\fixture']
    await savePrivacy({ ...disabled, terms: originals })
    const request = body('responses', true); request.input[0].content = originals.join(' | ')
    reviewMode = 'mcp-events'
    const response = await call(paths.responses, 'POST', request); assert(response.ok)
    const events = eventValues(await response.text())
    for (const output_index of [0,1]) {
      const delta = events.filter(event => event.type === 'response.mcp_call_arguments.delta' && event.output_index === output_index).map(event => event.delta).join('')
      assert.equal(JSON.parse(delta).value, originals[output_index])
      const done = events.find(event => event.type === 'response.mcp_call_arguments.done' && event.output_index === output_index)
      assert.equal(JSON.parse(done.arguments).value, originals[output_index])
    }
  })
  await reviewRegression('structured JSON output restores only assistant JSON fields', async () => {
    const secret = 'A"B\\C\nD'
    await savePrivacy({ ...disabled, terms: [secret] }); reviewMode = 'structured'
    for (const protocol of Object.keys(paths)) for (const stream of [false,true]) {
      const request = body(protocol, stream)
      if (protocol === 'responses') {
        request.input[0].content = secret; request.text = { format: { type: 'json_object' } }
        request.instructions = 'instructions ' + secret
        request.tools = [{ type: 'function', name: 'echo', description: 'description ' + secret, parameters: { type: 'object', properties: {} } }]
      } else {
        request.messages[0].content = secret
        if (protocol === 'chat_completions') request.response_format = { type: 'json_object' }
        else request.output_config = { format: { type: 'json_schema', schema: { type: 'object', properties: { value: { type: 'string' } }, required: ['value'], additionalProperties: false } } }
      }
      const response = await call(paths[protocol], 'POST', request); const raw = await response.text()
      assert(response.ok, 'structured response failed: ' + protocol + ' ' + raw.slice(0,100))
      let returned
      if (!stream) {
        const value = JSON.parse(raw)
        returned = protocol === 'chat_completions' ? value.choices[0].message.content : protocol === 'responses' ? value.output[0].content[0].text : value.content[0].text
        if (protocol === 'responses') { assert.equal(value.instructions, request.instructions); assert.deepEqual(value.tools, request.tools) }
      } else {
        const events = eventValues(raw)
        returned = protocol === 'chat_completions' ? events.map(event => event.choices?.[0]?.delta?.content ?? '').join('')
          : protocol === 'responses' ? events.filter(event => event.type === 'response.output_text.delta').map(event => event.delta).join('') : events.map(event => event.delta?.text ?? '').join('')
        if (protocol === 'responses') for (const event of events.filter(event => event.response)) {
          assert.equal(event.response.instructions, request.instructions); assert.deepEqual(event.response.tools, request.tools)
        }
      }
      assert.deepEqual(JSON.parse(returned), { value: secret }, protocol + ' structured value was corrupted')
    }
  })
  for (const mode of ['bom','lone-cr']) await reviewRegression('SSE framing restores ' + mode, async () => {
    await savePrivacy(enabled); reviewMode = mode
    const response = await call(paths.responses, 'POST', body('responses', true)); assert(response.ok)
    const events = eventValues(await response.text())
    assert.equal(events.filter(event => event.type === 'response.output_text.delta').map(event => event.delta).join(''), content)
  })
  await reviewRegression('SSE pending-token cap reports an actual stream error', async () => {
    await savePrivacy(enabled); reviewMode = 'wait-cap'
    const response = await call(paths.chat_completions, 'POST', body('chat_completions', true)); assert(response.ok)
    await assert.rejects(response.text(), /terminated|socket|other side closed|fetch failed|aborted/i)
  })
  await reviewRegression('title routing uses the original content before masking', async () => {
    const providerId = providerIds.chat_completions
    await json('/api/custom-providers/models','POST',{providerId,models:[{id:'privacy-chat_completions',enabled:true},{id:'privacy-title',enabled:true},{id:'privacy-vision',enabled:true,capabilities:{supportsImages:true}}],mappings:[]})
    const old = (await json('/api/models/title-routes')).data
    try {
      await json('/api/models/title-routes','PUT',{revision:old.revision,config:{enabled:true,rules:[{enabled:true,clients:['claude-code'],model:'privacy-title',fallback:'error'}]}})
      await savePrivacy({ ...disabled, terms: ['title'] }); reviewMode = 'title'
      const request = {model:'privacy-chat_completions',messages:[message('user','Please write a 5-10 word title for this fixture')]}
      const response = await call(paths.chat_completions,'POST',request,true,{'user-agent':'claude-cli/2.0.0'})
      assert(response.ok, await response.text())
      assert.equal(seen.at(-1).body.model,'privacy-title')
      assert(!seen.at(-1).text.includes('title'))
    } finally { const latest=(await json('/api/models/title-routes')).data;await json('/api/models/title-routes','PUT',{revision:latest.revision,config:old.config}) }
  })
  await reviewRegression('vision child shares masking and bypasses cross-request cache', async () => {
    const providerId = providerIds.chat_completions
    const old = (await json('/api/vision-compatibility')).data
    const newline = String.fromCharCode(10), term = 'ALPHA' + newline + 'OMEGA'
    try {
      await json('/api/vision-compatibility','PUT',{revision:old.revision,config:{...old.config,enabled:true,unknownCapability:'compat',rules:[{enabled:true,models:[{provider:providerId,model:'privacy-chat_completions'}],mode:'assist',source:'custom',target:{provider:providerId,model:'privacy-vision'},reuseResults:true}]}})
      await savePrivacy({...disabled,terms:[term]});reviewMode='vision'
      const request={model:'privacy-chat_completions',messages:[message('user',[textPart('ALPHA'),textPart('OMEGA'),{type:'image_url',image_url:{url:'data:image/png;base64,AQ=='}}])]}
      const oldCount=seen.length;let previousToken
      for(let round=0;round<2;round++) {
        const response=await call(paths.chat_completions,'POST',request);const raw=await response.text();assert(response.ok,raw.slice(0,500))
        const calls=seen.slice(oldCount).filter(item=>item.body.model==='privacy-vision')
        assert.equal(calls.length,round+1,'masked vision request must not reuse a prior map through the cache')
        const text=calls.at(-1).text;assert(!text.includes(term),'joined child text leaked')
        const token=text.match(/[{][{]A2A_[0-9a-f]{32}[}][}]/)?.[0];assert(token)
        if(round)assert.notEqual(token,previousToken);previousToken=token
        assert(!raw.includes('{{A2A_'),'vision result retained request placeholder')
      }
    } finally { const latest=(await json('/api/vision-compatibility')).data;await json('/api/vision-compatibility','PUT',{revision:latest.revision,config:old.config}) }
  })
  const beforeBlocks = seen.length
  const blocked = await call('/v1/images/generations', 'POST', { prompt: content })
  assert.equal(blocked.status, 400)
  assert.equal((await blocked.json()).error.code, 'privacy_masking')
  assert.equal((await call(paths.chat_completions, 'POST', body('chat_completions', false), true, { 'content-encoding': 'gzip' })).status, 415)
  assert.equal(seen.length, beforeBlocks)
  assert.deepEqual(settingsOf((await savePrivacy(disabled)).data), disabled)
  assert.deepEqual(await read('chat_completions', false), [content, password])
  assert.equal(seen.at(-1).text, content)
  console.log('PASS default/off, authorization, fail-closed, and request isolation')
  // Run the deliberate egress error last: the existing account error policy
  // may cool down that account, which is unrelated to the off-path assertions.
  await reviewRegression('egress masking failures finalize the actual attempt', async () => {
    await savePrivacy({...disabled,terms:['A']})
    await json('/api/prompt','PUT',{promptMode:'append',promptText:'A '.repeat(65537)})
    try {
      const before=seen.length;const request=body('chat_completions',false);request.messages[0].content='small input'
      const response=await call(paths.chat_completions,'POST',request);assert.equal(response.status,503);assert.equal((await response.json()).error.code,'privacy_masking');assert.equal(seen.length,before)
      const records=(await json('/api/stats/requests?status=error&limit=10')).data
      const rows=records.entries??records.requests??records.items;assert(Array.isArray(rows),'request stats rows missing: '+Object.keys(records))
      const row=rows.find(item=>item.attemptDetails?.length&&item.status===503);assert(row,'failed request missing attempt details')
      const attempt=row.attemptDetails.at(-1);assert.equal(attempt.status,503);assert(attempt.error,'preparation error not finalized')
    } finally {await json('/api/prompt','PUT',{promptMode:'passthrough',promptText:''})}
  })
  assert.deepEqual(reviewFailures, [], 'new review regressions must all pass')
} catch (failure) {
  console.error(output)
  throw failure
} finally {
  if (child && child.pid && child.exitCode === null && child.signalCode === null) {
    const exited = once(child, 'exit')
    child.kill()
    await exited
  }
  upstream.closeAllConnections()
  if (upstream.listening) await end(upstream)
  await rm(home, { recursive: true, force: true, maxRetries: 5 })
}

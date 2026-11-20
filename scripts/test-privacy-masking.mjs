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
const fakeKey = ['sk', 'projectfixture123456789'].join('-')
const email = 'fixture@example.test'
const password = 'fake"quoted\\value'
const content = [sentinel, fakeKey, email, fakeKey].join(' ')
const seen = []
let foreign = ''
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
    const tool = protocol === 'anthropic' ? messages[1].content[0].input
      : JSON.parse(protocol === 'responses' ? messages[1].arguments : messages[1].tool_calls[0].function.arguments)
    assert.equal(typeof text, 'string')
    const actualAuth = protocol === 'anthropic' ? req.headers['x-api-key'] : req.headers.authorization
    seen.push({ protocol, body, text, tool, actualAuth })
    const reply = (foreign ? foreign + ' | ' : '') + text
    const msg = { id: 'msg_local', type: 'message', role: 'assistant', model: body.model,
      content: [textPart(reply), { type: 'tool_use', name: 'echo', id: 'call_local', input: tool }],
      stop_reason: 'tool_use', stop_sequence: null, usage: { input_tokens: 3, output_tokens: 2 } }
    const response = { id: 'resp_local', object: 'response', model: body.model, status: 'completed',
      output: [{ type: 'message', role: 'assistant', id: 'msg_local', content: [{ type: 'output_text', text: reply, annotations: [] }] },
        { type: 'function_call', name: 'echo', call_id: 'call_local', id: 'fc_local', arguments: args(tool.password) }],
      usage: { input_tokens: 3, output_tokens: 2, total_tokens: 5 } }
    const chat = (object, choice) => ({ id: 'chatcmpl_local', object, model: body.model, created: 1, choices: [choice] })
    if (!body.stream) {
      res.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify(
        protocol === 'anthropic' ? msg : protocol === 'responses' ? response
          : chat('chat.completion', { index: 0, message: { ...message('assistant', reply), tool_calls: calls(args(tool.password)) }, finish_reason: 'tool_calls' })))
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
    AGENT2API_PROXY_API_KEY: key, AGENT2API_CAPTCHA_ENABLED: '0', NO_PROXY: '127.0.0.1,localhost', no_proxy: '127.0.0.1,localhost' },
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
  assert.equal((await call(endpoint, 'GET', undefined, false)).status, 401)
  const disabled = { secrets: false, personal: false, terms: [] }
  assert.deepEqual((await json(endpoint)).data, disabled)
  await json('/api/sanitize', 'PUT', { sanitizeBlacklistFingerprints: false })
  for (const protocol of Object.keys(paths)) {
    const baseUrl = upstreamUrl + '/' + protocol + (protocol === 'anthropic' ? '' : '/v1')
    const id = (await json('/api/custom-providers', 'POST', { name: 'privacy ' + protocol, baseUrl, protocol, apiKey: upstreamKey })).data.provider.id
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
  assert.equal((await call(endpoint, 'PUT', { secrets: 'wrong' })).status, 400)
  assert.deepEqual((await json(endpoint, 'PUT', enabled)).data, enabled)
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
  const before = seen.length
  const blocked = await call('/v1/images/generations', 'POST', { prompt: content })
  assert.equal(blocked.status, 400)
  assert.equal((await blocked.json()).error.code, 'privacy_masking')
  assert.equal((await call(paths.chat_completions, 'POST', body('chat_completions', false), true, { 'content-encoding': 'gzip' })).status, 415)
  assert.equal(seen.length, before)
  assert.deepEqual((await json(endpoint, 'PUT', disabled)).data, disabled)
  assert.deepEqual(await read('chat_completions', false), [content, password])
  assert.equal(seen.at(-1).text, content)
  console.log('PASS default/off, authorization, fail-closed, and request isolation')
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

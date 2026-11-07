// Agent2API image-generation bridge
'use strict';

const fs = require('node:fs/promises');
const path = require('node:path');
const crypto = require('node:crypto');
const { fileURLToPath } = require('node:url');

const ENDPOINT = process.env.AGENT2API_IMAGE_ENDPOINT || '';
const API_KEY = process.env.AGENT2API_IMAGE_KEY || '';
const OUTPUT = process.env.AGENT2API_IMAGE_OUTPUT_DIR || '';
const MAX_INPUT = 16 * 1024 * 1024;
const MAX_IMAGE = 32 * 1024 * 1024;
const versions = ['2025-11-25', '2025-06-18', '2025-03-26', '2024-11-05'];
const savedOutputs = new Map();
const active = new Set();
let serial = 0;

function failure(message, extra = {}) {
  const data = { status: 'error', message, ...extra };
  return { content: [{ type: 'text', text: JSON.stringify(data) }], structuredContent: data, isError: true };
}

function resultData(result) {
  if (result && result.structuredContent && typeof result.structuredContent === 'object') return result.structuredContent;
  for (const item of result && Array.isArray(result.content) ? result.content : []) {
    if (item.type === 'text') {
      try { const data = JSON.parse(item.text); if (data && typeof data === 'object') return data; } catch {}
    }
  }
  return null;
}

function endpoint() {
  const url = new URL(ENDPOINT);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) throw new Error('Invalid MCP endpoint');
  if (url.protocol === 'http:' && !['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)) throw new Error('Remote connections require HTTPS');
  if (!API_KEY) throw new Error('Gateway API key is missing');
  return url;
}

async function readBody(response, limit) {
  if (Number(response.headers.get('content-length') || 0) > limit) throw new Error('Response exceeds the size limit');
  const reader = response.body.getReader();
  const chunks = [];
  let length = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      length += value.length;
      if (length > limit) throw new Error('Response exceeds the size limit');
      chunks.push(Buffer.from(value));
    }
  } finally {
    await reader.cancel().catch(() => {});
  }
  return Buffer.concat(chunks, length);
}

async function remote(method, params, signal) {
  const url = endpoint();
  const response = await fetch(url, {
    method: 'POST', redirect: 'error',
    headers: { 'content-type': 'application/json', accept: 'application/json, text/event-stream',
      authorization: 'Bearer ' + API_KEY, 'MCP-Protocol-Version': '2025-11-25', 'user-agent': 'Agent2API-ImageBridge/1.0' },
    body: JSON.stringify({ jsonrpc: '2.0', id: ++serial, method, params }), signal,
  });
  const payload = JSON.parse((await readBody(response, 4 * 1024 * 1024)).toString('utf8'));
  if (!response.ok || payload.error || !payload.result) throw new Error(payload.error?.message || 'MCP endpoint returned HTTP ' + response.status);
  return payload.result;
}

function within(root, target) {
  const relative = path.relative(root, target);
  return !path.isAbsolute(relative) && relative !== '..' && !relative.startsWith('..' + path.sep);
}

async function rejectLinks(target) {
  const absolute = path.resolve(target);
  const parsed = path.parse(absolute);
  let current = parsed.root;
  for (const part of absolute.slice(parsed.root.length).split(path.sep).filter(Boolean)) {
    current = path.join(current, part);
    const stat = await fs.lstat(current);
    if (stat.isSymbolicLink()) throw new Error('Symbolic links are not allowed in image paths');
  }
  return fs.realpath(absolute);
}

async function outputRoot() {
  if (!OUTPUT || !path.isAbsolute(OUTPUT)) throw new Error('An authorized absolute output directory is required; reinstall the MCP configuration');
  const root = await rejectLinks(OUTPUT);
  if (!(await fs.stat(root)).isDirectory()) throw new Error('The output directory is unavailable');
  return root;
}

async function destination(relative) {
  if (relative === undefined || relative === '') return outputRoot();
  if (typeof relative !== 'string' || path.isAbsolute(relative) || /[\\:]/.test(relative)) throw new Error('output_subdir must be a relative directory inside the authorized output directory');
  const parts = relative.split('/');
  if (parts.some(part => !part || part === '.' || part === '..' || /[<>"|?*\x00-\x1f]/.test(part) || /[. ]$/.test(part)
      || /^(con|prn|aux|nul|com\d|lpt\d)(\.|$)/i.test(part))) throw new Error('Invalid output subdirectory');
  const root = await outputRoot();
  let target = root;
  for (const part of parts) {
    target = path.join(target, part);
    try { await fs.mkdir(target, { mode: 0o700 }); } catch (error) { if (error.code !== 'EEXIST') throw error; }
    const real = await rejectLinks(target);
    if (!within(root, real) || !(await fs.stat(real)).isDirectory()) throw new Error('Output path is outside the authorized directory');
  }
  return target;
}

function mime(bytes) {
  if (bytes.length >= 8 && bytes.subarray(0, 8).equals(Buffer.from([137,80,78,71,13,10,26,10]))) return 'image/png';
  if (bytes.length >= 3 && bytes[0] === 255 && bytes[1] === 216 && bytes[2] === 255) return 'image/jpeg';
  if (bytes.length >= 12 && bytes.toString('ascii', 0, 4) === 'RIFF' && bytes.toString('ascii', 8, 12) === 'WEBP') return 'image/webp';
  throw new Error('Only PNG, JPEG and WebP images are supported');
}

async function reference(value) {
  if (typeof value !== 'string' || !value.trim()) throw new Error('Each reference must be a non-empty string');
  if (/^(https?:\/\/|data:image\/|image:)/.test(value)) return value;
  const local = value.startsWith('file:') ? fileURLToPath(value) : value;
  if (!path.isAbsolute(local)) throw new Error('Local reference images need absolute paths');
  let configured;
  try { configured = JSON.parse(process.env.AGENT2API_IMAGE_REFERENCE_DIRS || '[]'); } catch { throw new Error('Invalid reference-directory configuration'); }
  if (!Array.isArray(configured) || configured.length > 16) throw new Error('Invalid reference-directory configuration');
  const roots = [await outputRoot()];
  for (const dir of configured) {
    if (typeof dir !== 'string' || !path.isAbsolute(dir)) throw new Error('Reference roots must be absolute directories');
    const selected = await rejectLinks(dir);
    if (!(await fs.stat(selected)).isDirectory()) throw new Error('Reference roots must be directories');
    roots.push(selected);
  }
  const target = await rejectLinks(local);
  if (!roots.some(root => within(root, target))) throw new Error('Reference image is outside the authorized directories');
  const handle = await fs.open(target, 'r');
  try {
    const stat = await handle.stat();
    if (!stat.isFile() || stat.size <= 0 || stat.size > 10 * 1024 * 1024) throw new Error('Reference image must be a regular file no larger than 10 MiB');
    const buffer = Buffer.alloc(stat.size);
    let offset = 0;
    while (offset < buffer.length) {
      const { bytesRead } = await handle.read(buffer, offset, buffer.length - offset, offset);
      if (!bytesRead) throw new Error('Reference image changed while reading');
      offset += bytesRead;
    }
    return 'data:' + mime(buffer) + ';base64,' + buffer.toString('base64');
  } finally { await handle.close(); }
}

async function saveImage(file, directory, signal) {
  if (!/^[a-f0-9]{64}$/.test(file.id || '')) throw new Error('Invalid image identifier');
  const url = new URL(endpoint());
  url.pathname = url.pathname.replace(/\/$/, '') + '/files/' + file.id;
  const response = await fetch(url, { headers: { authorization: 'Bearer ' + API_KEY }, redirect: 'error', signal });
  if (!response.ok) throw new Error('Image download failed with HTTP ' + response.status);
  const bytes = await readBody(response, MAX_IMAGE);
  const type = mime(bytes);
  if (type !== file.mime) throw new Error('Downloaded image type does not match the result');
  const extension = type === 'image/jpeg' ? 'jpg' : type === 'image/webp' ? 'webp' : 'png';
  const root = await outputRoot();
  const real = await rejectLinks(directory);
  if (!within(root, real)) throw new Error('Output directory changed');
  const target = path.join(real, 'image-' + file.id + '.' + extension);
  const temporary = path.join(real, '.agent2api-image-' + crypto.randomBytes(16).toString('hex') + '.tmp');
  let createdTemporary = false;
  try {
    const handle = await fs.open(temporary, 'wx', 0o600);
    createdTemporary = true;
    try { await handle.writeFile(bytes); await handle.sync(); }
    finally { await handle.close(); }
    if (await rejectLinks(directory) !== real) throw new Error('Output directory changed while saving');
    await fs.link(temporary, target);
  } catch (error) {
    if (error.code !== 'EEXIST' || !createdTemporary) throw error;
    const existing = await rejectLinks(target);
    if (!within(root, existing)) throw new Error('Existing image is outside the output directory');
    const stat = await fs.stat(existing);
    if (!stat.isFile() || stat.size !== bytes.length) throw new Error('A different file already exists; nothing was overwritten');
    const old = await fs.readFile(existing);
    if (!crypto.timingSafeEqual(crypto.createHash('sha256').update(old).digest(), crypto.createHash('sha256').update(bytes).digest())) {
      throw new Error('A different file already exists; nothing was overwritten');
    }
  } finally {
    if (createdTemporary) await fs.unlink(temporary).catch(() => {});
  }
  return { ...file, path: target, saved: true };
}

function adaptTools(tools) {
  return tools.map(tool => {
    if (!['generate_images', 'edit_images', 'get_image_result'].includes(tool.name)) return tool;
    const clone = structuredClone(tool);
    clone.description += ' This local bridge saves completed images into the administrator-authorized output directory and returns real file paths. It never overwrites different existing files.';
    clone.inputSchema.properties.output_subdir = { type: 'string', description: 'Optional relative subdirectory under the authorized output directory. No absolute paths or parent traversal.' };
    if (tool.name === 'get_image_result') clone.annotations = { ...clone.annotations, readOnlyHint: false, destructiveHint: false, idempotentHint: true };
    if (tool.name === 'edit_images') clone.inputSchema.properties.images.description += ' Local absolute paths are also accepted inside the authorized reference/output directories; the bridge reads them automatically.';
    return clone;
  });
}

async function callTool(name, args, signal) {
  if (!['generate_images', 'edit_images', 'get_image_result'].includes(name)) return failure('Unknown image tool');
  const originalId = args.request_id;
  const subdir = args.output_subdir ?? savedOutputs.get(args.job_id) ?? savedOutputs.get(originalId) ?? '';
  let directory;
  try { directory = await destination(subdir); }
  catch (error) { return failure(error.message, { request_id: originalId, job_id: args.job_id, submitted: false }); }
  const forwarded = { ...args };
  delete forwarded.output_subdir;
  if (typeof originalId === 'string' && /^[A-Za-z0-9_-]{8,128}$/.test(originalId)) {
    savedOutputs.set(originalId, subdir);
    while (savedOutputs.size > 1000) savedOutputs.delete(savedOutputs.keys().next().value);
  }
  try {
    if (name === 'edit_images' && Array.isArray(forwarded.images)) {
      const converted = [];
      let size = 0;
      for (const image of forwarded.images) {
        const value = await reference(image);
        size += value.length;
        if (size > 14 * 1024 * 1024) throw new Error('Reference images exceed the inline size limit');
        converted.push(value);
      }
      forwarded.images = converted;
    }
  } catch (error) { return failure(error.message, { request_id: originalId, submitted: false }); }
  let result;
  try { result = await remote('tools/call', { name, arguments: forwarded }, signal); }
  catch (error) {
    return failure('Submission or query response was lost: ' + error.message + '. Do not create another request_id; call get_image_result with the original request_id.',
      { request_id: originalId, job_id: args.job_id, status: 'uncertain' });
  }
  const data = resultData(result);
  if (data?.job_id) {
    savedOutputs.set(data.job_id, subdir);
    if (originalId) savedOutputs.set(originalId, subdir);
    while (savedOutputs.size > 1000) savedOutputs.delete(savedOutputs.keys().next().value);
  }
  if (!data || data.status !== 'succeeded' || !Array.isArray(data.images)) return result;
  const files = [];
  for (const file of data.images) {
    try { files.push(await saveImage(file, directory, signal)); }
    catch (error) { files.push({ ...file, saved: false, save_error: error.message }); }
  }
  const payload = { ...data, images: files, saved: files.every(file => file.saved),
    guidance: files.every(file => file.saved) ? 'Images are saved at the returned local paths.' : 'Images were generated, but some files were not saved. Query the same job to download again; never regenerate to retry saving.' };
  const content = [{ type: 'text', text: JSON.stringify(payload) }, ...(result.content || []).filter(item => item.type === 'image')];
  return { ...result, content, structuredContent: payload };
}

async function dispatch(message) {
  if (!message || message.jsonrpc !== '2.0' || typeof message.method !== 'string') return { jsonrpc: '2.0', id: null, error: { code: -32600, message: 'Invalid request' } };
  if (message.id === undefined || message.method.startsWith('notifications/')) return null;
  const id = message.id;
  if (active.size >= 8) return { jsonrpc: '2.0', id, result: failure('Too many active bridge requests; no new task was submitted') };
  const controller = new AbortController();
  active.add(controller);
  const timer = setTimeout(() => controller.abort(), 90000);
  try {
    let result;
    const params = message.params || {};
    if (message.method === 'initialize') {
      result = { protocolVersion: versions.includes(params.protocolVersion) ? params.protocolVersion : versions[0],
        capabilities: { tools: {} }, serverInfo: { name: 'agent2api-image-generation', version: '1.0.0' },
        instructions: 'Use unique request_id values and reuse them after timeouts. Query unfinished jobs instead of generating again. Files are restricted to administrator-authorized directories.' };
    } else if (message.method === 'ping') result = {};
    else if (message.method === 'tools/list') {
      const listed = await remote('tools/list', {}, controller.signal);
      result = { tools: adaptTools(listed.tools || []) };
    } else if (message.method === 'tools/call') result = await callTool(params.name, params.arguments || {}, controller.signal);
    else return { jsonrpc: '2.0', id, error: { code: -32601, message: 'Method not found' } };
    return { jsonrpc: '2.0', id, result };
  } catch (error) { return { jsonrpc: '2.0', id, result: failure(error.message) }; }
  finally { clearTimeout(timer); active.delete(controller); }
}

let pending = Buffer.alloc(0);
process.stdin.on('data', chunk => {
  pending = Buffer.concat([pending, chunk]);
  while (true) {
    const end = pending.indexOf(10);
    if (end < 0) break;
    const line = pending.subarray(0, end);
    pending = pending.subarray(end + 1);
    if (line.length > MAX_INPUT) { process.stderr.write('Image MCP input exceeds 16 MiB\n'); process.exit(1); }
    if (!line.toString('utf8').trim()) continue;
    let message;
    try { message = JSON.parse(line.toString('utf8')); }
    catch { process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: null, error: { code: -32700, message: 'Parse error' } }) + '\n'); continue; }
    void dispatch(message).then(result => { if (result) process.stdout.write(JSON.stringify(result) + '\n'); });
  }
  if (pending.length > MAX_INPUT) { process.stderr.write('Image MCP input exceeds 16 MiB\n'); process.exit(1); }
});
process.stdin.on('end', () => { for (const controller of active) controller.abort(); process.exit(0); });

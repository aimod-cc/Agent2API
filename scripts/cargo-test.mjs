#!/usr/bin/env node
import { spawn } from 'node:child_process'
import { mkdtempSync, realpathSync, rmSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const cwd = path.join(root, 'desktop-tauri', 'src-tauri')
const argv = process.argv.slice(2)
const plugins = argv[0] === '--plugins'
const coverage = argv[0] === '--coverage'
if (plugins || coverage) argv.shift()
if (plugins && !process.env.AGENT2API_TEST_PLUGIN_EXE) {
  console.error('插件联调需要 AGENT2API_TEST_PLUGIN_EXE 指向含 demo 的测试插件。')
  process.exit(1)
}
let executable = process.env.AGENT2API_TEST_PLUGIN_EXE
if (plugins) {
  try {
    executable = realpathSync(executable)
    if (!statSync(executable).isFile()) throw new Error('路径不是文件')
  } catch (error) {
    console.error(`测试插件不可用：${executable}（${error.message}）`)
    process.exit(1)
  }
}
const home = mkdtempSync(path.join(tmpdir(), 'agent2api-cargo-'))
const env = Object.fromEntries(Object.entries(process.env).filter(([key]) =>
  !/^(AGENT2API_|WORKBUDDY_)/i.test(key)))
env.AGENT2API_PROXY_HOME = home
env.WORKBUDDY_PROXY_HOME = home
env.NO_PROXY = 'localhost,127.0.0.1,::1'
env.no_proxy = env.NO_PROXY
if (plugins) env.AGENT2API_TEST_PLUGIN_EXE = executable
const args = coverage
  ? ['llvm-cov', '--locked', '-p', 'agent2api-server', '--lib', ...argv]
  : ['test', '--locked', '-p', 'agent2api-server', ...(plugins
      ? ['--lib', 'server::core::plugin_host::tests::', '--', '--ignored', ...argv]
      : argv)]
console.log(plugins ? '运行插件联调（使用独立临时目录）' : '运行服务端测试（使用独立临时目录）')
const child = spawn('cargo', args, { cwd, env, stdio: 'inherit' })
let cleaned = false
const cleanup = () => {
  if (cleaned) return
  cleaned = true
  try { rmSync(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }) }
  catch (error) { console.error(`临时目录未完全清理：${home}（${error.message}）`) }
}
child.once('error', error => {
  console.error(`无法启动 cargo：${error.message}`)
  cleanup()
  process.exitCode = 1
})
child.once('close', code => {
  cleanup()
  process.exitCode = code ?? 1
})
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.once(signal, () => child.kill(signal))
}

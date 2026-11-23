#!/usr/bin/env node
/**
 * Agent2Providers（普通插件）与 Agent2WebProviders（网页插件）单包 WASM 发版助手。
 *
 * 解决发版中的四个核心痛点：
 *   1. 自动扫描两个插件仓，识别真正改过源码/共享依赖/跨插件引用的包；
 *   2. 对比宿主机稳定版能力清单，提示依赖新版主程序宿主能力的插件；
 *   3. 同步更新 pack.json + Cargo.toml，并自动刷新与校验 wasm/Cargo.lock；
 *   4. 推送标签时按「每批最多 3 个」自动分批推送（避开 GitHub 超 3 个标签跳过触发 CI 的限制）。
 *
 * 用法：
 *   node scripts/release-plugins.mjs [status] [--all] [--repo=standard|web|all]
 *   node scripts/release-plugins.mjs bump antigravity=next-beta deepseek-web=0.1.0-beta.4 [--dry-run] [--skip-check]
 *   node scripts/release-plugins.mjs push-tags [--repo=standard|web|all] [--batch-size=3] [--yes]
 *   node scripts/release-plugins.mjs push-tags wasm-antigravity-v0.1.0-beta.3 --yes
 *   node scripts/release-plugins.mjs verify [agent2api-wasm-antigravity ...]
 */
import { existsSync, readdirSync, readFileSync } from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import {
  chunkArray,
  compareSemver,
  envForGhAccount,
  gitAuthHeaderArgs,
  parseCliArgs,
  readCargoPackageVersion,
  resolveGitProxy,
  resolveTargetVersion,
  runSpawn,
  updateCargoPackageVersion,
  updateJsonTopVersion,
} from './lib/release-utils.mjs'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const MAX_GITHUB_TAG_BATCH = 3

const PLUGIN_REPOS = [
  {
    key: 'standard',
    label: '普通插件仓（Agent2Providers）',
    dir: path.join(ROOT, 'plugin', 'Agent2Providers'),
    github: 'ProvidersPlugin/Agent2Providers',
    pushUser: 'aimodcc',
    expectedCategory: 'agent',
  },
  {
    key: 'web',
    label: '网页插件仓（Agent2WebProviders）',
    dir: path.join(ROOT, 'plugin', 'Agent2WebProviders'),
    github: 'ProvidersPlugin/Agent2WebProviders',
    pushUser: 'aimodcc',
    expectedCategory: 'web',
  },
]

const BUILD_RELEASES_REPO = 'Agent2Builder/agent2api-providers-releases'
const BUILD_USER = 'Agent2Builder'
const CDN_CATALOG_URL = 'https://cdn.agent2api.dev/agent2api/feeds/plugins/catalog.json'

function gitOut(cwd, args) {
  const res = runSpawn('git', args, { cwd, capture: true })
  return res.status === 0 ? res.stdout.trim() : ''
}

function repoRepoSelect(repoFlag) {
  if (!repoFlag || repoFlag === 'all') return PLUGIN_REPOS
  const found = PLUGIN_REPOS.filter(r => r.key === repoFlag)
  if (found.length === 0) throw new Error(`未知插件仓库过滤值：${repoFlag}（可用：standard / web / all）`)
  return found
}

function extractCapabilitiesFromRust(text) {
  const block = /pub\s+const\s+CAPABILITIES\s*:\s*&\[&str\]\s*=\s*&\[([\s\S]*?)\];/.exec(text)
  if (!block) return new Set()
  const out = new Set()
  for (const m of block[1].matchAll(/"([^"]+)"/g)) out.add(m[1])
  return out
}

function loadHostCapabilityContext() {
  const rel = 'desktop-tauri/src-tauri/server/src/server/core/plugin_host/wasm/manifest.rs'
  const abs = path.join(ROOT, rel)
  const headCaps = existsSync(abs) ? extractCapabilitiesFromRust(readFileSync(abs, 'utf8')) : new Set()
  const tagsRaw = gitOut(ROOT, ['tag', '--merged', 'HEAD', '--list', 'v*'])
  const allTags = tagsRaw
    .split(/\r?\n/)
    .map(s => s.trim())
    .filter(s => /^v\d+\.\d+\.\d+/.test(s))
    .sort(compareSemver)
  const latestTag = allTags[allTags.length - 1] || ''
  const prevTag = allTags[allTags.length - 2] || ''

  const capsAtTag = tag => {
    if (!tag) return null
    const oldText = gitOut(ROOT, ['show', `${tag}:${rel}`])
    return oldText ? extractCapabilitiesFromRust(oldText) : null
  }
  const latestCaps = capsAtTag(latestTag)
  const prevCaps = capsAtTag(prevTag)
  const unreleasedCaps = latestCaps ? new Set([...headCaps].filter(c => !latestCaps.has(c))) : new Set()
  const recentCaps = prevCaps ? new Set([...headCaps].filter(c => !prevCaps.has(c))) : new Set()
  return { headCaps, latestTag, prevTag, unreleasedCaps, recentCaps }
}

function walkRsFiles(dir) {
  if (!existsSync(dir)) return []
  const files = []
  for (const item of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, item.name)
    if (item.isDirectory()) files.push(...walkRsFiles(full))
    else if (item.isFile() && item.name.endsWith('.rs')) files.push(full)
  }
  return files
}

function collectSharedPaths(repoDir, pluginDir) {
  const refs = new Map()
  const addRef = targetAbs => {
    const rel = path.relative(repoDir, targetAbs).split('\\').join('/')
    if (rel.startsWith('..')) {
      refs.set(`cross:${rel}`, { crossRepo: true, path: rel })
      return
    }
    const parts = rel.split('/')
    if (parts[0] === 'wasm' && parts[1] && parts[1] !== path.basename(pluginDir)) {
      const modPath = `wasm/${parts[1]}`
      refs.set(`local:${modPath}`, { crossRepo: false, path: modPath })
    }
  }

  const cargoText = readFileSync(path.join(pluginDir, 'Cargo.toml'), 'utf8')
  for (const m of cargoText.matchAll(/path\s*=\s*"([^"]+)"/g)) {
    addRef(path.resolve(pluginDir, m[1]))
  }
  for (const rs of walkRsFiles(path.join(pluginDir, 'src'))) {
    const text = readFileSync(rs, 'utf8')
    for (const m of text.matchAll(/#\[\s*path\s*=\s*"([^"]+)"\s*\]|include_(?:str|bytes)!\(\s*"([^"]+)"\s*\)/g)) {
      const target = m[1] || m[2]
      if (target) addRef(path.resolve(path.dirname(rs), target))
    }
  }
  return [...refs.values()]
}

function discoverPackages(repo) {
  const wasmDir = path.join(repo.dir, 'wasm')
  if (!existsSync(wasmDir)) return []
  const list = []
  for (const entry of readdirSync(wasmDir, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue
    const pluginDir = path.join(wasmDir, entry.name)
    const packPath = path.join(pluginDir, 'pack.json')
    const cargoPath = path.join(pluginDir, 'Cargo.toml')
    const wasmPath = path.join(pluginDir, 'wasm.json')
    if (!existsSync(packPath) || !existsSync(cargoPath)) continue

    const pack = JSON.parse(readFileSync(packPath, 'utf8'))
    const cargoVer = readCargoPackageVersion(cargoPath)
    const wasm = existsSync(wasmPath) ? JSON.parse(readFileSync(wasmPath, 'utf8')) : { requires: [] }
    const sharedRefs = collectSharedPaths(repo.dir, pluginDir)
    list.push({
      id: entry.name,
      repo,
      dir: pluginDir,
      packPath,
      cargoPath,
      wasmPath,
      pack,
      version: pack.version,
      cargoVer,
      requires: wasm.requires || [],
      sharedRefs,
    })
  }
  return list.sort((a, b) => a.id.localeCompare(b.id))
}

function latestBatchTag(repoDir) {
  const raw = gitOut(repoDir, ['tag', '--merged', 'HEAD', '--list', 'v*'])
  if (!raw) return ''
  const tags = raw
    .split(/\r?\n/)
    .map(s => s.trim())
    .filter(s => /^v\d+\.\d+\.\d+/.test(s))
    .sort(compareSemver)
  return tags[tags.length - 1] || ''
}

function resolveBaselineTag(repoDir, pluginId, fallbackBatchTag) {
  const prefix = `wasm-${pluginId}-v`
  const raw = gitOut(repoDir, ['tag', '--list', `${prefix}*`])
  const tags = (raw ? raw.split(/\r?\n/) : [])
    .map(s => s.trim())
    .filter(s => s.startsWith(prefix))
    .sort((a, b) => compareSemver(a.slice(prefix.length), b.slice(prefix.length)))
  if (tags.length > 0) {
    return { tag: tags[tags.length - 1], kind: 'single' }
  }
  if (fallbackBatchTag) {
    const existsInBatch = runSpawn('git', ['cat-file', '-e', `${fallbackBatchTag}:wasm/${pluginId}/pack.json`], {
      cwd: repoDir,
      capture: true,
    })
    if (existsInBatch.status === 0) {
      return { tag: fallbackBatchTag, kind: 'batch' }
    }
  }
  return { tag: '', kind: 'none' }
}

function inspectPackageState(pkg, hostCtx, stdRepoDir, fallbackBatchTag) {
  const { id, repo } = pkg
  const issues = []
  if (pkg.pack.id !== `agent2api-wasm-${id}`) issues.push(`pack.json id=${pkg.pack.id}`)
  if (pkg.pack.version !== pkg.cargoVer) issues.push(`版本不一致（pack=${pkg.pack.version}, Cargo=${pkg.cargoVer}）`)
  const cat = pkg.pack.provider_category || 'agent'
  if (cat !== repo.expectedCategory) issues.push(`provider_category=${cat} 与仓库不符`)
  for (const reqCap of pkg.requires) {
    if (hostCtx.unreleasedCaps.has(reqCap)) {
      issues.push(`依赖主程序未发布的新宿主能力 ${reqCap}（${hostCtx.latestTag || '上版'} 尚不支持，需随新版主程序一并发）`)
    } else if (hostCtx.recentCaps.has(reqCap)) {
      issues.push(`使用近期新增宿主能力 ${reqCap}（${hostCtx.latestTag} 起支持，旧版客户端无法加载）`)
    }
  }

  const baseline = resolveBaselineTag(repo.dir, id, fallbackBatchTag)
  const ownRel = `wasm/${id}`
  const dirtyLines = gitOut(repo.dir, ['status', '--porcelain', '--', ownRel])
    .split(/\r?\n/)
    .filter(Boolean)

  let ownChanged = []
  const sharedLocalChanged = []
  let sdkChanged = false

  if (baseline.tag) {
    const rawDiff = gitOut(repo.dir, ['diff', '--name-only', `${baseline.tag}..HEAD`, '--', ownRel])
    ownChanged = rawDiff ? rawDiff.split(/\r?\n/).filter(Boolean) : []
    const tagTime = Number(gitOut(repo.dir, ['log', '-1', '--format=%ct', baseline.tag]) || 0)
    for (const ref of pkg.sharedRefs) {
      if (ref.crossRepo) {
        if (tagTime > 0 && stdRepoDir) {
          sdkChanged = Boolean(gitOut(stdRepoDir, ['log', `--after=${tagTime}`, '--oneline', '-n', '1', '--', 'wasm/sdk']))
        }
      } else {
        const hit = gitOut(repo.dir, ['diff', '--name-only', `${baseline.tag}..HEAD`, '--', ref.path])
        if (hit) sharedLocalChanged.push(ref.path)
      }
    }
  }

  return { pkg, baseline, dirtyLines, ownChanged, sharedLocalChanged, sdkChanged, issues }
}

function runStatus(flags) {
  const repos = repoRepoSelect(flags.get('repo'))
  const showAll = Boolean(flags.get('all'))
  const hostCtx = loadHostCapabilityContext()

  const stdRepo = PLUGIN_REPOS[0]
  const stdBranch = gitOut(stdRepo.dir, ['rev-parse', '--abbrev-ref', 'HEAD'])
  const stdAhead = Number(gitOut(stdRepo.dir, ['rev-list', '--count', `origin/${stdBranch}..HEAD`]) || 0)

  for (const repo of repos) {
    const branch = gitOut(repo.dir, ['rev-parse', '--abbrev-ref', 'HEAD'])
    const dirtyAll = gitOut(repo.dir, ['status', '--porcelain']).split(/\r?\n/).filter(Boolean)
    const ahead = Number(gitOut(repo.dir, ['rev-list', '--count', `origin/${branch}..HEAD`]) || 0)
    const batchFallback = latestBatchTag(repo.dir)
    console.log(`\n=== ${repo.label}（分支：${branch}，领先远端 ${ahead} 提交，未提交项 ${dirtyAll.length}）===`)
    if (repo.key === 'web' && stdAhead > 0) {
      console.log(`⚠️  注意：普通插件仓（${stdBranch}）尚有 ${stdAhead} 个本地提交未推到远端；网页插件依赖其 SDK 时请先推普通插件仓！`)
    }

    const pkgs = discoverPackages(repo)
    let cleanCount = 0
    for (const pkg of pkgs) {
      const st = inspectPackageState(pkg, hostCtx, repo.key === 'web' ? stdRepo.dir : null, batchFallback)
      const hasChange = st.dirtyLines.length > 0 || st.ownChanged.length > 0 || st.sharedLocalChanged.length > 0 || st.sdkChanged || st.baseline.kind === 'none' || st.issues.length > 0
      if (!hasChange && !showAll) {
        cleanCount += 1
        continue
      }
      const notes = []
      if (st.baseline.kind === 'none') notes.push('尚未在任何发布 tag 中出现')
      if (st.dirtyLines.length > 0) notes.push(`工作区未提交(${st.dirtyLines.length})`)
      if (st.ownChanged.length > 0) notes.push(`自有文件改动(${st.ownChanged.length})`)
      if (st.sharedLocalChanged.length > 0) notes.push(`共享依赖改动(${st.sharedLocalChanged.join(', ')})`)
      if (st.sdkChanged) notes.push('上游 sdk 自上次发版后有改动')
      if (st.issues.length > 0) notes.push(`⚠️ ${st.issues.join('；')}`)
      const baseLabel = st.baseline.kind === 'batch' ? `批量 ${st.baseline.tag}` : (st.baseline.tag || '无')
      const statusText = notes.length > 0 ? notes.join(' | ') : '✓ 自上次发版 tag 后无改动'
      console.log(`  - ${pkg.id.padEnd(16, ' ')} v${pkg.version.padEnd(13, ' ')} (基准: ${baseLabel}) → ${statusText}`)
    }
    if (!showAll && cleanCount > 0) {
      console.log(`  （另有 ${cleanCount} 个插件自上次发版 tag 后无改动，加 --all 可全部列出）`)
    }
  }
}

function runBump(positional, flags) {
  const specs = positional.slice(1)
  if (specs.length === 0) {
    throw new Error('请指定至少一个插件版本目标，例如：antigravity=next-beta 或 deepseek-web=0.1.0-beta.4')
  }
  const repos = repoRepoSelect(flags.get('repo'))
  const byId = new Map()
  for (const repo of repos) {
    for (const pkg of discoverPackages(repo)) byId.set(pkg.id, pkg)
  }

  const dryRun = Boolean(flags.get('dry-run'))
  const touchedRepos = new Set()
  for (const item of specs) {
    const eq = item.indexOf('=')
    if (eq === -1) throw new Error(`格式应为 <插件名>=<目标版本或 next-beta|patch|minor|graduate>：收到 ${item}`)
    const id = item.slice(0, eq).trim()
    const spec = item.slice(eq + 1).trim()
    const pkg = byId.get(id)
    if (!pkg) throw new Error(`找不到插件：${id}`)
    const nextVer = resolveTargetVersion(pkg.version, spec)
    if (dryRun) {
      console.log(`  [预演] ${pkg.repo.key}/${id}：${pkg.version} → ${nextVer}（tag: wasm-${id}-v${nextVer}）`)
      continue
    }
    updateJsonTopVersion(pkg.packPath, nextVer)
    updateCargoPackageVersion(pkg.cargoPath, nextVer)
    touchedRepos.add(pkg.repo)
    console.log(`  ✓ ${pkg.repo.key}/${id}：${pkg.version} → ${nextVer}（同步改好 pack.json 与 Cargo.toml）`)
  }

  if (dryRun || flags.has('skip-check')) return
  for (const repo of touchedRepos) {
    console.log(`\n→ 在 ${repo.label} 刷新 wasm/Cargo.lock 并校验编译…`)
    const refresh = runSpawn(
      'cargo',
      ['check', '--manifest-path', 'wasm/Cargo.toml', '--workspace', '--target', 'wasm32-unknown-unknown'],
      { cwd: repo.dir },
    )
    if (refresh.status !== 0) throw new Error(`${repo.label} 刷新 Cargo.lock 失败`)
    const locked = runSpawn(
      'cargo',
      ['check', '--manifest-path', 'wasm/Cargo.toml', '--workspace', '--target', 'wasm32-unknown-unknown', '--locked'],
      { cwd: repo.dir },
    )
    if (locked.status !== 0) throw new Error(`${repo.label} --locked 校验失败`)
  }
  console.log('\n✅ 插件版本号与 Cargo.lock 更新完毕！请在各自目录提交更新日志、打 tag 并使用 push-tags 分批推送。')
}

function remoteTagsSet(repo, authEnv) {
  const args = [
    ...gitAuthHeaderArgs(authEnv.GH_TOKEN),
    'ls-remote',
    '--tags',
    `https://github.com/${repo.github}.git`,
    'refs/tags/wasm-*-v*',
  ]
  const res = runSpawn('git', args, { cwd: repo.dir, env: authEnv, capture: true })
  if (res.status !== 0) throw new Error(`读取 ${repo.github} 远端标签失败：${res.stderr}`)
  const out = new Set()
  for (const line of res.stdout.split(/\r?\n/)) {
    const m = /refs\/tags\/([^\s^]+)$/.exec(line.trim())
    if (m) out.add(m[1])
  }
  return out
}

function runPushTags(positional, flags) {
  const explicitTags = positional.slice(1)
  const repos = repoRepoSelect(flags.get('repo'))
  const rawBatch = Number(flags.get('batch-size') ?? MAX_GITHUB_TAG_BATCH)
  const batchSize = Math.min(MAX_GITHUB_TAG_BATCH, Math.max(1, Number.isFinite(rawBatch) ? rawBatch : MAX_GITHUB_TAG_BATCH))
  const confirmed = Boolean(flags.get('yes'))
  const pushUser = typeof flags.get('user') === 'string' ? flags.get('user') : 'aimodcc'
  const authEnv = envForGhAccount(pushUser)

  for (const repo of repos) {
    const branch = gitOut(repo.dir, ['rev-parse', '--abbrev-ref', 'HEAD'])
    const localWasmTags = gitOut(repo.dir, ['tag', '--merged', 'HEAD', '--list', 'wasm-*-v*'])
      .split(/\r?\n/)
      .map(s => s.trim())
      .filter(Boolean)

    let targetTags
    if (explicitTags.length > 0) {
      const localSet = new Set(localWasmTags)
      targetTags = explicitTags.filter(t => localSet.has(t))
    } else {
      const remoteSet = remoteTagsSet(repo, authEnv)
      targetTags = localWasmTags.filter(t => !remoteSet.has(t))
    }

    const aheadCount = Number(gitOut(repo.dir, ['rev-list', '--count', `origin/${branch}..HEAD`]) || 0)
    if (aheadCount === 0 && targetTags.length === 0) {
      console.log(`→ ${repo.label}：分支与单包 tag 均已与远端同步`)
      continue
    }

    const batches = chunkArray(targetTags, batchSize)
    console.log(`\n=== ${repo.label}（分支：${branch}，领先 ${aheadCount} 提交，待推 tag ${targetTags.length} 个，分 ${batches.length} 批，每批 ≤ ${batchSize}）===`)
    for (const [idx, batch] of batches.entries()) {
      console.log(`  第 ${idx + 1}/${batches.length} 批：${batch.join(' ')}`)
    }
    if (!confirmed) {
      console.log('ℹ️  当前为预览模式（未加 --yes），不执行任何远端推送。确认后再加 --yes。')
      continue
    }

    const repoEnv = envForGhAccount(repo.pushUser || pushUser)
    const remoteUrl = `https://github.com/${repo.github}.git`
    if (aheadCount > 0) {
      console.log(`  → 先推送分支 ${branch}…`)
      const pushBranchRes = runSpawn('git', [...gitAuthHeaderArgs(repoEnv.GH_TOKEN), 'push', remoteUrl, branch], {
        cwd: repo.dir,
        env: repoEnv,
      })
      if (pushBranchRes.status !== 0) throw new Error(`推送 ${repo.github} 分支 ${branch} 失败`)
    }
    for (const [idx, batch] of batches.entries()) {
      console.log(`  → 推送第 ${idx + 1}/${batches.length} 批标签…`)
      const pushTagRes = runSpawn('git', [...gitAuthHeaderArgs(repoEnv.GH_TOKEN), 'push', remoteUrl, ...batch], {
        cwd: repo.dir,
        env: repoEnv,
      })
      if (pushTagRes.status !== 0) throw new Error(`推送标签批次 ${idx + 1} 失败`)
    }
  }
}

function fetchJsonViaCurl(url, env) {
  const res = runSpawn('curl', ['-fsSL', url], { env, capture: true })
  if (res.status !== 0) throw new Error(`请求 ${url} 失败：${res.stderr || res.status}`)
  return JSON.parse(res.stdout)
}

function runVerify(positional) {
  const filters = positional.slice(1)
  const builderEnv = envForGhAccount(BUILD_USER)
  const proxyEnv = { ...process.env }
  const proxy = resolveGitProxy()
  if (proxy) proxyEnv.HTTPS_PROXY = proxy

  console.log(`=== 插件构建仓库最近工作流（${BUILD_RELEASES_REPO}）===`)
  runSpawn('gh', ['run', 'list', '--repo', BUILD_RELEASES_REPO, '--limit', '12'], { env: builderEnv })

  console.log('\n=== CDN 插件目录对比（直连 vs 避开缓存参数）===')
  const directCat = fetchJsonViaCurl(CDN_CATALOG_URL, proxyEnv)
  const freshCat = fetchJsonViaCurl(`${CDN_CATALOG_URL}?t=${Date.now()}`, proxyEnv)
  console.log(`直连 CDN：revision=${directCat.revision} updated=${directCat.updated}`)
  console.log(`刷新缓存：revision=${freshCat.revision} updated=${freshCat.updated}`)
  if (directCat.revision !== freshCat.revision) {
    console.log('⚠️  直连 CDN 与最新目录 revision 不同，说明边缘缓存仍在短暂刷新窗口内。')
  }

  const freshById = new Map((freshCat.plugins || []).map(p => [p.id, p]))
  const targets = filters.length > 0 ? filters : [...freshById.keys()].slice(0, 12)
  for (const rawTarget of targets) {
    const id = rawTarget.startsWith('agent2api-wasm-') ? rawTarget : `agent2api-wasm-${rawTarget}`
    const plugin = freshById.get(id)
    if (!plugin) {
      console.log(`  - ${id}：⚠️ 未出现在目录中`)
      continue
    }
    const tag = plugin.release?.tag_name || '无'
    const assets = (plugin.release?.assets || []).map(a => a.name).join(', ')
    console.log(`  - ${id} → ${tag} | 附件：${assets}`)
  }
}

function main() {
  const { positional, flags } = parseCliArgs(process.argv.slice(2))
  const cmd = flags.has('help') ? 'help' : (positional[0] || 'status')
  if (cmd === 'help') {
    console.log('子命令：status（默认） | bump <id=ver...> | push-tags [tags...] [--yes] | verify [ids...]')
    return
  }
  if (cmd === 'status') return runStatus(flags)
  if (cmd === 'bump') return runBump(positional, flags)
  if (cmd === 'push-tags') return runPushTags(positional, flags)
  if (cmd === 'verify') return runVerify(positional)
  throw new Error(`未知子命令：${cmd}`)
}

try {
  main()
} catch (error) {
  console.error(`❌ ${error.message}`)
  process.exit(1)
}

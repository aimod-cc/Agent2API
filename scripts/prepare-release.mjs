#!/usr/bin/env node
/**
 * Agent2API 主程序发版检查与准备脚本（只改本地工作区与刷新构建产物，绝不自动提交/打 tag/推送）。
 *
 * 用法：
 *   node scripts/prepare-release.mjs                        # 仅检查当前 6+2 处版本、工作区与未发布提交
 *   node scripts/prepare-release.mjs 3.5.0-beta.8           # 更新 6+2 处版本，刷新 Cargo.lock 与手机端产物
 *   node scripts/prepare-release.mjs next-beta --dry-run    # 预演 beta 序号递增，不写入文件
 *   node scripts/prepare-release.mjs graduate               # 预发布转正（3.5.0-beta.7 → 3.5.0，应在 main 分支）
 *   node scripts/prepare-release.mjs patch                  # 正式版末位 +1（3.5.0 → 3.5.1，应在 main 分支）
 *
 * 常用开关：
 *   --dry-run        仅展示将修改的版本位置，不改文件、不跑构建
 *   --skip-checks    改完版本后跳过 cargo check 与 npm run mobile:build
 *   --full-frontend  额外重编 ui-islands 与 i18n（等同 CI 的完整前端同源重建）
 *   --allow-branch   忽略「正式版在 main、beta 版在 beta」的分支校验
 *   --force          工作区存在未提交改动时也继续修改版本号
 */
import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import {
  compareSemver,
  execText,
  parseCliArgs,
  parseSemver,
  readCargoPackageVersion,
  resolveTargetVersion,
  runSpawn,
  updateCargoPackageVersion,
  updateJsonTopVersion,
  updateNpmLockVersion,
} from './lib/release-utils.mjs'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const IGNORED_UNTRACKED = new Set(['.antigravitycli', '.antigravitycli/'])

const VERSION_TARGETS = [
  { id: '1. 根 package.json', rel: 'package.json', type: 'json' },
  { id: '2. 桌面端 package.json', rel: 'desktop-tauri/package.json', type: 'json' },
  { id: '3. 桌面端 Cargo.toml', rel: 'desktop-tauri/src-tauri/Cargo.toml', type: 'cargo' },
  { id: '4. Tauri 配置 tauri.conf.json', rel: 'desktop-tauri/src-tauri/tauri.conf.json', type: 'json' },
  { id: '5. 服务端 Cargo.toml', rel: 'desktop-tauri/src-tauri/server/Cargo.toml', type: 'cargo' },
  { id: '6. 手机端 package.json', rel: 'desktop-tauri/mobile/package.json', type: 'json' },
  { id: '6b. 手机端 package-lock.json', rel: 'desktop-tauri/mobile/package-lock.json', type: 'npm-lock' },
]

function readCurrentVersions() {
  return VERSION_TARGETS.map(item => {
    const abs = path.join(ROOT, item.rel)
    if (item.type === 'cargo') {
      return { ...item, abs, current: readCargoPackageVersion(abs) }
    }
    const parsed = JSON.parse(readFileSync(abs, 'utf8'))
    if (item.type === 'npm-lock') {
      const top = parsed.version
      const rootPkg = parsed.packages?.['']?.version
      return { ...item, abs, current: top === rootPkg ? top : `${top} / packages[""]=${rootPkg}`, mismatchLock: top !== rootPkg }
    }
    return { ...item, abs, current: parsed.version }
  })
}

function listGitStatus() {
  const res = runSpawn('git', ['status', '--porcelain'], { cwd: ROOT, capture: true })
  if (res.status !== 0) throw new Error('执行 git status 失败')
  const raw = res.stdout.replace(/\r?\n$/, '')
  if (!raw) return { dirty: [], ignoredNoise: [] }
  const dirty = []
  const ignoredNoise = []
  for (const line of raw.split(/\r?\n/)) {
    if (!line.trim()) continue
    const filePath = line.slice(3).trim()
    if (IGNORED_UNTRACKED.has(filePath) || filePath.startsWith('docs/release-notes/')) {
      ignoredNoise.push(line)
    } else {
      dirty.push(line)
    }
  }
  return { dirty, ignoredNoise }
}

function getLatestTagOnHead() {
  const res = runSpawn('git', ['tag', '--merged', 'HEAD', '--list', 'v*'], { cwd: ROOT, capture: true })
  if (res.status !== 0) return ''
  const tags = res.stdout.split(/\r?\n/).map(s => s.trim()).filter(Boolean)
  tags.sort((a, b) => compareSemver(a, b))
  return tags[tags.length - 1] || ''
}

function readBuildRepoWiring() {
  const buildYml = readFileSync(path.join(ROOT, '.github/workflows/build.yml'), 'utf8')
  const versionRs = readFileSync(path.join(ROOT, 'desktop-tauri/src-tauri/server/src/server/core/update/version.rs'), 'utf8')
  const pluginUpdateRs = readFileSync(path.join(ROOT, 'desktop-tauri/src-tauri/server/src/server/core/plugin_host/update.rs'), 'utf8')
  const serverCargo = readFileSync(path.join(ROOT, 'desktop-tauri/src-tauri/server/Cargo.toml'), 'utf8')

  const workflowRepo = /repository:\s*([^\s#]+)/.exec(buildYml)?.[1] || '?'
  const appDefaultRepo = /DEFAULT_REPO[^"]*"([^"]+)"/.exec(versionRs)?.[1] || '?'
  const pluginFallbackRepo = /FALLBACK_REPO[^"]*"([^"]+)"/.exec(pluginUpdateRs)?.[1] || '?'
  const protoTag = /agent2api-provider-protocol\s*=\s*\{[^\n]*tag\s*=\s*"([^"]+)"/.exec(serverCargo)?.[1] || '?'
  return { workflowRepo, appDefaultRepo, pluginFallbackRepo, protoTag }
}

function printStatusReport() {
  const branch = execText('git', ['rev-parse', '--abbrev-ref', 'HEAD'], { cwd: ROOT })
  const rows = readCurrentVersions()
  const unique = new Set(rows.map(r => r.current))
  const { dirty, ignoredNoise } = listGitStatus()
  const lastTag = getLatestTagOnHead()
  const wiring = readBuildRepoWiring()

  console.log(`=== Agent2API 主程序发版自检（当前分支：${branch}）===`)
  for (const row of rows) {
    console.log(`  - ${row.id.padEnd(28, ' ')} ${row.current}`)
  }
  console.log(unique.size === 1 ? '✅ 6+2 处版本号完全一致' : '⚠️  存在不一致的版本号，准备发版前需同步')
  console.log(`→ 构建派发目标：${wiring.workflowRepo} | 客户端兜底：${wiring.appDefaultRepo} | 插件兜底：${wiring.pluginFallbackRepo} | 协议 tag：${wiring.protoTag}`)

  if (dirty.length === 0) {
    console.log('✅ 工作区干净（无未提交源码改动）')
  } else {
    console.log(`⚠️  工作区有 ${dirty.length} 项未提交改动：`)
    for (const line of dirty.slice(0, 15)) console.log(`     ${line}`)
    if (dirty.length > 15) console.log(`     … 另有 ${dirty.length - 15} 项`)
  }
  if (ignoredNoise.length > 0) {
    console.log(`ℹ️  已忽略本地目录/草稿：${ignoredNoise.join(' ; ')}`)
  }

  if (lastTag) {
    const commitsRaw = execText('git', ['log', `${lastTag}..HEAD`, '--oneline', '--no-merges'], { cwd: ROOT })
    const commits = commitsRaw ? commitsRaw.split(/\r?\n/) : []
    console.log(`\n→ 自上一个主程序 tag（${lastTag}）以来的提交（共 ${commits.length} 条）：`)
    for (const c of commits.slice(0, 25)) console.log(`   ${c}`)
    if (commits.length > 25) console.log(`   … 另有 ${commits.length - 25} 条`)
    const docTouchRaw = execText('git', ['diff', '--name-only', `${lastTag}..HEAD`, '--', 'docs/install*.md', 'scripts/', '.github/workflows/'], { cwd: ROOT })
    if (docTouchRaw) {
      console.log(`ℹ️  自 ${lastTag} 起涉及脚本/安装文档/工作流的改动：${docTouchRaw.split(/\r?\n/).join(', ')}`)
    }
  }

  const curVer = rows[0].current
  const draftPath = path.join(ROOT, 'docs', 'release-notes', `${curVer}.md`)
  console.log(existsSync(draftPath)
    ? `ℹ️  已存在日志草稿：docs/release-notes/${curVer}.md`
    : `ℹ️  尚无 docs/release-notes/${curVer}.md 草稿`)
}

function applyVersionBump(targetVersion, dryRun) {
  for (const item of VERSION_TARGETS) {
    const abs = path.join(ROOT, item.rel)
    if (dryRun) {
      console.log(`  [预演] ${item.rel} → ${targetVersion}`)
      continue
    }
    if (item.type === 'json') {
      updateJsonTopVersion(abs, targetVersion)
    } else if (item.type === 'cargo') {
      updateCargoPackageVersion(abs, targetVersion)
    } else if (item.type === 'npm-lock') {
      updateNpmLockVersion(abs, targetVersion)
    }
    console.log(`  ✓ ${item.rel} → ${targetVersion}`)
  }
}

function main() {
  const { positional, flags } = parseCliArgs(process.argv.slice(2))
  if (flags.has('help') || positional[0] === 'help') {
    console.log('用法：node scripts/prepare-release.mjs [目标版本 | next-beta | patch | minor | graduate] [--dry-run] [--skip-checks] [--full-frontend] [--allow-branch] [--force]')
    return
  }

  const spec = flags.get('version') || positional[0]
  if (!spec || flags.has('check')) {
    printStatusReport()
    return
  }

  const rows = readCurrentVersions()
  const currentVersion = rows[0].current
  const targetVersion = resolveTargetVersion(currentVersion, spec)
  const targetSemver = parseSemver(targetVersion)
  const isBeta = targetSemver.prerelease.length > 0
  const branch = execText('git', ['rev-parse', '--abbrev-ref', 'HEAD'], { cwd: ROOT })

  if (!flags.has('allow-branch')) {
    const expectedBranch = isBeta ? 'beta' : 'main'
    if (branch !== expectedBranch) {
      throw new Error(`当前分支是 ${branch}，而目标版本 ${targetVersion} 应在 ${expectedBranch} 分支上发布；确认无误可加 --allow-branch`)
    }
  }

  const { dirty } = listGitStatus()
  const dryRun = Boolean(flags.get('dry-run'))
  if (dirty.length > 0 && !dryRun && !flags.has('force')) {
    throw new Error(`工作区有未提交改动（${dirty.length} 项），请先提交或清理后再准备发版（或加 --force）`)
  }

  console.log(`→ 准备主程序版本：${currentVersion} → ${targetVersion}（分支：${branch}${dryRun ? '，预演模式' : ''}）`)
  applyVersionBump(targetVersion, dryRun)

  if (dryRun) {
    console.log('预演完成：未写入任何文件。')
    return
  }

  if (!flags.has('skip-checks')) {
    console.log('\n→ 刷新 Cargo.lock 并校验主程序与测试代码编译（cargo check --tests）…')
    const cargoRes = runSpawn('cargo', ['check', '--manifest-path', 'desktop-tauri/src-tauri/Cargo.toml', '--tests'], { cwd: ROOT })
    if (cargoRes.status !== 0) throw new Error('cargo check --tests 失败')

    console.log('\n→ 运行全链路启动、存量数据整理与管理面板核心接口冒烟测试…')
    const smokeRes = runSpawn('node', ['scripts/cargo-test.mjs', '--lib', 'server::native_gateway_tests::full_lifecycle_bootstrap_maintenance_and_panel_smoke'], { cwd: ROOT })
    if (smokeRes.status !== 0) throw new Error('全链路启动冒烟测试未通过，已拦截发版准备')

    console.log('\n→ 重建手机端界面产物（同步新版本号与缓存哈希）…')
    const mobileRes = runSpawn('npm', ['run', 'mobile:build'], { cwd: ROOT, useNpmOnWin: true })
    if (mobileRes.status !== 0) throw new Error('npm run mobile:build 失败')

    if (flags.has('full-frontend')) {
      console.log('\n→ 重建桌面端界面与多语言产物…')
      const islandsRes = runSpawn('npm', ['run', 'ui:islands'], { cwd: ROOT, useNpmOnWin: true })
      if (islandsRes.status !== 0) throw new Error('npm run ui:islands 失败')
      const i18nRes = runSpawn('npm', ['--prefix', 'desktop-tauri/i18n', 'run', 'build'], { cwd: ROOT, useNpmOnWin: true })
      if (i18nRes.status !== 0) throw new Error('i18n 构建失败')
    }
  }

  const lastTag = getLatestTagOnHead()
  console.log('\n✅ 主程序发版准备完成！下一步提醒：')
  console.log(`  1. 请编写/确认更新日志草稿：docs/release-notes/${targetVersion}.md（若改动了安装参数/包名，同步检查 docs/install*.md 六份与 README*.md）`)
  if (lastTag) {
    console.log(`  2. 核对自 ${lastTag} 起的改动并由用户逐条确认版本号与更新日志后再提交/打 tag v${targetVersion}。`)
  }
  console.log('  3. 提交时避免 git add . 把 .antigravitycli/ 等本地目录带入，可用：git add -- package.json desktop-tauri')
}

try {
  main()
} catch (error) {
  console.error(`❌ ${error.message}`)
  process.exit(1)
}

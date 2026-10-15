#!/usr/bin/env bash
# 发版收尾脚本：v* tag 的 build 工作流跑完后，一条命令挂上 GitHub Release。
#
#   bash scripts/release.sh vX.Y.Z            # 自动找该 tag 的成功 build run
#   bash scripts/release.sh vX.Y.Z <run-id>   # 或显式指定 run
#
# 做三件事：
#   1. 下载 build 工作流的产物 artifact（windows-nsis / macos-universal，以及
#      新版的 linux-server-amd64 / -arm64 两个容器内更新包）到 dist/；
#   2. 用 tag 所指提交的提交信息（= 更新日志，见 agent.md 第 5 节）创建 / 更新
#      GitHub Release 并挂全部附件；
#   3. 打印验收提示。
#
# linux-server-* 两个 artifact 是后加的（容器内一键更新用）：老 tag 的 run 里
# 没有它们，脚本只提醒、不中断（历史 tag 仍可重跑收尾）。
#
# 幂等可重跑：附件 `--clobber` 覆盖上传。

set -euo pipefail

TAG="${1:?用法: scripts/release.sh vX.Y.Z [build-run-id]}"
RUN_ID="${2:-}"

case "$TAG" in
  v*) ;;
  *) echo "错误: '$TAG' 不是 v* 形式的 tag" >&2; exit 1 ;;
esac

command -v gh   >/dev/null || { echo "错误: 未安装 gh CLI" >&2; exit 1; }
# gh 的 JSON 输出用 node 解析（不引入 jq 依赖，两端都有 node）
command -v node >/dev/null || { echo "错误: 未安装 node" >&2; exit 1; }

# gh 是 Go 程序：既不读 git 的 http.proxy，也不读 Windows 系统代理，只认代理
# 环境变量。本机 git 配了代理而 gh 没配，下面的下载安装包与上传附件都是大文件
# 传输，直连不稳时会中途失败 —— 故把 git 的代理透传给 gh。
# 只设 HTTPS_PROXY：gh 的请求（含附件上传下载）全是 HTTPS，实测不读 HTTP_PROXY。
# 从 git 读取而非写死端口，换代理只改 git 一处；git 未配代理就不设，退回直连
# （外部已显式设过时不覆盖）。
if [ -z "${HTTPS_PROXY:-}" ]; then
  # 按 git/curl 查找代理的顺序依次尝试，兼容这几种配置写法：
  #   http.<url>.proxy（按仓库配） / http.proxy / https.proxy
  # 只取 --get-urlmatch 会漏掉 https.proxy，只取 https.proxy 会漏掉前两种。
  GH_PROXY=$(git config --get-urlmatch http.proxy https://github.com || true)
  [ -n "$GH_PROXY" ] || GH_PROXY=$(git config --get https.proxy || true)
  if [ -n "$GH_PROXY" ]; then
    export HTTPS_PROXY="$GH_PROXY"
    echo "→ 已启用代理: $GH_PROXY"
  fi
fi

# ── 1. 定位 build run 并下载安装包 ────────────────────────────
if [ -z "$RUN_ID" ]; then
  echo "→ 查找 $TAG 触发的 build 工作流…"
  # 不按 run 整体 conclusion 过滤：只验「两个安装包 artifact 是否齐全」，
  # 构建成功但别处失败的 run 里安装包照旧可用
  for id in $(gh run list --workflow=build --limit 30 --json databaseId,headBranch \
      | node -e 'let s="";process.stdin.on("data",d=>s+=d);process.stdin.on("end",()=>{JSON.parse(s).filter(r=>r.headBranch===process.argv[1]).forEach(r=>process.stdout.write(r.databaseId+"\n"))})' "$TAG"); do
    if gh api "repos/$(gh repo view --json nameWithOwner -q .nameWithOwner)/actions/runs/$id/artifacts" \
         --jq 'any(.artifacts[].name; . == "macos-universal") and any(.artifacts[].name; . == "windows-nsis")' >/dev/null 2>&1; then
      RUN_ID=$id
      break
    fi
  done
  [ -n "$RUN_ID" ] || {
    echo "错误: 没找到 $TAG 的可用 build run（需 macos-universal 与 windows-nsis 两个 artifact 齐全）—— 先确认 CI 跑完（gh run list），或手动传 run-id" >&2
    exit 1
  }
fi
echo "→ build run: $RUN_ID"

rm -rf dist
gh run download "$RUN_ID" -D dist
EXE=$(ls dist/windows-nsis/*.exe)
DMG=$(ls dist/macos-universal/*.dmg)
echo "→ 已下载 $(basename "$EXE") / $(basename "$DMG")"

# 容器内更新包：两个架构的 tar.gz + 同名 .sha256（面板按名找校验文件，
# 缺了会拒绝安装）。老 tag 的 run 没有这两个 artifact —— 提醒一句继续走。
ASSETS=("$EXE" "$DMG")
if [ -d dist/linux-server-amd64 ] && [ -d dist/linux-server-arm64 ]; then
  # 两个目录下各是 <包名>.tar.gz 与 <包名>.tar.gz.sha256，原样全带上
  for file in dist/linux-server-amd64/* dist/linux-server-arm64/*; do
    ASSETS+=("$file")
  done
  echo "→ 已下载 $(basename "$(ls dist/linux-server-amd64/*.tar.gz)") / $(basename "$(ls dist/linux-server-arm64/*.tar.gz)")（含 .sha256）"
else
  echo "⚠️  这个 run 里没有 linux-server-* 产物（本功能上线前的 tag？）——"
  echo "    Release 将不含容器内更新包，面板更新入口会退回「手动升级命令」提示"
fi

# ── 2. 更新日志 = tag 所指提交的提交信息 ──────────────────────
NOTES_FILE=$(mktemp)
trap 'rm -f "$NOTES_FILE"' EXIT
git log -1 --format=%B "$TAG" > "$NOTES_FILE"

echo "→ GitHub Release"
if gh release view "$TAG" >/dev/null 2>&1; then
  gh release upload "$TAG" --clobber "${ASSETS[@]}"
  echo "  ↳ 已存在，附件覆盖上传"
else
  gh release create "$TAG" --title "$TAG" --notes-file "$NOTES_FILE" "${ASSETS[@]}"
fi

# ── 3. 验收提示 ───────────────────────────────────────────────
echo
echo "发版收尾完成。核对："
echo "  [1] GitHub Release:  gh release view $TAG"
echo "  [2] Docker Hub:      https://hub.docker.com/r/aimodcc/agent2api/tags （$TAG 版本号 + latest）"
echo "  [3] 安装包「关于」页版本号与 $TAG 一致"
echo "  [4] 容器内更新：面板「设置 → 软件更新」出现「下载并更新」，且 Release 里"
echo "      agent2api-server-linux-{amd64,arm64}.tar.gz 与各自的 .sha256 都在"

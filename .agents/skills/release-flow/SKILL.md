---
name: release-flow
description: Agent2API（workbuddy）仓库的协作与发版规范 —— 分支落脚点、合并 PR 与回复 Issue 的署名、版本号递增、主程序正式版与 beta 预发布流程、独立 WASM 插件发版流程、源码 / 构建 / 官网与 CDN 分发体系分工、构建仓库迁移、发版验收与排错。凡是涉及切分支、给 PR 选 base、合并 PR、回 Issue/PR、改版本号、写更新日志、打 tag、挂 GitHub Release、发 beta 版、发布 WASM 插件、改 CI 工作流、配签名 secret、换构建仓库、排查发版失败时，都先按本 Skill 执行 —— 即使对方没提「发版」「分支」这类字眼。
---

# Agent2API 协作与发版规范

覆盖两类工作：**日常协作**（分支落脚点、PR 署名、Issue 落款，第 1–3 节）与**发版**（版本号、更新日志、主程序与 WASM 插件发版、官网 CDN 分发、验收，第 4–7 节），排错见第 8 节。

## 0. 两条人工红线（必须停下等确认）

下列两件事**必须由人先审过、明确点头**，代理才能继续；可以准备草稿、可以列依据，不能自己拍板往下走：

1. **发版**：进入发版流程（改版本号、建发布提交、推 main / beta、打 tag、挂 Release）之前，先把版本号与「这次要发什么」讲清楚，等用户确认。**未经确认不得推任何分支或 tag** —— tag 一旦推出去就会触发构建、产出公开的安装包与镜像，收不回来。
2. **更新日志**：写好的日志（发布提交的提交信息，同时也是 GitHub Release 正文）必须先给用户逐条过目。用户要求改措辞、增删条目、调整分类前缀时，按反馈改完要**再确认一次**，不能因为「改得不多」就直接提交。

一次发版里两条同时适用：既要有版本号的确认，也要有日志的确认；用户说「继续」之后才动手。**若本次同时涉及主程序与多个独立单包 WASM 插件联动发版**，不必逐个包反复打断询问：先把「主程序目标版本号 + 有改动需要发布的单包插件清单及各自版本号」连同各自的更新日志草稿合并整理成一张总览表交由用户一次性过目；有修改则改后合并再确认，全部拍板后再按仓库依赖顺序推进。

## 0.5 源码 / 构建 / 官网 CDN 分工

构建与公开分发不在源码仓库直接产出，独立索引仓库 `Agent2plugin/Agent2plugin` 也已退役。当前发版涉及三段架构，各管一段，弄混了就会出现「tag 推上去了，但官网 / 客户端收不到更新」这类静默故障：

| 环节 | 对应工程 / 地址 | 职责说明 |
|---|---|---|
| **主程序源码** | `aimod-cc/Agent2API`（当前仓库） | 源码、文档、`.github/workflows/build.yml`（推 `v*` tag 只做派发，不编译） |
| **独立 WASM 插件源码** | `Agent2Providers` / `Agent2WebProviders`（子目录 `plugin/`） | 单提供商独立 WASM 包与 SDK；均支持单包 `wasm-build.yml`（`wasm-<id>-v*`）与批量 `build.yml`（`v*`）派发构建 |
| **主程序构建仓库** | `Agent2Builder/agent2api-releases` | `release.yml` 构建全平台包，`publish-installers.yml` 校验安装包、镜像源码仓库并向官网后台上报发布，`docker.yml` 发布双架构镜像。**主程序签名密钥只在这里** |
| **插件构建仓库** | `Agent2Builder/agent2api-providers-releases` | `wasm-release.yml` / `wasm-batch-release.yml` / `publish-wasm-artifacts.yml` 构建并签名 WASM 插件包、同步发布到官网后台 |
| **官网、后台与 R2 / CDN** | `Agent2API-Website`（`agent2api.dev`）、`Agent2API-Admin`（`oyl.agent2api.dev`）、`cdn.agent2api.dev` | 接收 CI 带签名上报（`/api/ci/releases`、`/api/ci/plugins`），托管安装包与插件包，并生成 CDN feed（`cdn.agent2api.dev/agent2api/feeds/...`）供客户端优先读取，兜底走 `agent2api.dev/api/...` |

### 为什么这么分

七个平台 job + 两架构镜像，跑一次是实打实的重负载；而 tag 是发版动作、频率不可控。把「打一个版本标签」变成「自动兑换七台虚拟机」，最坏会被判定为 Actions 滥用。构建挪到专用账号下，额度与负载都落在那边。

### 一次 tag 的完整链路

```
git push origin vX.Y.Z
  └─ 源码仓库 build.yml（dispatch job，约 10 秒）
       └─ repository_dispatch → Agent2Builder/agent2api-releases
            ├─ release.yml  各平台构建 → publish-installers.yml
            │    ├─ 发布安装包 → 构建仓库 Release
            │    ├─ 签名上报 → 官网后台（oyl.agent2api.dev）刷新 R2 与 CDN feed
            │    └─ 同步主仓库 → 相同附件与日志
            └─ docker.yml   双架构镜像 → Docker Hub 与 GHCR
```

`release.yml` 与 `docker.yml` 都监听 `repository_dispatch` 的 `release` 事件，所以一条派发同时起两边。

### 换构建仓库（构建账号被封 / 要迁移时）

按顺序做，**顺序错了会让发布与官网同步断档或兜底失效**：

1. 新建构建仓库（主程序对应 `agent2api-releases`，独立 WASM 插件对应 `agent2api-providers-releases`），迁入工作流与 scripts，并配齐 secrets（包含与官网后台通信的 `RELEASE_PUBLISH_SECRET` / `PLUGIN_PUBLISH_SECRET`，见第 8 节）；
2. 让官网后台 `Agent2API-Admin` 识别并接收新仓库的发布上报，通过 `/api/ci/releases` 与 `/api/ci/plugins` 刷新 `cdn.agent2api.dev/agent2api/feeds/` 下的 CDN 清单（`updates/app.json`、`updates/releases.json`、`plugins/catalog.json`）以及 `agent2api.dev/api/...`；
3. 更新源码仓库的派发目标：
   - 主程序：`.github/workflows/build.yml` 里的 `repository:`
   - 插件：`plugin/Agent2Providers/.github/workflows/{build,wasm-build}.yml` 与 `plugin/Agent2WebProviders/.github/workflows/{build,wasm-build}.yml` 里的 `repository:`
4. 改源码仓库里的兜底常量与发版脚本（当官网 / CDN 不可用或本地脚本查包时使用）：
   - `desktop-tauri/src-tauri/server/src/server/core/update/version.rs` 的 `DEFAULT_REPO`
   - `desktop-tauri/src-tauri/server/src/server/core/plugin_host/update.rs` 的 `FALLBACK_REPO`
   - `scripts/release.sh` 的 `BUILD_REPO` 默认值

**说明**：客户端桌面端与插件更新优先读取 `cdn.agent2api.dev/agent2api/feeds/...`（并兜底 `agent2api.dev/api/...`），安装包与插件包下载仅放行 `agent2api.dev` 与 `cdn.agent2api.dev` 域名；只要构建仓库把签名后的元数据推上官网后台并刷出 CDN feed，已发布的旧版本无需重新发版即可拉到新产物（`Agent2plugin/Agent2plugin` 的旧格式 `app.json` / `index.json` 已停止使用）。

## 1. 分支落脚点：日常只碰 beta，发版才动 main

| 分支 | 角色 | 什么时候动它 |
|---|---|---|
| `beta` | 集成分支（默认落脚点） | 日常修改、合并 PR、解冲突 —— 一律先到这里 |
| `main` | 发版分支 | 只在构建发版时：切 main、把 beta 合进来、打 tag |

- **合并 PR 时 base 选 `beta`**，不要选 `main`。合进 `main` 等于把还没发布的改动提前塞进发版分支；之后还得反过来把 main 合回 beta，平白多一次合并提交、多一次冲突机会。已经把 PR 开成 base=main 的，先在 PR 页面把 base 改成 `beta` 再合。
- **本地改动也提交到 `beta`**（写代码、改文档、解冲突都算），推 `origin/beta`；合并结果的核对、构建自检都在 beta 上做完。
- **只有正式发版才切 `main`**：`git switch main` → 把 beta 合进来 → 写发布提交、推 main、打 tag；tag 必须落在 `main` 上。发完版 beta 再把 main 合回来，两边保持一致。
- **不要自作主张选落脚分支**：用户没说要发版，就不要往 `main` 提交 / 推送，也不要把 PR 的 base 改成 `main`。
- beta 版（预发布）是唯一例外：不切 `main`，tag 直接落在 `beta` 分支自己身上（见第 6 节）。

## 2. 合并 PR：保留贡献者署名

合并贡献者的 PR 时，提交作者（author）必须还是贡献者本人 —— 这直接决定对方能不能进仓库首页 Contributors 名单、贡献图里有没有这一笔。署名是贡献者应得的东西，不该因为合并方式丢掉。

GitHub 的归属规则只有一条：**提交头里的 author 邮箱出现在某个账号已验证的邮箱列表里，这次提交就算那个账号的**；否则记成匿名条目，不进 Contributors。另有两条口径：统计只认默认分支（本仓库 `main`），合并提交与空提交不计入。

- **默认用 GitHub 网页上的合并按钮**（`Merge pull request` / `Squash and merge` / `Rebase and merge`）：三种都会保留原作者为 author；squash 时 PR 里的多位作者会一并进 co-author 名单。
- **禁止「本地摘取重提」**：把对方的改动拷过来自己提交（本地 `git merge --squash` 后自己 commit、重抄一遍改动、或改了 author 再提交）都不行 —— 这样 author 是合并者，贡献者的署名在 GitHub 侧完全不可见。本仓库历史上出现过这种情况，别再来一次。
- **确实要在本地处理时**（有冲突要解、要顺带调整），必须把署名带上：单个原作者用 `git commit --author="原作者 <原作者邮箱>"`；一个提交里有多个人的工作，在提交信息末尾加 trailer `Co-authored-by: 名字 <邮箱>`。
- **摘除 AI 的 co-author trailer**：贡献者提交信息末尾的 `Co-Authored-By: Claude Code <…>` 这类 AI trailer，合并时一律删除 —— GitHub 会把它渲染成共同作者，进了 `main` 就出现在 Contributors 名单里，而 AI 不是贡献者。只删 trailer 行本身，author 与其余提交信息逐字保留；正文里对 AI 产品的**技术性提及**（「支持 Claude Code 客户端」「Claude 兼容端点」之类）与此无关，不动。摘除动的是已合入分支的历史，趁合并后、发版前尽早做（`git filter-branch --msg-filter` 摘行后 force-push），拖到发版后就得重写 `main`。
- **邮箱必须能对上账号**：用对方 GitHub 账号里**已验证**的地址；对方想保密就用 GitHub 给的 `ID+用户名@users.noreply.github.com`（在对方账号的 Emails 设置里查）。内网地址、`user@机器名` 这类邮箱永远关联不上账号，提交会被记成匿名。
- 合并后顺手核对一次：`git log --format='%an <%ae>' -1 <合并提交>`，或在 PR 页面看 commit 旁的头像是否指向原作者。

## 3. 回复 Issue / PR：标明 AI 来源

**由 AI 代理发出（含代笔）的每一条 Issue / PR 回复，末尾都要单独一行写明来源。** 收件人有权知道自己在跟谁说话 —— 与第 2 节的署名是同一条道理：**谁说的写谁**。

固定落款，逐字照用（别改写成「机器人」「助手」这类含糊说法）：

> 🤖 本条回复由 AI Agent 生成

- **位置**：整条回复的最后一行，单独成段（前后各留一个空行）；不要塞进正文段落、列表项、引用块或代码块里 —— 引用别人的话时，落款落在自己的话之后。
- **英文场合**（外部贡献者的 PR / Issue）在中文那行下面再补一行 `Generated by an AI agent.`，说的是同一件事，两边都不省。
- **代理自己新建的 Issue / PR**，正文末尾同样加。
- **代笔也算**：人在对话里让代理转达（哪怕逐字）一段话，落款照加 —— 它描述的是「这条回复是谁发出去的」，不是「谁构思的」。
- 只约束 Issue / PR **回复**（评论、review 意见、关闭 / 合并说明）。提交信息不追加这一行，署名口径见第 2 节。

## 4. 版本号

- 版本号形如 `X.Y.Z`（如 `2.7.2`），日常小版本「递增 0.01」= 末位 +1（如 `2.7.8` → `2.7.9`）。
- **递增幅度以用户要求为准**：默认用上面的「递增 0.01」；用户另有要求时（如「这次递增 0.1」）一律听用户的，按用户指定的幅度算出新版本号，不要自作主张套用默认幅度。
- **版本号要同步改 6 处**（缺一处会导致安装包与显示版本对不上）：
  1. `package.json`（根）
  2. `desktop-tauri/package.json`
  3. `desktop-tauri/src-tauri/Cargo.toml`
  4. `desktop-tauri/src-tauri/tauri.conf.json`
  5. `desktop-tauri/src-tauri/server/Cargo.toml`
  6. `desktop-tauri/mobile/package.json` —— 手机端安装包与手机网页端 `APP_VERSION` 都从这里读：Android 的 `versionName` / `versionCode` 由 `android/variables.gradle` 读它（`versionCode` 由 semver 折算，只增不减，见该文件注释）；iOS 的 `MARKETING_VERSION` / `CURRENT_PROJECT_VERSION` 由 `build.yml` 的 ios job 读它并传给 xcodebuild；构建手机前端时 `desktop-tauri/mobile/vite.config.ts` 还会把 `__APP_VERSION__` 编进 `desktop-tauri/ui/m/mobile.js` 并更新 `desktop-tauri/ui/sw.js` 的缓存哈希。不改它两端的包与手机网页都还是旧版本号，产物名 `Agent2API-<版本>.apk` / `.ipa` 也取自这里
- **改完第 6 处，必须同时做两件事**：
  1. **同步更新 `desktop-tauri/mobile/package-lock.json` 两处 `version`**（文件顶层的 `"version"` 与 `packages[""].version`）。`npm ci` 对这两处不一致是**静默容忍**的（实测不会报错，也不会自动改），漏改 CI 照绿，但仓库里会留下版本号不一致的 lock 文件，下次谁跑 `npm install` 又会把它改回来，平白多出一个无关 diff（桌面端 `desktop-tauri/package-lock.json` 本来落后，不随发版改）。
  2. **重跑一次 `npm run mobile:build`**，让 `desktop-tauri/ui/m/` 与 `desktop-tauri/ui/sw.js` 跟着重编；CI 的 `.github/workflows/frontend-artifacts.yml` 会逐字节核对构建产物，漏编这一步会导致该检查直接失败。
- 服务端 / 桌面端应用内「关于」与更新检查读的是 `env!("CARGO_PKG_VERSION")`（第 5 处），改完跑一次 `cargo check` 让 `Cargo.lock` 跟着刷新；手机端网页「关于 / 更新检查」则读取 `mobile/package.json` 注入的 `APP_VERSION`（系统「应用信息」与发布包名亦取自该处）。
- **推荐直接使用主程序准备脚本**：先运行 `node scripts/prepare-release.mjs`（或 `npm run release:prepare`）查看 6+2 处版本一致性、工作区状态及上一个 tag 以来的提交摘要；确认版本号后运行 `node scripts/prepare-release.mjs <目标版本 | next-beta | patch | graduate>` 一次性同步全部 6+2 处版本号并自动执行 `cargo check` 与 `npm run mobile:build`（若同时改了桌面 UI 或多语言，可加 `--full-frontend`）。该脚本只改本地文件与构建产物，不会自动提交或推送。
- **顺手过一遍安装文档**：安装说明已经从 README 搬进 `docs/install.md`（README 只留速查表 + 一个链接），另有 5 份译文 `docs/install.{en,zh-Hant,ja,ko,pt-BR}.md`，六份内容一一对应。凡是这次改动碰到了**安装 / 升级 / 卸载步骤、产物体名、默认端口、环境变量、安装脚本参数**，六份都要同步改；README 速查表受影响时也一起改。
- **README 语言落点**：根 `README.md` 是**英文**（GitHub 默认展示），简体中文在 `docs/README.zh-CN.md`，其余为 `docs/README.{zh-Hant,ja,ko,pt-BR}.md`。中文仍是文案源语言，支持矩阵等六份要同步的内容改完记得回填英文根 README。

## 5. 正式版发版流程

按顺序执行；第 0 节的两条红线贯穿全程。

```bash
# 1. 收干净工作区（tag 里的代码就是发出去的代码；若存在 .antigravitycli/ 等工具目录注意排除）
git status --short
node scripts/prepare-release.mjs

# 2. 切到 main 并把 beta 合进来
git switch main && git pull
git merge beta

# 3. 同步改完 6+2 处版本号、刷新 Cargo.lock 并重编手机端前端产物（写入 __APP_VERSION__ 与 sw.js 缓存哈希）
#    手动改也可，推荐一步完成：
node scripts/prepare-release.mjs <X.Y.Z>

# 4. 写发布提交：提交信息 = 更新日志（提交前建议显式指定路径，避免 git add . 带入无关工具目录）
git add -- package.json desktop-tauri docs
git commit -F 更新日志.txt   # 或直接 -m "..."

# 5. 推 main、打 tag、推 tag
#    推 tag 只会在**源码仓库**跑一个十几秒的派发 job，真正的构建在
#    Agent2Builder/agent2api-releases 里跑（见 0.5 节）
git push origin main
git tag vX.Y.Z
git push origin vX.Y.Z

# 6. 到**构建仓库**看进度（不是本仓库）
gh run list --repo Agent2Builder/agent2api-releases --limit 4

# 7. 构建完成后由工作流发布安装包并同步源码仓库，检查两个收尾任务
#    本地 scripts/release.sh 只在自动流程不可用或旧版本补发时使用
```

**发布提交的提交信息 = 更新日志**。自动发布任务读取固定源码提交的日志；应急脚本仍读取 `git log -1 --format=%B "$TAG"`。不得从其它分支生成替代日志。

- **发布日志统一分为「核心更新」和「其他更新」两部分**，正式版、beta 版及独立插件的发布日志均适用。两部分分别用标题展示，标题下都必须使用有序列表，并各自从 `1.` 开始编号；不再把所有内容混成一份连续编号清单，也不用无序列表代替。
- **核心更新**放本次主要新功能、重要体验变化和关键修复；**其他更新**放其余优化、一般修复、多语言、文档及构建流程调整。同一项只归入一部分，不重复列出。
- 每行一条，继续使用分类前缀：`新增：` / `优化：` / `修复：` / `更新：` / `移除：`。版本变化放在「其他更新」有序列表的最后一条，写成 `版本：旧版本 → 新版本`；beta 后缀必须完整保留，首次发布写 `版本：首次预发布 <版本号>` 或 `版本：首次发布 <版本号>`。
- 固定排版示例（发布提交信息与 Release 正文使用同一份内容）：

  ```markdown
  ## 核心更新

  1. 新增：本次主要功能。
  2. 修复：影响核心使用的关键问题。

  ## 其他更新

  1. 优化：其他体验细节。
  2. 更新：多语言文案与构建流程。
  3. 版本：3.4.2 → 3.5.0-beta.1
  ```

- 内容取「上个 tag 以来的全部提交」综合整理（`git log vX.Y.Z..HEAD --oneline`），纯 CI / 临时的调试提交归并到「其他更新」的一条流程性描述即可；
- 本次没有代码改动时用空提交承载：`git commit --allow-empty -F 更新日志.txt`；
- 更新日志草稿与历史发布说明统一写在 `docs/release-notes/<版本号>.md`（如 `docs/release-notes/2.9.2.md`），不要在项目根目录新建 `.release-notes-*.md` —— `docs/release-notes/` 已在忽略列表里（只忽略这个目录，`docs/` 下其它文档正常入库），新增文件不必再加忽略规则。

推 `v*` tag 后，**源码仓库**只跑 `build.yml` 的 `dispatch` job（约 10 秒）；它向构建仓库派发，**构建仓库**里的两个工作流同时开跑：

| 工作流（跑在构建仓库） | 产出 | 说明 |
|---|---|---|
| `release.yml` → `publish-installers.yml` | 桌面端、服务端与手机端发布文件 | 构建任务为 `macos`、`windows`、`linux-desktop`、`linux-server`（amd64 / arm64）、`windows-server`、`android`、`ios`。七个任务全部成功后，`publish-installers.yml` 由**完成事件自动触发**（`workflow_run`）：校验桌面 / 服务端 / 手机端全部附件（`3.5.0-beta.8` 起含 APK 与 IPA）并发布到构建仓库、用签名密钥同步上报官网后台（`Agent2API-Admin`）刷新 R2 与 CDN feed，再镜像一份到源码仓库。WASM 插件由独立插件构建仓库 `agent2api-providers-releases` 发布 |
| `docker.yml` | Docker Hub `aimodcc/agent2api:<版本>` 与 GHCR `ghcr.io/aimod-cc/agent2api:<版本>`，amd64 / arm64 双架构 | 正式版发布 `<版本>` + `:latest`；beta 预发布发布 `<版本>` + 滚动 `:beta`，**不移动 `:latest`**；手动 `workflow_dispatch` 且不填 ref 时只出 `:dev`，不移动 `:beta` 或 `:latest`。两个仓库推的是同一份清单 |

**源码仓库的 `build.yml` 只做派发，`docker.yml` 已整体删除** —— 镜像构建现在只在构建仓库里跑（源码仓库保留那份只会让人误以为它还在工作）。

跟踪进度：

```bash
gh run list --repo Agent2Builder/agent2api-releases --limit 4
gh run watch <run-id> --repo Agent2Builder/agent2api-releases --exit-status
```

**自动收尾**由构建仓库的 `publish-installers.yml` 完成，它监听 `release.yml` 的完成事件（`workflow_run`），只有一个 job、顺序做两步：

- **发布到构建仓库并同步官网 / CDN**：按那次构建 run 取产物，用 `.github/scripts/collect-assets.sh` 清点（自 `3.5.0-beta.8` 起支持 **14 份**〔含 `_universal.dmg`〕或 **15 份**〔macOS 拆为 `_aarch64.dmg` 与 `_x64.dmg`〕：桌面端 4/5 份 + 服务端 `agent2api-server-*` 6 份〔包 + `.sha256`〕+ 手机端 `Agent2API-<版本>.apk` / `Agent2API-<版本>.ipa` 及各自 `.sha256` 共 4 份；`3.5.0-beta.8` 之前旧版继续兼容 10 / 11 份），先建草稿 Release、**逐个**上传、全齐才公开；beta 自动标预发布。随后使用 `RELEASE_PUBLISH_SECRET` 向官网后台（`oyl.agent2api.dev/api/ci/releases`）签名上报，生成 R2 与 CDN 的 `updates/app.json`、`updates/releases.json`。**客户端优先读 CDN feed，兜底读官网 API 与构建仓库 Release。**
- **镜像到源码仓库**：同一份附件与逐字相同的正文，用 `SOURCE_RELEASE_TOKEN` 建 / 改 `aimod-cc/Agent2API` 的 Release。缺这个 token 或网络失败时这一步明确失败，本仓库已发布的 Release 保留、用户更新不受影响。
- **为什么发布与构建分成两个工作流**：发布出错只需要重跑 `publish-installers.yml`（几十秒、不重新编译），不会被迫把七个平台再建一遍 —— tag 是发版动作、频率不可控，重跑构建最坏会被判成 Actions 滥用。重跑用 `only` 挑步骤，按 run 号补发就传 `run_id`：

  ```bash
  # 补发 / 重跑（不填 run_id 时用触发本工作流的那次构建）
  gh workflow run publish-installers.yml --repo Agent2Builder/agent2api-releases -f run_id=<run-id>
  # 只重做镜像那一步
  gh workflow run publish-installers.yml --repo Agent2Builder/agent2api-releases -f run_id=<run-id> -f only=mirror
  ```

- **自动发布只认 `repository_dispatch` 触发的构建**（也就是 tag 发版）。手动触发的构建不会自动发布，免得把分支快照发出去；要发就手动传 `run_id`。
- 上传是**同名覆盖**（逐个 `--clobber`）：gh 批量上传遇到「附件已存在」会报 HTTP 422 并中断整批，症状是 Release 建好了附件少一半（2026-10-17 实测）。所以重跑天然幂等，只补缺的那几个。

本地 `scripts/release.sh` 保留为**应急入口**，只用于旧版本或自动流程不可用时：

```bash
bash scripts/release.sh vX.Y.Z <run-id>
```

脚本会清空当前目录的 `dist/`；使用前必须检查并保护已有产物，建议在独立工作区运行。

- 目标仓库由脚本里的 `BUILD_REPO` 决定，默认 `Agent2Builder/agent2api-releases`；临时换目标用环境变量：`BUILD_REPO=Other/repo bash scripts/release.sh vX.Y.Z`。
- 脚本找的是构建仓库里名为 **`release`** 的工作流（不是 `build`）。源码仓库那个 `build.yml` 只做派发，没有产物可下。
- **更新日志仍从源码仓库取**（`git log -1 --format=%B "$TAG"`），所以脚本要在源码仓库目录里跑。

### 工作流的跨仓库凭据

构建仓库的 `publish` 使用该任务的 `GITHUB_TOKEN` 和 `contents: write`。`mirror` 必须单独配置 `SOURCE_RELEASE_TOKEN`：仅授权 `aimod-cc/Agent2API` 的细粒度令牌，仓库权限为 **Contents: Read and write**；不需要工作流写权限，因为源码版本 tag 必须已经存在。

该 Secret 只传给 `mirror`，不传给平台构建或 `publish`。缺少时同步任务明确失败，构建仓库已完成的发布保留。不要把本机登录令牌直接复制为长期 Secret。

### 应急脚本的两把凭据

构建仓库属于专用构建账号，源码仓库属于主账号 —— 各自的 Release 只有**有权限的那一方**能创建 / 修改。应急脚本保留两个 token 变量：

```bash
BUILD_TOKEN=...   # 对构建仓库有 contents:write（默认取当前 gh 活动账号）
SOURCE_TOKEN=...  # 对源码仓库有 contents:write（同上）
```

本机两个账号都在 gh 里时，按账号各给一把：

```bash
BUILD_TOKEN=$(gh auth token --user Agent2Builder) \
SOURCE_TOKEN=$(gh auth token --user aimod-cc) \
bash scripts/release.sh vX.Y.Z
```

不传这两个变量时，脚本假定当前 `gh auth` 的账号对两边都有权限（单一账号管两个仓库的部署就是这样）。

⚠️ **症状对照**：用错 token 时 GitHub 回的是 `HTTP 404: Not Found`，不是 403 —— 看着像「Release 不存在」，实际是「这个 token 看不见它」。遇到 404 先查 token 属于哪个账号。

### 镜像到源码仓库

自动流程总是同步主仓库；应急脚本默认开镜像（`MIRROR_TO_SOURCE=0` 可关）。产物在构建仓库，但用户与贡献者习惯看源码仓库的 Releases 页，两个位置都应有发布。

镜像的正文与构建仓库那份**逐字相同**（同一个更新日志），不追加任何说明行 —— 两个 Release 的内容应当一致。**不要**在镜像正文里加「同一份发布，也是应用更新检查读取的那一份：…」这类指向构建仓库的句子，用户明确不要这一段；更新检查优先读取官网 / CDN 分发清单（`cdn.agent2api.dev` / `agent2api.dev`），兜底仓库由 `DEFAULT_REPO` 决定，不需要在正文里解释。

⚠️ **Release 必须出现在构建仓库并同步到官网 / CDN**。桌面端更新检查优先读取 `cdn.agent2api.dev/agent2api/feeds/updates/app.json` 与 `releases.json`（兜底读 `agent2api.dev/api/...` 与 `core::update::version` 的 `DEFAULT_REPO`）；只发在源码仓库或未上报官网后台的症状是「Release 建出来了，但所有人都显示已是最新」——不报错，最难查。

## 6. beta 版（预发布）

应用里有「设置 → 软件更新 → 更新设置 → **加入 beta 版**」开关：**打开的人才收到 beta 更新，没打开的人只看正式发布。**

beta 版不是另一条流水线，而是第 5 节流程的一个变体 —— 唯一结构差别是「**从 `beta` 分支打带预发布后缀的 tag**」，提交、tag、Release、验收逐字照旧。第 0 节的两条红线同样适用。

```bash
# 1. 在 beta 分支上动手（beta 版不碰 main）
git switch beta && git pull
git status --short          # 必须为空

# 2. 六处版本号与 desktop-tauri/mobile/package-lock.json 两处一起改成「与 tag 逐字一致」的形态：
#    tag 打 v3.1.0-beta.1 → 全部写 3.1.0-beta.1（清单见第 4 节）
cargo check                 # 顺带刷新 Cargo.lock
npm run mobile:build        # 同步 ui/m/mobile.js (__APP_VERSION__) 与 ui/sw.js 哈希

# 3. 提交并推 beta 分支（提交信息 = 这次的 beta 更新日志，格式同第 5 节）
git commit -am "…beta 版日志…"
git push origin beta

# 4. 打 tag 并推：tag 落在 beta 分支的提交上
git tag v3.1.0-beta.1
git push origin v3.1.0-beta.1

# 5. 到**构建仓库**看三个工作流（Release → publish-installers + docker）
gh run list --repo Agent2Builder/agent2api-releases --limit 4

# 6. 构建成功后 publish-installers 自动发布，并同步相同附件和正文到源码仓库
#    核对发布 / 镜像那两步与 docker 成功，无需本地下载和上传
```

验收重点四条：Release 显示 **Prerelease**、构建仓库桌面端 / 服务端 / 手机端附件齐全（并同步到源码仓库与官网 / CDN `updates/releases.json` 预发布通道）、Docker Hub 上 `:latest` 的 Pushed 时间**没有**跟着变、已发布的 APK / IPA 与手机网页 `APP_VERSION`（`desktop-tauri/ui/m/mobile.js` 与 `ui/sw.js`）一致且能装能连、官网下载页与手机端更新检查可用（完整清单见第 7 节）。

**三处不能错**，每处错了都有明确症状：

1. **六处版本号漏了后缀**（写成 `3.1.0`）：比较是按 semver 做的，运行版本 `3.1.0` 对线上 tag `3.1.0-beta.1` 会判成「当前比线上新」，**beta 用户永远收不到更新**。六个文件都接受带后缀的版本号（Cargo / npm / tauri.conf.json 都认），照 tag 抄即可。APK 的 `versionCode` 由 semver 前三段折算、不带后缀（见 `android/variables.gradle`），beta 与同号正式版取同一个值，不构成冲突。
2. **Release 漏标 pre-release**：标记就是开关的分界线，正式通道排除预发布。自动发布根据 semver 预发布后缀判定；应急脚本也会设置。若已有公开版本的标记不一致，自动流程明确失败，不擅自改写。
3. **tag 打在 `main` 上**：beta 是集成分支，beta 版要的是它的快照。打在 `main` 上要么不含 beta 分支的新改动（白测一场），要么等于把还没发布的改动提前推进 `main`。

构建仓库 `.github/workflows/docker.yml` 发布 beta 时同时产出完整版本标签（如 `3.3.0-beta.3`）与滚动 `:beta`，**不移动 `:latest`**。Docker 用户用 `aimodcc/agent2api:beta` 跟随 beta，或用完整版本号固定版本；手动 dev 构建不会移动 `:beta`。

后续常见三件事：

- **再发一个 beta**：核心号不变、递增序号（`3.1.0-beta.2`，六处一起改，上面那套命令照走）。预发布序号能分出先后（`beta.2` > `beta.1`），已经在 beta 上的人照样收到更新 —— 所以 beta 之间也要保持「tag 与版本号逐字一致」。
- **转正**（发 `3.1.0` 正式版）：按第 5 节在 `main` 上走，六处版本号去掉后缀。**beta 用户不会自动接回正式版**：开关开着的人只看预发布版（`pick_listed_release` 按「当前跑的是不是 beta 版」筛），转正的 `3.1.0` 对他们不算更新 —— 要留在 beta 频道的人等下一个 beta（`3.2.0-beta.1` 一出现就收得到）；想回正式版由用户自己关开关，关掉后只在**版本号高于他手上那个 beta** 的正式版发布时才收到更新（`3.1.0` > `3.1.0-beta.1`，但更早的 `3.0.9` 不会把他降级）。
- **「打开了却没有反应」**：仓库里一个预发布 Release 都没有时，打开这个开关也只会看到正式版 —— 第一次 beta 发出去之后才有效果。

## 6.5 独立 WASM 插件发版流程（Agent2Providers / Agent2WebProviders）

独立 WASM 插件不再打进主程序安装包，也不再发布五平台原生压缩包；单包与批量构建统一在 `Agent2Builder/agent2api-providers-releases` 完成（改版本、写日志与推 tag 同样适用「先问人、后推送」两条红线）：

0. **先扫描实际有改动的包并核对跨仓库依赖**：
   - 运行 `node scripts/release-plugins.mjs`（或 `npm run release:plugins`）一次巡检 `plugin/Agent2Providers` 与 `plugin/Agent2WebProviders`：对比每个 `wasm/<id>` 自上次 `wasm-<id>-v*` tag 以来的源码改动、共享模块依赖（`wasm/sdk`、`*-common`、`#[path = "..."]` 跨目录引用）以及是否声明了最新主程序才支持的新宿主能力。
   - **只发有改动的包**：不要对未改动插件顺手打新包；多插件同轮发版时，每个单包提交与 `git tag` 各自写**只属于本插件**的「核心更新 / 其他更新」日志，不把无关插件内容混入。
   - **先后依赖顺序（联动必看）**：
     - 若本次网页插件用到 `Agent2Providers/wasm/sdk` 的新改动，**必须先提交并推送 `Agent2Providers` 的 `beta` 分支**，再推 `Agent2WebProviders` 的单包 tag（网页单包工作流默认按 `sdk_ref=beta` 检出上游 SDK）。
     - 若某插件 `wasm.json` 依赖本次主程序新增宿主能力（如宿主机计算或新登录能力），需确认配套主程序版本同步发布，并在更新日志及用户提示中注明所需主程序版本下限。

1. **单提供商 WASM 发版（`Agent2Providers` 与 `Agent2WebProviders` 均适用）**
   - 用 `node scripts/release-plugins.mjs bump <id>=<目标版本|next-beta|patch> ...` 同步递增目标插件 `wasm/<id>/pack.json` 与 `wasm/<id>/Cargo.toml` 版本号（beta 测试包保留 `-beta.<n>` 后缀）；脚本会自动先跑不带 `--locked` 的 `cargo check --workspace --target wasm32-unknown-unknown` 刷新 `wasm/Cargo.lock`，再跑带 `--locked` 的检查对齐 CI。
   - 逐个插件提交发布日志（`## 核心更新` / `## 其他更新`）并打轻量/附注标签 `wasm-<id>-v<version>`：触发各自 `.github/workflows/wasm-build.yml` 派发到构建仓库 `wasm-release.yml`；构建仓库核对 `provider_category`、编译 `wasm32-unknown-unknown`、打包并附加 `ed25519` 官方签名，建 Release `wasm-<id>-v<version>`，再通过 `PLUGIN_PUBLISH_SECRET` 上报 `oyl.agent2api.dev/api/ci/plugins` 刷新 `plugins/catalog.json` / `community/*.json`。
   - **⚠️ 推送 tag 每批不得超过 3 个**：GitHub 同一次 `git push` 超过 3 个 tag 时会跳过 `on: push: tags:` 自动触发。多包发版请拆批推送，或用 `node scripts/release-plugins.mjs push-tags --yes` 自动按「先推分支、再每批最多 3 个 tag」顺序推送；若仍需手动补派发，可在对应源仓库运行 `gh workflow run wasm-build.yml -f provider=<id> -f version=<ver>`（注：手动触发要求 `wasm-build.yml` 已存在于该仓库的默认分支上 —— 目前 `Agent2Providers` 默认分支为 `main`、`Agent2WebProviders` 默认分支为 `beta`）。

2. **全量 / 批量 WASM 发版（`Agent2Providers` / `Agent2WebProviders`）**
   - 更新根目录 `pack.json`、`wasm/Cargo.toml`（及各自待发布子包）版本号并刷新 `wasm/Cargo.lock`；
   - **先后顺序**：若要同时发 `Agent2WebProviders`，**必须先在 `Agent2Providers` 推好同版本 `v<version>` tag**，`Agent2WebProviders` 的 `.github/workflows/build.yml` 会校验上游 SDK `v<version>` tag 已存在；
   - 推送 `v<version>` tag：派发至 `wasm-batch-release.yml`，产出批量汇总 Release（`v<version>` 或 `web-v<version>`）与逐包 Release（`wasm-<id>-v<version>`）并上报官网后台；若某阶段失败需要补推，可在插件构建仓库手动触发 `publish-wasm-artifacts.yml`（支持参数 `-f only=all|publish|website|mirror`）。

## 7. 验收清单

以下 `gh` 命令都带 `--repo Agent2Builder/agent2api-releases` —— Release 与构建 run 都在构建仓库，不带这个参数会去查源码仓库然后什么都找不到（手动跑 gh 前先设代理，见第 8 节）。

- [ ] **构建仓库的 run 全绿**：`gh run list --repo Agent2Builder/agent2api-releases --limit 4` —— 应看到 `Release` 与 `docker` 两个 run，前者七个 job 全绿（含 `publish`）；
- [ ] GitHub Release（在构建仓库）：`gh release view vX.Y.Z --repo Agent2Builder/agent2api-releases` —— 正文日志齐全，全部附件都在：Windows `Agent2API_<版本>_x64-setup.exe`、macOS（`Agent2API_<版本>_universal.dmg` 或按芯片拆分的 `Agent2API_<版本>_aarch64.dmg` + `_x64.dmg`）、Linux `Agent2API_<版本>_amd64.deb` 与 `Agent2API_<版本>_amd64.AppImage`、三端服务端更新包及 `.sha256` 共 6 份，以及手机端 `Agent2API-<版本>.apk`、`Agent2API-<版本>.ipa` 与各自 `.sha256` 共 4 份（总计 14 或 15 份；`3.5.0-beta.8` 之前旧版为 10 或 11 份，见第 5 节）；
- [ ] **镜像到源码仓库**：`gh release view vX.Y.Z --repo aimod-cc/Agent2API` —— 附件数与构建仓库一致（14 或 15 份：桌面端 + 3 个服务端包×2 + APK/IPA×2）；正文与构建仓库那份逐字相同，**不带**指向构建仓库的附加说明；
- [ ] **手机端包同步分发到官网与 R2**：`Agent2API-<版本>.apk(.sha256)` 与 `Agent2API-<版本>.ipa(.sha256)` 与桌面 / 服务端同轮上传 R2 并出现在官网 `agent2api.dev/downloads` 及 `cdn.agent2api.dev/agent2api/feeds/updates/releases.json`；
- [ ] **WASM 插件包**：可运行 `node scripts/release-plugins.mjs verify` 核对 `Agent2Builder/agent2api-providers-releases` 构建 run 与对应 Release（`wasm-<provider>-v<版本>` 或批量 `v<版本>` / `web-v<版本>`），并在官网 `cdn.agent2api.dev/agent2api/feeds/plugins/catalog.json` 中核对单提供商跨平台 WASM 包 `agent2api-wasm-<id>-<版本>-wasm32-unknown-unknown.zip`、`.sha256` 及 `ed25519` 官方签名（`keyId: official-2026`），不计入主程序附件。刚发布完 1~2 分钟内 CDN 边缘缓存可能仍返回旧版本，可带时间戳查询参数核对最新 `revision` 或稍等一两分钟复查直连链接与包体下载返回 200。
- [ ] Linux 桌面端：装 deb（或 `chmod +x` 后跑 AppImage）能启动，且面板「设置 → 软件更新」能检查到版本、给出对应包型的安装入口（deb → 系统安装器，AppImage → 换文件）；
- [ ] 手机端 Android：从 Release / 官网可下载 `Agent2API-<版本>.apk`（含 `.sha256`），装上后能连接网关，且设置页「检查更新」支持从官网 R2 下载校验并安装；
- [ ] 手机端 iOS：从 Release / 官网可下载 `Agent2API-<版本>.ipa`（含 `.sha256`；未签名包需自行重签），设置页「检查更新」支持直接下载 `.ipa` 或跳转官网下载页；
- [ ] Docker Hub：`aimodcc/agent2api` 的 Tags 页出现 `<版本>` 与 `latest`，Pushed 时间一致；GHCR：`ghcr.io/aimod-cc/agent2api:<版本>` 可拉（**新包默认私有**，首次要去包设置改 Public）；
- [ ] **更新检查与官网分发真的能查到**（这条最容易漏）：`gh release view vX.Y.Z --repo Agent2Builder/agent2api-releases` 有内容，且 `cdn.agent2api.dev/agent2api/feeds/updates/app.json`（及 `releases.json`）已刷出对应版本、下载链接落在 `agent2api.dev` 或 `cdn.agent2api.dev` 下。官网未同步时，面板会出现「Release 已建好但客户端显示已是最新」；
- [ ] 安装包「关于」页版本号与 tag 一致；
- [ ] 安装文档对得上实际产物：`docs/install.md` 与 5 份译文的产物名、端口、命令、环境变量跟这次 Release 一致（README 里那几张速查表同理）；
- [ ] beta 版多一条：Release 显示 **Prerelease**；Docker Hub 上完整版本标签与 `beta` 的镜像 digest 一致，包含 amd64 / arm64，且 `latest` 没有跟着动。

## 8. 已知坑与排查

### 8.0 多账号权限对照（本机推送 / 查进度前先核对）

本机 `gh` 同时登录多个账号时，**日常活动账号保持主账号 `aimod-cc` 即可**；操作其他组织仓库时临时取用对应账号令牌，不要来回改全局默认登录态（遇到仓库 404 先查是否用错了账号）：

| 仓库 | 归属账号 / `gh auth token --user` | 常见动作 |
|---|---|---|
| `aimod-cc/Agent2API` | `aimod-cc` | 推主程序分支 / tag、查看源码仓库 CI、查看镜像 Release |
| `ProvidersPlugin/Agent2Providers`、`ProvidersPlugin/Agent2WebProviders` | `aimodcc` | 推插件 `beta` / `main` / tag、手动触发 `wasm-build.yml` |
| `Agent2Builder/agent2api-releases`、`Agent2Builder/agent2api-providers-releases` | `Agent2Builder` | 查看构建 run、检查双边产物 Release、补跑 `publish-installers.yml` |

推送插件标签或核对 CDN 目录时，直接使用 `node scripts/release-plugins.mjs push-tags --yes` / `verify` 会自动绑定对应账号凭据与 GitHub 代理。

### 8.1 secrets 放在哪

**构建、签名与官网上报用的 secret 分别放在对应构建仓库（主程序 `Agent2Builder/agent2api-releases` 与插件 `Agent2Builder/agent2api-providers-releases`）**，各源码仓库只需保留向对应构建仓库派发事件所需的令牌。

| secret | 在哪 | 用途 | 缺了会怎样 |
|---|---|---|---|
| `AGENT2API_PLUGIN_KEY` | 插件构建仓库 | WASM 插件包 ed25519 官方签名私钥（`keyId: official-2026`） | 产物未签名，网关安装时标黄 |
| `RELEASE_PUBLISH_SECRET` | 主程序构建仓库 | 向 `oyl.agent2api.dev/api/ci/releases` 签名上报主程序发布并更新官网 / CDN | 官网与 CDN feed 不更新，自动更新查不到新版 |
| `PLUGIN_PUBLISH_SECRET` | 插件构建仓库 | 向 `oyl.agent2api.dev/api/ci/plugins` 签名上报 WASM 插件与官方目录 | 官网与 CDN 插件目录不更新 |
| `ANDROID_KEYSTORE_BASE64` / `ANDROID_KEYSTORE_PASSWORD` / `ANDROID_KEY_ALIAS` / `ANDROID_KEY_PASSWORD` | 主程序构建仓库 | APK 签名 | `android` job 直接红 |
| `IOS_CERTIFICATE_BASE64` / `IOS_CERTIFICATE_PASSWORD` / `IOS_PROVISIONING_PROFILE` / `IOS_TEAM_ID` | 主程序构建仓库 | iOS 签名 | 产出未签名 IPA（可 AltStore 重签），不报错 |
| `DOCKERHUB_USERNAME` / `DOCKERHUB_TOKEN` | 主程序构建仓库 | 推 Docker Hub | 镜像工作流明确失败 |
| `GHCR_TOKEN` | 主程序构建仓库 | 推 GHCR。**必须是 aimod-cc 账号**、仅 `write:packages` 的 classic PAT（内置 `GITHUB_TOKEN` 只认构建账号自己的命名空间，跨账号会被拒） | 镜像工作流明确失败 |
| `SOURCE_RELEASE_TOKEN` | **主程序构建仓库** | 同步主仓库 Release，限定源码仓库 Contents: write | `mirror` 明确失败，可单独重跑 |
| `BUILDER_DISPATCH_TOKEN` | **各源码仓库** | `Agent2API`、`Agent2Providers`、`Agent2WebProviders` 向对应构建仓库派发构建事件 | tag 推上去后什么都不会发生 |

**GitHub 的 secret 值读不回来**（设计如此），所以「把 secret 从 A 仓库搬到 B 仓库」只能由人重新填一遍，不能脚本复制。迁移构建仓库时按上表逐项重配。

`BUILDER_DISPATCH_TOKEN` 需要对目标构建仓库具有触发权限（可配为对应仓库 `contents: write` 的 PAT 或授权 token）。

### 8.2 镜像与 GHCR

- **GHCR 新包默认私有**：首次推送后需到包设置手动改 Public，否则别人 `docker pull` 会 401（API 改不了这个默认值）。
- **镜像名不能用 `${GITHUB_REPOSITORY}`**：工作流跑在构建仓库里，那个变量会解析成构建仓库的名字，镜像就推到构建账号的命名空间下去了。`docker.yml` 用 `GHCR_OWNER` / `GHCR_IMAGE` 两个 env 显式指定，换主仓库时连同 `SOURCE_URL`（`org.opencontainers.image.source` 的覆盖值）一起改这三处。
- **包要关联主仓库**：`metadata-action` 默认把 `org.opencontainers.image.source` 填成「工作流所在仓库」（= 构建仓库），跨账号连不上。`docker.yml` 在 `labels:` 里显式重写为主仓库地址，这样 GHCR 的包页面才会指回 `aimod-cc/Agent2API`。

### 8.3 代理

- **gh 不读 git 的代理配置，跑 gh 前要先设代理环境变量**：`gh` 是 Go 程序，不读 `~/.gitconfig` 里的 `http(s).proxy`，也不读 Windows 系统代理（WinINET），只认 `HTTPS_PROXY` / `HTTP_PROXY` 环境变量。本机 GitHub 直连时通时不通，手动执行 gh 命令前先设：

  ```bash
  export HTTPS_PROXY=$(git config --get-urlmatch http.proxy https://github.com || git config --get https.proxy)
  ```

  只设 `HTTPS_PROXY` 就够（gh 的请求全是 HTTPS，实测不读 `HTTP_PROXY`）。`scripts/release.sh` 已内置这一步（从 git 配置读取后透传给 gh），用脚本发版无需手动设。代理没开时 gh 会立刻报 `proxyconnect tcp: ... connection refused` 而不回退直连 —— 与 `git push` 的表现一致。

### 8.4 重配 Android 签名（在构建仓库）

- **APK 签名走四个 secret，缺一个 `android` job 直接红**。密钥本体（`*.jks`）与 `keystore.properties` 都在 `.gitignore` 里，只在打包机与构建仓库 secret 各存一份。来源是打包机 `desktop-tauri/mobile/android/` 下的 `agent2api-release.jks` 与 `keystore.properties`，keyAlias 为 `agent2api`。
- 重配 / 补配的操作（在打包机上执行，值从本地文件读，不要手抄）：

  ```bash
  cd desktop-tauri/mobile/android
  # base64 必须是单行（-w0），带换行会解出损坏的 jks
  base64 -w0 agent2api-release.jks > /tmp/ks.b64
  gh secret set ANDROID_KEYSTORE_BASE64 --repo Agent2Builder/agent2api-releases < /tmp/ks.b64
  # 其余三个直接取 keystore.properties 的值，去掉键名与换行
  sed -n 's/^storePassword=//p' keystore.properties | tr -d '\n' | gh secret set ANDROID_KEYSTORE_PASSWORD --repo Agent2Builder/agent2api-releases
  sed -n 's/^keyAlias=//p'      keystore.properties | tr -d '\n' | gh secret set ANDROID_KEY_ALIAS --repo Agent2Builder/agent2api-releases
  sed -n 's/^keyPassword=//p'   keystore.properties | tr -d '\n' | gh secret set ANDROID_KEY_PASSWORD --repo Agent2Builder/agent2api-releases
  rm -f /tmp/ks.b64
  gh secret list --repo Agent2Builder/agent2api-releases   # 只显示名字与更新时间，值读不回来
  ```

  校验密钥本身是否可用：`keytool -list -keystore agent2api-release.jks -storepass <storePassword>`，应看到别名 `agent2api` 的 `PrivateKeyEntry`。**换密钥的后果**是已安装用户无法覆盖升级（签名不一致），不是重装一遍就好 —— 原密钥丢了等于断了 APK 的升级路径。

### 8.5 其它

- **APK 构建的两处环境差异**（构建仓库的 `release.yml` 已处理，改这块时别删）：`android/gradle.properties` 里的 `org.gradle.java.home` 是本机 Windows 绝对路径，runner 上要删掉回落 `JAVA_HOME`；`gradle-wrapper.properties` 的 `distributionUrl` 指向腾讯云镜像，runner 在海外要换回官方源。
- **APK 的产物名由 `mobile/package.json` 的 version 决定**：`Agent2API-<版本>.apk`，`release.sh` 原样挂上 Release。安装文档六份里写的也是这个名字，改命名规则要连文档一起改。
- **推含工作流改动的提交会被拒**：报错 `refusing to allow an OAuth App to create or update workflow ... without workflow scope`。解决：`gh auth switch --user <账号>` 切到目标账号，再 `gh auth refresh --scopes workflow` 授权一次。注意 `gh auth refresh` **没有 `--user` 参数**，它只作用于当前活动账号；不先 switch 就会出现「浏览器里授权的是 A，refresh 的是 B」的报错。
- **一次推超过 3 个 tag 不会全部触发构建**：GitHub 规定单次 `git push` 携带超过 3 个标签时会跳过 `on: push: tags:` 自动工作流。批量发多个单包插件时按 ≤ 3 个分批推（或使用 `scripts/release-plugins.mjs push-tags --yes`）。
- **切勿用 `git add .` 把本机工具目录带入提交**：若根目录存在 `.antigravitycli/` 等工具/IDE 运行目录或本地链接，提交前用 `git status --short` 核对并显式 `git add` 源码目录，或使用 `scripts/prepare-release.mjs` 先做干净检查。
- **单包插件版本变动后必须刷新 `wasm/Cargo.lock`**：只改 `wasm/<id>/Cargo.toml` 而不刷新锁文件会让 CI 的 `--locked` 检查直接失败；先跑不带 `--locked` 的 `cargo check --workspace --target wasm32-unknown-unknown` 再跑 `--locked`（`scripts/release-plugins.mjs bump` 已内置这两步）。
- **协议库按 tag 引用插件仓库**：`desktop-tauri/src-tauri/server/Cargo.toml` 里 `agent2api-provider-protocol` 引用 `ProvidersPlugin/Agent2Providers` 的协议 tag（具体版本以该行 `tag = ...` 为准）。**只有 `crates/provider-protocol` 的跨端协议本身发生改动时**，才需要先在插件仓库打协议 tag 并更新主程序引用；日常独立 WASM 插件迭代与发版无需修改主程序这里。本地联调若需临时改协议，可用 `.cargo/config.toml` 的 `[patch]` 指向本地路径，**但绝不能把那段提交**。

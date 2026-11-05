# 官方基线与本地改动记录

本文记录同步分支的官方基线、本地改动范围与构建跟进情况，便于后续继续跟进官方更新和本地改动维护。

## 仓库与分支

- 个人 fork：`https://github.com/mojie126/A2API`
- 官方仓库：`https://github.com/aimod-cc/Agent2API`
- 官方基线分支：`main`
- 本地同步分支：`sync/main-2026-11-05`

`main` 只用于同步官方更新，不直接承载本地功能改动。本分支以官方 3.4.2 为基线，保留性能优化、推理折叠与持续集成调整，并承载后续本地改动。

## 官方基线

- 官方版本：`v3.4.2`
- 官方基线提交：`e8bed73`（`main` 当前指向，官方 3.4.2 更新内容）
- 同步提交：`e99f203`（2026-11-05，在官方基线上重新落地本地改动）
- 官方更新重点：自定义路由组、压缩路由、客户端识别、服务端一键更新、MonkeyCode 提供商、签到领取人工验证、网页搜索聚合、账号余额到期展示、移除 LobsterAI 新增账号入口、版本 3.4.1 → 3.4.2

## 本地改动

### 保留的性能与功能改动（同步提交 e99f203）

- 性能优化沿用 performance 分支记录的口径：调试报文限量与及时释放、在途状态合并写入、日志与请求统计批量维护与缓存、请求总数内存缓存、SQLite 检查点与增量回收、图片逐张压缩、终态与限额事件合并事务等，完整清单见 performance 分支 `docs/baseline-and-local-changes_2026-11-04.md`
- 推理折叠：服务端新增 `reasoning_collapse` 模块，设置页与系统信息页提供配置与状态展示
- 持续集成：四条工作流全部只允许手动触发，`fork-build.yml` 更名为 `performance-build`，作为个人 fork 的统一构建入口
- Rust 锁文件与包清单同步更新，保证 Windows 构建依赖可用

主要改动位置：

- `desktop-tauri/src-tauri/server/src/server/api/`（chat、pipeline、protocol、reasoning_collapse、record_persist、stats_api、live_progress 等）
- `desktop-tauri/src-tauri/server/src/server/core/`（debug_traffic、image_compress、limiter、usage_query、usage_records、upstream 等）
- `desktop-tauri/src-tauri/server/src/server/logs_store/`、`request_stats/`、`db/`
- `desktop-tauri/ui-islands/src/islands/`（settings-model、settings-page、settings-state、system-info-pane）
- `desktop-tauri/src-tauri/Cargo.lock`、`desktop-tauri/src-tauri/server/Cargo.toml`
- `.github/workflows/`（build、fork-build、frontend-artifacts、warm-cache 四条工作流）

### 本轮新增改动（2026-11-05，提交 d0b4ac6）

- 路由配置画布连线支持在途请求高亮：账号在途数按提供商（配置页）与成员（路由组页）汇总到画布节点，承载在途请求的连线改为主题色加粗描边并叠加虚线流动动画，端点圆点同步变色
- 系统偏好减少动态效果时停用流动动画，保留高亮描边
- 画布图例补充连线高亮含义，六语词典同步新增文案

改动位置：`desktop-tauri/ui-islands/src/islands/routing-canvas-layout.tsx`、`routing-config-panel.tsx`、`routing-group-panel.tsx`、`routing-page.tsx`，`desktop-tauri/ui/css/page-routing-config.css`，`desktop-tauri/i18n/i18n-src/*/routing.json` 与对应词典产物。

## 构建流程

个人 fork 使用 `.github/workflows/fork-build.yml`（显示名 `performance-build`）手动构建：

- Windows 桌面端安装包，产物名 `windows-nsis`
- Windows 服务端压缩包与校验文件，产物名 `windows-server-amd64`
- `ghcr.io/mojie126/a2api:latest`，仅支持 `linux/amd64`
- Release 按应用版本创建为草稿，由人工确认后再正式发布

仓库内所有 GitHub Actions 工作流只允许手动触发，不响应分支推送、拉取请求、标签推送或定时任务。

## 验证记录

- 前端岛类型检查：通过
- 前端主面板与登录页构建：通过
- 多语言词典构建与缺失键体检：通过
- frontend-artifacts 运行 `42021734220`：云端重建全部前端产物并逐字节同源校验，通过
- performance-build 运行 `42022820095`：Windows 桌面端、Windows 服务端与 `linux/amd64` 镜像全部成功；首次尝试仅「创建草稿 Release」失败，报错 `Tried to update "v3.4.2" which is neither a draft or prerelease (updateOnlyUnreleased is on)`，与已发布的 v3.4.2 Release 冲突；重跑后全部成功
- 本轮新增改动仅涉及前端、样式与多语言词典，未触及服务端代码

## 后续同步官方更新

1. 在 `main` 执行 `git fetch origin --prune` 并快进到官方最新版本
2. 切回本分支，按本轮相同方式把官方内容落到本分支，保持本地改动清单完整
3. 重点检查 `desktop-tauri/src-tauri/server` 下的调试报文、请求管线、日志存储、请求统计与数据库模块，以及 `.github/workflows` 与推理折叠相关文件
4. 重新运行服务端编译和测试、前端类型检查与构建、词典构建
5. 手动运行 `performance-build`，检查 Windows 产物与 `linux/amd64` 镜像
6. 更新本文的官方版本、基线提交与验证记录

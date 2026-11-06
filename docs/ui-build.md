# 前端（UI 岛）构建

面板前端是仓库里的两份 React 岛，**产物检入仓库**：`desktop-tauri/ui/islands/ui.js`、`ui.css`、`login.js`。改完 `.tsx` 不构建，等于没改。

## 两个包，别漏装

| 目录 | 角色 | 为什么必须装 |
|---|---|---|
| `desktop-tauri/ui-islands` | 岛本体（vite 入口） | 构建就发生在这里 |
| `desktop-tauri/ui-kit` | 私有组件库 `@agent2api/ui` | `vite.config.ts` 把 `@ui` 别名指到它的 **源码**（`../ui-kit/src`），而它有**独立的 `package.json`** |

**只装 `ui-islands` 会失败**，症状分两段，容易误判成代码问题：

1. `tsc --noEmit` 报一片 `Parameter 'value' implicitly has an 'any' type` —— 不是真类型错误，是 `ui-kit` 的 `@base-ui/react` 类型解析不到，回调参数退化成 `any`；
2. 跳过 tsc 后 `vite build` 报 `Can't resolve 'tw-animate-css'` —— 那是 `ui-kit` 的 devDependency，由 `ui-kit/styles/globals.css` 引入。

两处都是**装完 `ui-kit` 依赖即刻消失**。

## 步骤

```bash
# 一次
cd desktop-tauri/ui-kit && npm install
cd ../ui-islands && npm install

# 每次改完前端
cd desktop-tauri/ui-islands && npm run build   # tsc --noEmit && vite build && vite build --config vite.login.config.ts
```

产物落在 `desktop-tauri/ui/islands/`。**裸机 / headless 网关读的是 `dist/ui/`**（`run-headless.ps1` 里 `AGENT2API_UI_DIR` 指向它），所以要再同步一次：

```powershell
Copy-Item desktop-tauri\ui\islands\* dist\ui\islands\ -Force
```

前端新增的静态资源（如 `assets/providers/<id>.png`）同样两边都要有：`desktop-tauri/ui/assets/` 与 `dist/ui/assets/`。

## 用 pnpm 代替 npm 时的两个坑

仓库标准是 npm（`ui-islands/package-lock.json` 在版本控制里）。如果你本地用 pnpm：

1. pnpm 10+ 默认拦掉依赖的构建脚本，`pnpm install` 会以 `ERR_PNPM_IGNORED_BUILDS: esbuild` 退出，**且 `pnpm run build` 会在跑脚本前做一次校验安装，于是连 vite 都起不来**。放行方式二选一：
   - `pnpm approve-builds` 交互式勾选；
   - 或在 `ui-islands/package.json` 加 `"pnpm": { "onlyBuiltDependencies": ["esbuild"] }`（本仓库**没有**提交这一段，因为标准是 npm）。
2. 在 `ui-kit` 里跑 `pnpm install` 会生成 `pnpm-lock.yaml`，不要提交它 —— 仓库用的是 `package-lock.json`。只想装依赖不写锁文件时用 `pnpm install --lockfile=false --ignore-scripts`。

## 别做这两件事

- **不要手改 `ui.js`**：产物是 minified 单行文件，手工插入一条映射能跑，但下次任何人重建都会覆盖，而且 rebuild 后是整包 diff（84 行全变），混着手改会看不出什么才是真改动。
- **不要只改 `dist/ui`**：那是运行时拷贝，下一次发版/构建就没了；源在 `desktop-tauri/ui/` 与 `ui-islands/src/`。

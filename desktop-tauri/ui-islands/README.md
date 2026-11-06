# ui-islands（面板前端「岛」）

面板前端的 React 岛源码。产物构建到 `../ui/islands/`（`ui.js` / `ui.css` / `login.js`，**检入仓库**）。

## 先装两个包的依赖，别只装这里

```bash
cd desktop-tauri/ui-kit     && npm install   # ← 必须。漏了它，下面的构建一定会失败
cd ../ui-islands            && npm install
```

`vite.config.ts` 把 `@ui` 别名指向 `../ui-kit/src`（源码，不是打包产物），而 `ui-kit` 有**独立的 package.json**。只装 `ui-islands` 时的两个典型症状（都不是代码问题）：

- `tsc --noEmit` 报一片 `Parameter 'value' implicitly has an 'any' type` —— `ui-kit` 的 `@base-ui/react` 类型解析不到，回调参数退化成 `any`；
- 跳过 tsc 后 `vite build` 报 `Can't resolve 'tw-animate-css'` —— 那是 `ui-kit` 的 devDependency，由 `ui-kit/styles/globals.css` 引入。

装完 `ui-kit` 依赖，两者同时消失。

## 构建

```bash
npm run build     # tsc --noEmit && vite build && vite build --config vite.login.config.ts
```

产物在 `../ui/islands/`。裸机 / headless 网关读的是 `dist/ui/`（`run-headless.ps1` 的 `AGENT2API_UI_DIR`），改完要再同步一次：

```powershell
Copy-Item desktop-tauri\ui\islands\* dist\ui\islands\ -Force
```

新增静态资源（如 `assets/providers/<id>.png`）两边都要放：`desktop-tauri/ui/assets/` 与 `dist/ui/assets/`。

## 用 pnpm 的话

仓库标准是 npm（`package-lock.json` 在版本控制里）。pnpm 10+ 默认拦依赖构建脚本，会以 `ERR_PNPM_IGNORED_BUILDS: esbuild` 退出，且 `pnpm run build` 在跑脚本前先做一次校验安装，会连 vite 都起不来。放行：`pnpm approve-builds`，或临时加 `"pnpm": { "onlyBuiltDependencies": ["esbuild"] }`（本仓库未提交这段）。在 `ui-kit` 里用 pnpm 时别提交它生成的 `pnpm-lock.yaml`。

## 别手改产物

`ui.js` 是 minified 单行文件：手插一条映射能跑，但任何人重建都会覆盖，而且 rebuild 后是整包 diff（约 84 行全变），手改痕迹会淹没真改动。

完整的构建 / 排错说明见 [docs/ui-build.md](../../docs/ui-build.md)。

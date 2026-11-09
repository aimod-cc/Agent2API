# models.dev 模型能力补齐跟进

## 目标

模型管理使用 models.dev 补充各提供商目录没有声明的对下游能力，包括上下文窗口、最大输出、工具调用、图片、视频、思考支持和逐模型思考档位，并标明每个值的来源。

## 已实现

- 模型身份解析顺序为：用户精确绑定、适配器 `modelsDevId`、canonical 标识、精确模型 ID、同提供商名称对齐、全局唯一名称。
- `models.dev` 精简索引保留模型名称；归一化键冲突时保留全部记录，不再静默覆盖。
- 手动绑定支持 canonical 标识、源展示名和名称对齐开关。
- 自定义提供商别名从目标模型继承元数据，不再拿别名本身查询 models.dev。
- 设置页手动刷新沿用模型目录更新任务的实际间隔，不再将后续自动刷新推迟一天。
- models.dev 请求支持 gzip、两分钟总超时和分类错误提示。
- 模型目录更新任务行增加代理下拉框，选项来自网络代理池，默认直连。

## 数据与优先级

能力值优先级为：

1. 用户在模型能力弹窗中的覆盖值。
2. 提供商目录已经声明的值。
3. models.dev 补齐值。
4. 未声明。

名称对齐仅在同一提供商内生效，名称归一化后必须只有一个候选。精确模型 ID 绑定、适配器标识和 canonical 标识始终优先于名称匹配。

## 网络说明

- 模型目录更新任务中的代理选择复用软件更新的公网出口设置，因此两处共享同一代理。
- models.dev 的 `api.json` 与 `models.json` 体积较大，直连或代理带宽不足时会读取超时。
- 运行期请求 `Accept-Encoding: gzip` 并显式解压；不支持的压缩编码会明确报错。
- 超时不会覆盖已有的内嵌快照或本地缓存，旧数据继续生效。

## 已知边界

- 仓库内旧快照没有模型名称，首次成功联网刷新后，models.dev 自身的全局名称索引才会完整。
- 任意模型 ID，例如 `sn-*`，不能从名称形态推断身份，必须由适配器或用户显式绑定。
- 历史失败冷却会继续保存在任务状态中；修复网络后可从任务行点击立即更新越过冷却。
- 模型目录更新中的失败项可能来自具体提供商，例如 OfficeAce 返回 401，与 models.dev 失败是两条独立结果。

## 维护检查

```powershell
cargo test -p agent2api-server model_meta --lib
cargo test -p agent2api-server model_meta::refresh::tests --lib
cargo check -p agent2api-server
cd desktop-tauri/ui-islands
node node_modules/typescript/bin/tsc --noEmit
node node_modules/vite/bin/vite.js build
```

发布前还应确认更新后的 `ui/islands/ui.js`、模型域五份翻译文件和本地桌面端 release 构建一致。

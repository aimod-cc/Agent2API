# 重置面板管理员（忘记密码）

**简体中文** | [English](./reset-admin.en.md) | [繁體中文](./reset-admin.zh-Hant.md) | [日本語](./reset-admin.ja.md) | [한국어](./reset-admin.ko.md) | [Português (BR)](./reset-admin.pt-BR.md)

本页解决一种情况：**忘了面板的管理员密码，进不去界面**。

密码只以 bcrypt 哈希存在数据库里，无法「找回」——只能把管理员记录**删掉**，
让面板回到「未注册」状态，再重新注册一个。这个操作会清空管理员账号与密码，
但不影响账号池、网关 Key、日志与其它所有业务数据。

> 如果你还能登录，请不要用本页的办法：去 **设置 → 账户 → 修改密码**，
> 那里需要输入旧密码，改完会自动撤销其他设备上的登录。

## 一、先停掉程序

删除记录必须在程序**完全停止**时做。数据库以 WAL 模式运行，进程还在跑的时候
写入可能被它覆盖，或者被锁住。

| 形态 | 停止方式 |
| --- | --- |
| Windows 桌面端 | 托盘图标右键 → **退出**（只关窗口不够：默认是关到托盘，进程还在） |
| macOS / Linux 桌面端 | 托盘 / 菜单栏图标 → **退出**（macOS 也可 `Cmd+Q` 之后确认托盘图标已消失） |
| Windows 服务端 | 管理员 PowerShell：`Stop-Service Agent2API` |
| Linux 服务端 | `sudo systemctl stop agent2api` |
| Docker | 在 compose 目录执行 `docker compose stop` |

## 二、找到数据库文件

数据库是一个 SQLite 文件 `agent2api.db`，位于「配置目录」下：

| 形态 | 路径 |
| --- | --- |
| Windows（桌面端 / 服务端） | `%USERPROFILE%\.agent2api\agent2api.db` |
| macOS / Linux（桌面端 / 服务端） | `~/.agent2api/agent2api.db` |
| Docker（compose 默认） | 宿主上的 `./data/agent2api.db`（容器内是 `/data/agent2api.db`） |

如果设过 `AGENT2API_PROXY_HOME`，配置目录就是该变量的值。

面板的 **设置 → 版本信息 → 数据文件** 一栏显示的就是这个文件的完整路径
（还能登录时先记下来最省事）。

## 三、删除管理员记录

管理员存在 `kv` 表里，键名是 **`panelAdmin`**。

### 有 `sqlite3` 命令

```bash
# Windows / macOS / Linux 桌面端与服务端
sqlite3 ~/.agent2api/agent2api.db "DELETE FROM kv WHERE key='panelAdmin';"

# Windows 的路径写法（PowerShell 或 CMD）
sqlite3 "%USERPROFILE%\.agent2api\agent2api.db" "DELETE FROM kv WHERE key='panelAdmin';"

# Docker：先停容器，再从宿主的 data 目录删
docker compose stop
sqlite3 ./data/agent2api.db "DELETE FROM kv WHERE key='panelAdmin';"
```

### 没有 `sqlite3`

任一 SQLite 图形工具都可以（DB Browser for SQLite、DBeaver、Navicat 等）：
打开 `agent2api.db` → 找到 `kv` 表 → 删除 `key` 等于 `panelAdmin` 的那一行 →
保存 / 写入。

也可以用 Python（大多数系统自带）：

```bash
python -c "import sqlite3,os; p=os.path.expanduser('~/.agent2api/agent2api.db'); c=sqlite3.connect(p); c.execute(\"DELETE FROM kv WHERE key='panelAdmin'\"); c.commit(); c.close(); print('done')"
```

## 四、重启并重新注册

重新启动程序（Docker 用 `docker compose start`），打开面板：

- **桌面端**：窗口会直接进入主界面（没有管理员 = 不需要登录）。想重新设置密码，
  就打开 **设置 → 账户** —— 未注册时会提示去登录页注册；也可以直接访问
  `http://127.0.0.1:3065/login` 完成注册。
- **服务端 / Docker**：打开面板地址会自动跳到登录页，页面会显示
  **「首次使用：创建一个管理员账号」**，填用户名与密码即可。

注册完成后，面板重新要求登录。

## 五、注意：环境变量只在首次生效

如果部署时配过：

```bash
AGENT2API_ADMIN_USER=admin
AGENT2API_ADMIN_PASSWORD=your-password
```

它们**只用于首次播种** —— 数据库里第一次没有管理员时写入一次，之后完全以数据库
为准（这样才能在面板里改密码）。因此：

- 改掉这两个环境变量**不会**改掉已注册的密码；
- 删掉 `panelAdmin` 记录后重启，若环境变量还在，会用它们**重新播种**一个管理员
  账号 —— 这时「新密码」就是环境变量里那个。

如果不希望它自动重建，先把环境变量从部署配置里去掉再重启。

同一组配置里还有 `AGENT2API_ADMIN_PASSWORD_HASH`（bcrypt 哈希，优先于明文密码），
行为与上面一致。

## 六、安全提醒

删掉管理员记录后，面板在「未注册」状态下是**免登录**的：

- 桌面端：只有本机能访问，风险有限；但如果开着「允许局域网访问」，
  局域网内的设备也能打开管理面板 —— 请尽快完成重新注册。
- 服务端 / Docker：**任何能访问面板端口的人都能接管管理面**。删完请立刻重启并注册，
  不要在中间停留。管理端口暴露在公网时尤其要注意。

如果不小心把数据库改坏了，先别继续操作：数据库文件旁边可能还有
`backups/` 目录（整库恢复前自动生成的安全备份），里面的 `pre-restore-*.db`
可以整份拷回来替换。

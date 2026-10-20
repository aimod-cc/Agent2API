# 重設面板管理員（忘記密碼）

[简体中文](./reset-admin.md) | [English](./reset-admin.en.md) | **繁體中文** | [日本語](./reset-admin.ja.md) | [한국어](./reset-admin.ko.md) | [Português (BR)](./reset-admin.pt-BR.md)

本頁處理一種情況：**忘記面板的管理員密碼，進不去介面**。

密碼只以 bcrypt 雜湊存在資料庫裡，無法「找回」——只能把管理員記錄**刪掉**，
讓面板回到「未註冊」狀態，再重新註冊一個。這個操作會清空管理員帳號與密碼，
但不影響帳號池、閘道 Key、日誌與其他所有業務資料。

> 如果你還能登入，請不要用本頁的辦法：到 **設定 → 帳戶 → 修改密碼**，
> 那裡需要輸入舊密碼，改完會自動撤銷其他裝置上的登入。

## 一、先停掉程式

刪除記錄必須在程式**完全停止**時做。資料庫以 WAL 模式執行，行程還在跑的時候
寫入可能被它覆蓋，或者被鎖住。

| 形態 | 停止方式 |
| --- | --- |
| Windows 桌面端 | 托盤圖示右鍵 → **結束**（只關視窗不夠：預設是關到托盤，行程還在） |
| macOS / Linux 桌面端 | 托盤 / 選單列圖示 → **結束**（macOS 也可 `Cmd+Q` 之後確認托盤圖示已消失） |
| Windows 服務端 | 管理員 PowerShell：`Stop-Service Agent2API` |
| Linux 服務端 | `sudo systemctl stop agent2api` |
| Docker | 在 compose 目錄執行 `docker compose stop` |

## 二、找到資料庫檔案

資料庫是一個 SQLite 檔案 `agent2api.db`，位於「設定目錄」下：

| 形態 | 路徑 |
| --- | --- |
| Windows（桌面端 / 服務端） | `%USERPROFILE%\.agent2api\agent2api.db` |
| macOS / Linux（桌面端 / 服務端） | `~/.agent2api/agent2api.db` |
| Docker（compose 預設） | 主機上的 `./data/agent2api.db`（容器內是 `/data/agent2api.db`） |

如果設過 `AGENT2API_PROXY_HOME`，設定目錄就是該變數的值。

面板的 **設定 → 版本資訊 → 資料檔案** 一欄顯示的就是這個檔案的完整路徑
（還能登入時先記下來最省事）。

## 三、刪除管理員記錄

管理員存在 `kv` 表裡，鍵名是 **`panelAdmin`**。

### 有 `sqlite3` 指令

```bash
# Windows / macOS / Linux 桌面端與服務端
sqlite3 ~/.agent2api/agent2api.db "DELETE FROM kv WHERE key='panelAdmin';"

# Windows 的路徑寫法（PowerShell 或 CMD）
sqlite3 "%USERPROFILE%\.agent2api\agent2api.db" "DELETE FROM kv WHERE key='panelAdmin';"

# Docker：先停容器，再從主機的 data 目錄刪
docker compose stop
sqlite3 ./data/agent2api.db "DELETE FROM kv WHERE key='panelAdmin';"
```

### 沒有 `sqlite3`

任一 SQLite 圖形工具都可以（DB Browser for SQLite、DBeaver、Navicat 等）：
開啟 `agent2api.db` → 找到 `kv` 表 → 刪除 `key` 等於 `panelAdmin` 的那一列 →
儲存 / 寫入。

也可以用 Python（大多數系統內建）：

```bash
python -c "import sqlite3,os; p=os.path.expanduser('~/.agent2api/agent2api.db'); c=sqlite3.connect(p); c.execute(\"DELETE FROM kv WHERE key='panelAdmin'\"); c.commit(); c.close(); print('done')"
```

## 四、重新啟動並重新註冊

重新啟動程式（Docker 用 `docker compose start`），開啟面板：

- **桌面端**：視窗會直接進入主介面（沒有管理員 = 不需要登入）。想重新設定密碼，
  就開啟 **設定 → 帳戶** —— 未註冊時會提示去登入頁註冊；也可以直接存取
  `http://127.0.0.1:3065/login` 完成註冊。
- **服務端 / Docker**：開啟面板位址會自動跳到登入頁，頁面會顯示
  **「首次使用：建立一個管理員帳號」**，填使用者名稱與密碼即可。

註冊完成後，面板重新要求登入。

## 五、注意：環境變數只在首次生效

如果部署時設定過：

```bash
AGENT2API_ADMIN_USER=admin
AGENT2API_ADMIN_PASSWORD=your-password
```

它們**只用於首次播種** —— 資料庫裡第一次沒有管理員時寫入一次，之後完全以資料庫
為準（這樣才能在面板裡改密碼）。因此：

- 改掉這兩個環境變數**不會**改掉已註冊的密碼；
- 刪掉 `panelAdmin` 記錄後重新啟動，若環境變數還在，會用它們**重新播種**一個管理員
  帳號 —— 這時「新密碼」就是環境變數裡那個。

如果不希望它自動重建，先把環境變數從部署設定裡去掉再重新啟動。

同一組設定裡還有 `AGENT2API_ADMIN_PASSWORD_HASH`（bcrypt 雜湊，優先於明文密碼），
行為與上面一致。

## 六、安全提醒

刪掉管理員記錄後，面板在「未註冊」狀態下是**免登入**的：

- 桌面端：只有本機能存取，風險有限；但如果開著「允許區域網路存取」，
  區域網路內的裝置也能開啟管理面板 —— 請儘快完成重新註冊。
- 服務端 / Docker：**任何能存取面板連接埠的人都能接管管理面**。刪完請立刻重新啟動並註冊，
  不要在中間停留。管理連接埠暴露在公網時尤其要注意。

如果不小心把資料庫改壞了，先別繼續操作：資料庫檔案旁邊可能還有
`backups/` 目錄（整庫還原前自動產生的安全備份），裡面的 `pre-restore-*.db`
可以整份拷回來替換。

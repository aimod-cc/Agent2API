# パネル管理者のリセット（パスワードを忘れた場合）

[简体中文](./reset-admin.md) | [English](./reset-admin.en.md) | [繁體中文](./reset-admin.zh-Hant.md) | **日本語** | [한국어](./reset-admin.ko.md) | [Português (BR)](./reset-admin.pt-BR.md)

このページは「**パネルの管理者パスワードを忘れてサインインできない**」場合の手順です。

パスワードは bcrypt ハッシュとしてのみ保存されているため「取り出す」ことはできません。
管理者レコードを**削除**してパネルを「未登録」状態に戻し、登録し直すしかありません。
この操作で消えるのは管理者アカウントとパスワードだけで、アカウントプール、ゲートウェイ
キー、ログ、その他の業務データには影響しません。

> まだサインインできる場合はこのページを使わないでください。
> **設定 → アカウント → パスワード変更** を使います（現在のパスワードが必要で、
> 変更後は他の端末のサインインが自動的に無効になります）。

## 1. プログラムを停止する

レコードの削除はプログラムが**完全に停止**している状態で行ってください。データベースは
WAL モードで動作しており、実行中に書き込むと上書きされたりロックされたりします。

| 形態 | 停止方法 |
| --- | --- |
| Windows デスクトップ | トレイアイコンを右クリック → **終了**（ウィンドウを閉じるだけでは不十分。既定ではトレイに格納されプロセスは残ります） |
| macOS / Linux デスクトップ | トレイ / メニューバーのアイコン → **終了**（macOS は `Cmd+Q` の後、トレイアイコンが消えたことを確認） |
| Windows サーバー | 管理者 PowerShell：`Stop-Service Agent2API` |
| Linux サーバー | `sudo systemctl stop agent2api` |
| Docker | compose ディレクトリで `docker compose stop` |

## 2. データベースファイルを探す

データベースは SQLite の単一ファイル `agent2api.db` で、「設定ディレクトリ」の下にあります。

| 形態 | パス |
| --- | --- |
| Windows（デスクトップ / サーバー） | `%USERPROFILE%\.agent2api\agent2api.db` |
| macOS / Linux（デスクトップ / サーバー） | `~/.agent2api/agent2api.db` |
| Docker（compose の既定） | ホスト上の `./data/agent2api.db`（コンテナ内は `/data/agent2api.db`） |

`AGENT2API_PROXY_HOME` を設定している場合、設定ディレクトリはその値になります。

パネルの **設定 → バージョン情報 → データファイル** に完全なパスが表示されます
（サインインできるうちに控えておくと簡単です）。

## 3. 管理者レコードを削除する

管理者は `kv` テーブルにキー **`panelAdmin`** で格納されています。

### `sqlite3` がある場合

```bash
# Windows / macOS / Linux のデスクトップとサーバー
sqlite3 ~/.agent2api/agent2api.db "DELETE FROM kv WHERE key='panelAdmin';"

# Windows のパス表記（PowerShell または CMD）
sqlite3 "%USERPROFILE%\.agent2api\agent2api.db" "DELETE FROM kv WHERE key='panelAdmin';"

# Docker：先にコンテナを停止し、ホストの data ディレクトリから削除
docker compose stop
sqlite3 ./data/agent2api.db "DELETE FROM kv WHERE key='panelAdmin';"
```

### `sqlite3` がない場合

SQLite の GUI ツール（DB Browser for SQLite、DBeaver、Navicat など）でも可能です。
`agent2api.db` を開く → `kv` テーブルを開く → `key` が `panelAdmin` の行を削除 →
保存 / 書き込み。

Python（多くの環境に同梱）でも可能です。

```bash
python -c "import sqlite3,os; p=os.path.expanduser('~/.agent2api/agent2api.db'); c=sqlite3.connect(p); c.execute(\"DELETE FROM kv WHERE key='panelAdmin'\"); c.commit(); c.close(); print('done')"
```

## 4. 再起動して登録し直す

プログラムを再起動し（Docker は `docker compose start`）、パネルを開きます。

- **デスクトップ**：そのままメイン画面に入ります（管理者なし = サインイン不要）。
  パスワードを設定し直すには **設定 → アカウント** を開くとログインページでの登録を
  案内されます。`http://127.0.0.1:3065/login` を直接開いて登録しても構いません。
- **サーバー / Docker**：パネルを開くとログインページにリダイレクトされ、
  **「初回利用：管理者アカウントを作成」** と表示されます。ユーザー名とパスワードを
  入力して登録してください。

登録後は再びサインインが必要になります。

## 5. 注意：環境変数は初回のみ有効

デプロイ時に以下を設定していた場合：

```bash
AGENT2API_ADMIN_USER=admin
AGENT2API_ADMIN_PASSWORD=your-password
```

これらは**初回のシード専用**です。データベースに管理者が存在しない最初の一度だけ書き込まれ、
以降はデータベースが正となります（パネルからパスワードを変更できるのはこのためです）。
したがって：

- これらの環境変数を変更しても、**登録済みのパスワードは変わりません**。
- `panelAdmin` を削除して再起動したとき環境変数が残っていると、それらで管理者が
  **再シード**されます。この場合の「新しいパスワード」は環境変数の値です。

自動で再作成されたくない場合は、再起動前にデプロイ設定から環境変数を外してください。

同じ設定群に `AGENT2API_ADMIN_PASSWORD_HASH`（bcrypt ハッシュ、平文パスワードより優先）
もありますが、挙動は同じです。

## 6. セキュリティ上の注意

管理者レコードを削除すると、パネルは「未登録」状態で**認証なし**になります。

- デスクトップ：このマシンからしか到達できないためリスクは限定的ですが、
  「LAN アクセスを許可」が有効だと LAN 上の端末からもパネルを開けます。
  速やかに登録し直してください。
- サーバー / Docker：**パネルのポートに到達できる誰もが管理面を掌握できます**。
  削除後は直ちに再起動して登録してください。管理ポートがインターネットに露出している
  場合は特に注意が必要です。

データベースを誤って壊してしまった場合は、それ以上操作せずに確認してください。
データベースファイルの隣に `backups/` ディレクトリ（全体復元の前に自動作成される
安全バックアップ）があり、`pre-restore-*.db` をコピーして戻せば復旧できます。

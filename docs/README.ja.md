<div align="center">

# Agent2API

**複数の AI デスクトップクライアントのログイン状態を、ローカルの OpenAI 互換 API ゲートウェイに**

[简体中文](../README.md) | [English](./README.en.md) | [繁體中文](./README.zh-Hant.md) | **日本語** | [한국어](./README.ko.md) | [Português (BR)](./README.pt-BR.md)

[![license](https://img.shields.io/badge/license-MIT%20%2B%20告知-green)](../LICENSE)
[![release](https://img.shields.io/github/v/release/Agent2API/agent2api?include_prereleases)](https://github.com/Agent2API/agent2api/releases/latest)
[![docker pulls](https://img.shields.io/docker/pulls/aimodcc/agent2api)](https://hub.docker.com/r/aimodcc/agent2api)
[![stars](https://img.shields.io/github/stars/Agent2API/agent2api?style=flat&color=yellow)](https://github.com/Agent2API/agent2api/stargazers)
[![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux%20%7C%20Docker-blue)](https://github.com/Agent2API/agent2api/releases/latest)

[✨ 主な機能](#-主な機能) • [💬 コミュニティとサポート](#-コミュニティとサポート) • [🤝 対応プロバイダー](#-対応プロバイダー) • [🚀 クイックスタート](#-クイックスタート) • [📦 インストールとデプロイ](./install.ja.md) • [📸 画面プレビュー](#-画面プレビュー) • [🔧 開発とビルド](#-開発とビルド) • [📜 使用に関する告知](#-使用に関する告知)

</div>

---

Agent2API は、複数の AI デスクトップクライアントでログイン済みのアカウントを 1 つにまとめ、OpenAI 互換のエンドポイントとして使えるようにします。カスタム `base_url` に対応したクライアントに `http://127.0.0.1:3065/v1` を設定すれば、これらのアカウントのモデル枠をそのまま呼び出せます —— **上流の API キーも、クライアントの変更も不要**です。

## ✨ 主な機能

- 🔌 **OpenAI 互換** —— クライアントに `base_url` を設定するだけ。Chat / Responses / Messages の 3 プロトコルに対応
- 🏪 **17 社の組み込みプロバイダー** —— WorkBuddy、小浣熊、CatPaw、Qoder、Trae などがすぐ使え、カスタムプロバイダーにも対応
- 🔁 **認証情報の自動更新** —— トークンは期限前に自動更新、期限切れ間近のものは一括更新、モデルカタログも追従
- 🪂 **マルチアカウントのローテーション** —— 優先度順にキューイングし、429 は次のアカウントへ。レート制限中は待たずにスキップ
- 🛡️ **外向きプロキシ** —— アカウントごとに出口 IP を割り当て、送信フィンガープリントは公式クライアントを装う
- 🔑 **ゲートウェイのセキュリティ** —— 権限を分けた複数のゲートウェイキー、ログイン時の人間確認、キー未設定なら既定で拒否
- 📊 **リクエストレポート** —— リクエスト数・成功率・トークン消費を一覧、キャッシュヒット率と 365 日ヒートマップ付き
- ⏰ **スケジュールタスク** —— 自動チェックイン、認証情報メンテナンス、モデルカタログ更新を 1 ページで管理
- 💰 **残高と使用量** —— アカウント行に残高と使用量を表示、複数アカウントの一括照会にも対応
- 🖥️ **4 つの形態** —— Windows / macOS / Linux のデスクトップアプリ（トレイ常駐・Node 不要）と、パネルとゲートウェイが同一ポートの Docker 版（amd64 / arm64）。UI は 6 言語

## ❤️ スポンサー

<a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><img src="../assets/sponsor-proxylane.png" alt="ProxyLane（スポンサー）" width="100%" /></a>

<table>

<tr>
<td width="240"><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><picture><source media="(prefers-color-scheme: dark)" srcset="../assets/proxylane-logo-for-dark-bg.png" /><img src="../assets/proxylane-logo-for-light-bg.png" alt="ProxyLane" width="220" /></picture></a></td>
<td><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme">ProxyLane</a> が本プロジェクトをスポンサーしています！ProxyLane は AI アカウント専用のネイティブ静的住宅用 IP（Static Residential ISP）を提供しています。実在の家庭向けブロードバンド事業者の IP を使い、Scamalytics の不正スコアが低く、IP は 1 つずつ専有、通信量は無制限、$3.90 から。「ネットワークプロキシ」ページでアカウントごとに出口を 1 つ割り当てるだけです。初回購入はコード <code>AGENT2API25</code> で 25% オフ。</td>
</tr>

</table>

## 💬 コミュニティとサポート

- **QQ グループ**: Agent2API の交流グループ —— **グループ 1（`1127656296`）はまもなく満員のため、グループ 2（`1130894674`）への参加をお願いします**（QQ で番号を検索して参加できます）。
- **Telegram グループ**: <https://t.me/+9gM8YPvgRL02NGRl>
- **Discord サーバー**: <https://discord.gg/z9GzSJMWp>
- **💖 支援**: Agent2API がお役に立ったら、[作者にコーヒーをおごる](./SPONSOR.md)のも歓迎です。支援は任意で、機能の解放などはありません。
- **🤝 スポンサー / ビジネス**: スポンサーになりたい、またはここに製品を掲載したい場合は [541675139@qq.com](mailto:541675139@qq.com) までメールでご連絡ください。

## 🤝 対応プロバイダー

各プラットフォームのリバースプロキシ対応状況（✓ 対応 · ✗ 非対応 · — 該当なし）:

| プラットフォーム | LLM リクエスト | トークン自動更新 | モデル一覧 | 残高照会 | チェックイン | 受け取り系 |
| --- | :--: | :--: | :--: | :--: | :--: | :--: |
| WorkBuddy 中国版 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| WorkBuddy 国際版 | ✓ | ✓ | ✓ | ✓ | ✗ | — |
| 小浣熊 | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| CatPaw | ✓ | ✗ | ✓ | ✓ | ✗ | — |
| AutoClaw（中国版 / 国際版） | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Qoder | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Cline（Free / Pass） | ✓ | ✓ | ✓ | ✓ | — | — |
| Accio（国際版 / 中国版） | ✓ | ✓ | ✓ | ✓ | — | — |
| ZCode（中国版 / 国際版） | ✓ | ✗ | ✗ | ✓ | — | ✓ |
| CodeArts | ✓ | ✓ | ✓ | ✓ | — | ✓ |
| Trae | ✓ | ✓ | ✓ | ✓ | — | — |
| Loomy（iFlytek） | ✓ | ✗ | ✓ | ✓ | ✓ | — |
| KukuAI（Baidu Wenku） | ✓ | ✗ | ✓ | ✓ | ✓ | — |
| MonkeyCode（長亭科技、中国版 / 国際版） | ✓ | ✗ | ✓ | — | — | — |
| Command Code | ✓ | ✗ | ✓ | — | — | — |
| Antigravity（Google、Gemini / Claude / GPT-OSS） | ✓ | ✓ | ✓ | — | — | — |
| OfficeAce（Huawei Cloud、OfficeClaw） | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| OrcaTerm（Tencent Cloud） | ✓ | ✓ | ✓ | ✓ | — | — |
| 智谱清言（Zhipu Qingyan） | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| カスタムプロバイダー | ✓ | — | ✓ | — | — | — |

> 注：モデル一覧の ✓ はオンラインでの自動取得に対応することを意味します（一部のプラットフォームは内蔵の静的テーブルも併用）。ZCode は内蔵の静的テーブルのみです。トークン自動更新の ✗ は、そのプラットフォームに利用可能な更新の仕組みがないことを意味します（アカウントは有効期間内はそのまま利用できます）。Qoder のチェックインは中国版のみです。
>
> **智谱清言** は本プロジェクトで**唯一「ゲストモード」に対応**したプロバイダです。ログイン不要で、ワンクリックでそのまま会話に使える資格情報を取得できます（無料枠は上流がデバイス単位で発行します）。代わりにゲストアカウントにはポイント体系がありません —— 上表の「残高照会 / サインイン / 受け取り」、および**タスクセンターの初心者タスクとデイリータスク**は**ログイン済みアカウント**にのみ有効で（トークンを貼り付けるかクライアントのログイン状態をインポート）、ゲストではスキップされます。

`/v1/chat/completions`、`/v1/responses`、`/v1/messages`（`count_tokens` を含む）と `/v1/models` はすべてのプラットフォームで同じように扱われます。モデルマッピング、グローバル優先度キュー、429 フォールバック、外向きプロキシ、送信フィンガープリントの秘匿化、リクエストレポートも全プラットフォーム共通です。

> **本プロジェクトは学習・交流目的のみです。** 非公式クライアントの形態で自分のアカウントのログイン状態を再利用するため、上流サービスの規約に適合しない可能性があります。リスク（アカウントの凍結などを含む）は利用者自身が負担し、商用利用・課金の回避は禁止です。詳しくは[使用に関する告知](#-使用に関する告知)と [LICENSE](../LICENSE) をご覧ください。
>
> 本プロジェクトは個人用途のローカルプロキシツールであり、各上流ベンダーおよびその公式製品とは一切関係ありません（一覧は[使用に関する告知](#-使用に関する告知)）。インターフェース仕様は各社クライアントの通信の観察によるもので、上流はいつでも変更される可能性があります。

## 🚀 クイックスタート

3 つの形態からお使いの環境に合うものを選んでください。**インストール・アップグレード・アンインストール・トラブルシューティングの詳細は [インストールとデプロイ](./install.ja.md) にあります**。

| 形態 | 成果物 | インターフェース |
| --- | --- | --- |
| **デスクトップ**（Windows / macOS / Linux） | `Agent2API_<バージョン>_x64-setup.exe`、`_universal.dmg`、`_amd64.deb`、`_amd64.AppImage` | アプリウィンドウ |
| **サーバー**（Windows Server / Linux、GUI なし） | `agent2api-server-windows-amd64.zip`、`agent2api-server-linux-<arch>.tar.gz` | ブラウザーパネル |
| **Docker** | `aimodcc/agent2api:latest`（amd64 / arm64） | ブラウザーパネル |

**デスクトップ**: インストールして起動 → 「アカウント」ページでアカウントを追加 → クライアントの `base_url` を `http://127.0.0.1:3065/v1` に設定（`api_key` は任意の文字列で構いません。例: `sk-local`）。Node などのランタイムは不要で、ゲートウェイはアプリのプロセス内で動きトレイに常駐します。

**サーバー**（GUI なし。システムサービスとして登録され、起動時に自動開始）:

```bash
# Linux
tar -xzf agent2api-server-linux-amd64.tar.gz && cd agent2api-server-linux-amd64 && sudo bash install.sh
```

```powershell
# Windows Server（管理者 PowerShell）
Expand-Archive agent2api-server-windows-amd64.zip -DestinationPath C:\agent2api-pkg
cd C:\agent2api-pkg; .\install-service.ps1
```

**Docker**:

```bash
docker run -d --name agent2api --restart unless-stopped -p 3065:3065 -v ./data:/data aimodcc/agent2api:latest
```

どの形態でも、導入後にパネル（`http://127.0.0.1:3065/`。サーバーと Docker はホストの IP に読み替え）を開いて管理者アカウントを登録し、「ゲートウェイキー」ページでキーを作成してください。疎通確認: `curl http://127.0.0.1:3065/health`

## 📸 画面プレビュー

### アカウント

![アカウント管理ページ: グローバルキュー、モデル別レート制限クールダウン、有効期限と残高](../assets/screenshots/accounts.png)

アカウントを追加:

![アカウント追加: プロバイダーとバージョンを選び、Web ログインへ](../assets/screenshots/add-account.png)

### モデル管理

![モデル管理ページ: プロバイダーごとのモデル一覧、倍率、コンテキスト、モデルマッピング](../assets/screenshots/models.png)

### レポート

概要: リクエスト数 / 成功率 / トークン量 / トップモデルとアカウント・プロバイダー別ランキング、用量ドーナツと 365 日のアクティビティヒートマップ:

![レポート概要: 統計カード、トップアカウント / プロバイダー、モデル別・プロバイダー別ドーナツ](../assets/screenshots/report-overview.png)

トレンド: 24 時間の**キャッシュヒット率**（折れ線）と**トークン消費**（面）を二軸で重ね、下部に日別トークン棒グラフ:

![レポートトレンド: キャッシュヒット率とトークン消費の二軸トレンド、日別トークン棒グラフ](../assets/screenshots/report-trends.png)

### チェックインセンター

![チェックインセンター: チェックイン概要とプロバイダー別デイリーチェックイン](../assets/screenshots/checkin-center.png)

## 🔧 開発とビルド

### 動作環境

- Rust >= 1.77 と Tauri 2 ツールチェーン（デスクトップアプリ本体のコンパイル用。Windows では WebView2 ランタイムも必要）
- Node.js >= 18.17（`npm run tauri:*` とフロントエンドのビルドスクリプトの実行のみ。デスクトップアプリは実行時に Node へ依存しません）
- 対象プラットフォーム: Windows / macOS / Linux のデスクトップ（各プラットフォームの bundle: Windows NSIS、macOS app+dmg、Linux deb+AppImage）と Docker のサーバー

### よく使うスクリプト

```bash
npm run tauri:install      # デスクトップの依存関係をインストール（npm --prefix desktop-tauri install と同等）
npm run tauri:dev          # 開発モードでデスクトップアプリを起動（ホットリロード付き）
npm run tauri:build        # デスクトップのインストーラーをビルド

npm run build:icon         # アイコンのソース画像を生成（アイコン設計を変えた後に実行し、続けて tauri icon を実行）
```

パッケージ成果物は `target/release/bundle/nsis/Agent2API_<バージョン>_x64-setup.exe`（3.0 時点でおよそ 8.6 MB。`src-tauri/.cargo/config.toml` が cargo の `target-dir` をプロジェクトルートの `target/` に指定しています）。

macOS の `.dmg` は **macOS 上でしかビルドできません**（`npm --prefix desktop-tauri run build -- --target universal-apple-darwin`）。Windows からのクロスコンパイルは不可のため、公式成果物は CI の `build` ワークフロー（`macos-latest` で 2 ターゲットを統合）が生成します。Linux デスクトップのパッケージは同じワークフローの `linux-desktop` job が生成し（deb + AppImage）、Docker イメージとコンテナ内更新に使うサーバーパッケージは `linux-server` job が生成します。後者は Dockerfile のコンパイル経路を再利用するため、パッケージ内の実行ファイルはイメージと同一ツールチェーンの成果物になります。

## 📜 使用に関する告知

### 学習・交流目的に限る

本プロジェクトは、HTTP リバースプロキシ、SSE ストリーミングのパススルー、複数アップストリームのプロトコル適応、デスクトップパッケージング（Tauri）といった技術テーマを学ぶための実践プロジェクトであり、**個人の学習・研究目的のみ**を対象としています。公式製品ではなく、Tencent および WorkBuddy / CodeBuddy、Meituan および CatPaw、SenseTime および小浣熊、Zhipu および AutoClaw / autoglm、Alibaba および Qoder / Accio、Huawei Cloud および CodeArts / OfficeAce、ByteDance および Trae、iFlytek および Loomy、Baidu Wenku および KukuAI、長亭科技 および MonkeyCode、Command Code、Google および Antigravity のいずれとも一切の関係がなく、その承認やスポンサーシップも受けていません。

### リバースプロキシの挙動について

本プロジェクトが実装しているのはローカルのリバースプロキシです。自分のマシン上で自分のアカウントのログイン状態を再利用し、リクエストを公式の上流ゲートウェイへ転送します。有料機能や権限チェックを突破・回避するものではなく、使用するクォータは常に自分のアカウントが元々持つ枠から来ます。ただし明確にしておくべきは、この「非公式クライアントの形態でログイン状態を再利用する」転送方法が、**上流サービスの利用規約や利用条件に適合しない可能性がある**という点です。利用するかどうか、およびそれに伴う一切の結果（レート制限、リスク管理の対象化、アカウントの停止や凍結を含みますがこれらに限りません）は、すべて利用者自身が負担します。

### 禁止事項

本プロジェクトを、いかなる商用目的、利益目的の再配布、大量アカウント運用、上流の課金やクォータ制限の回避、その他現地の法令に違反する活動に使用することを禁じます。関連するモデルサービスを本番環境や商用シーンで呼び出す場合は、公式チャネルと公式 API をご利用ください。

### 認証情報とデータのリスク

本プロジェクトはアカウントの認証情報（`accessToken` / `refreshToken` など）を**平文**で本機の設定ディレクトリ（既定は `~/.agent2api/`）に保存し、エクスポート機能が生成するファイルにも平文の認証情報が含まれます。適切に管理し、公開リポジトリへのコミット、クラウドストレージへのアップロード、第三者への共有は絶対に行わないでください。認証情報の漏えいによる損害は利用者自身が負担します。

### 無保証および権利表示

本プロジェクトは「現状のまま」提供され、作者はその可用性、安定性、安全性、特定目的への適合性について一切保証しません。上流のインターフェースはいつでも変更される可能性があり、本プロジェクトは予告なく動作しなくなり、メンテナンスが終了する場合があります。完全な条件は [LICENSE](../LICENSE) を参照してください。本プロジェクトにおけるインターフェース仕様やプロトコルフィールドなどの情報は、各公式クライアントの公開されたネットワーク通信を観察・整理したものであり、関連する商標とサービスの権利はそれぞれの所有者に帰属します。権利者が必要以上のものであるとお考えの場合は作者までご連絡ください。速やかに調整または削除します。

## 📄 ライセンス

本プロジェクトは [MIT License](../LICENSE) に基づき公開されており、著作権表示を保持する限り、自由に使用・改変・配布できます。

注意すべき点として、LICENSE の本文の後に**使用に関する告知**が付されており、その第 3 条が MIT に対して**制限を追加**しています（商用利用の禁止、利益目的の再配布の禁止、大量アカウント運用の禁止）。したがって本プロジェクトは**純粋な MIT ではありません** —— **MIT の条件と使用に関する告知が合わせて完全なライセンスと使用条件を構成し**、同一の行為について両者が異なる結論を出す場合は、より厳しい側が優先されます。これが `Cargo.toml` で SPDX の `"MIT"` を宣言せず、`license-file` で LICENSE を指している理由でもあります。

## ⭐ Star History

<a href="https://star-history.com/#Agent2API/agent2api&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date" />
  </picture>
</a>

<div align="center">

**[最新版をダウンロード](https://github.com/Agent2API/agent2api/releases/latest)** • **[問題を報告](https://github.com/Agent2API/agent2api/issues)**

このプロジェクトが役に立ったら、⭐️ Star をもらえるとうれしいです！

</div>

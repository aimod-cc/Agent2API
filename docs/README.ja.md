<div align="center">

# Agent2API

**AI ルーティングゲートウェイ + Agent クライアント設定管理で、モデル API とローカル設定を統合**

[English](../README.md) | [简体中文](./README.zh-CN.md) | [繁體中文](./README.zh-Hant.md) | **日本語** | [한국어](./README.ko.md) | [Português (BR)](./README.pt-BR.md)

[![license](https://img.shields.io/badge/license-AGPL--3.0-green)](../LICENSE)
[![release](https://img.shields.io/github/v/release/aimod-cc/Agent2API?include_prereleases)](https://github.com/aimod-cc/Agent2API/releases/latest)
[![docker pulls](https://img.shields.io/docker/pulls/aimodcc/agent2api)](https://hub.docker.com/r/aimodcc/agent2api)
[![stars](https://img.shields.io/github/stars/aimod-cc/Agent2API?style=flat&color=yellow)](https://github.com/aimod-cc/Agent2API/stargazers)
[![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux%20%7C%20Docker-blue)](https://github.com/aimod-cc/Agent2API/releases/latest)

[✨ 主な機能](#-主な機能) • [💬 コミュニティとサポート](#-コミュニティとサポート) • [🚀 クイックスタート](#-クイックスタート) • [📦 インストールとデプロイ](./install.ja.md) • [📸 画面プレビュー](#-画面プレビュー) • [🔧 開発とビルド](#-開発とビルド) • [📜 使用に関する告知](#-使用に関する告知)

</div>

---

Agent2API は **AI ルーティングゲートウェイと Agent クライアント設定管理**のプロジェクトです。カスタム AI プロバイダーを統一 API に接続し、アカウントのルーティング、モデルマッピング、ゲートウェイキー、リクエストレポートを集中管理します。ローカル Agent クライアントの接続プロファイルのインポート、切り替え、書き込みプレビューにも対応します。

## ✨ 主な機能

- 🔌 **統一モデル API** — OpenAI Chat / Responses / Images、Anthropic Messages と統一モデル一覧。Raccoon、Loomy、Antigravity、ModelScope、WorkBuddy、豆包工作 が画像生成を提供し、Raccoon、Loomy、Antigravity、ModelScope、WorkBuddy は参照画像の編集にも対応。生成と編集は AI アシスタントのツールからも利用可能
- 🧭 **AI ルーティング** — モデルマッピング、アカウント優先度、負荷分散、セッション固定、フェイルオーバー
- 🤖 **Agent クライアント設定管理** — 複数の接続設定、既存設定のインポート、書き込みプレビュー、サービス切り替え
- 🧩 **プラグイン拡張** — 追加プロバイダーは本体に同梱せず個別にインストール・更新。カスタムプロバイダーにはプラグイン不要
- 🛡️ **外向きプロキシ** — アカウントと更新タスクの出口をプロキシプールから選択
- 🔑 **ゲートウェイのセキュリティ** — 権限別キー、パネルログインの人間確認、キー未設定時のモデルリクエスト拒否
- 📊 **リクエストレポート** — リクエスト数、成功率、トークン使用量、キャッシュヒット率、365 日ヒートマップ
- ⏰ **バックグラウンド管理** — モデル一覧、認証情報、ソフトウェア更新を管理。毎日のチェックインは各プロバイダー自身の受付開始時刻に実行（Qoder 10:01、ZCode 19:01、その他はスケジュール時刻に従う）し、未チェックインのアカウントだけを対象にした追いかけ実行も行います。追加プロバイダーの固有タスクはプラグインが提供
- 🖥️ **3 つの実行形態** — Windows / macOS / Linux デスクトップ、Windows / Linux ヘッドレスサーバー、Docker（amd64 / arm64）。Node ランタイム不要、6 言語 UI

## ❤️ スポンサー

<a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><img src="../assets/sponsor-proxylane.png" alt="ProxyLane（スポンサー）" width="100%" /></a>

<table>

<tr>
<td width="240"><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><picture><source media="(prefers-color-scheme: dark)" srcset="../assets/proxylane-logo-for-dark-bg.png" /><img src="../assets/proxylane-logo-for-light-bg.png" alt="ProxyLane" width="220" /></picture></a></td>
<td><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme">ProxyLane</a> が本プロジェクトをスポンサーしています！ProxyLane は AI アカウント専用のネイティブ静的住宅用 IP（Static Residential ISP）を提供しています。実在の家庭向けブロードバンド事業者の IP を使い、Scamalytics の不正スコアが低く、IP は 1 つずつ専有、通信量は無制限、$3.90 から。「ネットワークプロキシ」ページでアカウントごとに出口を 1 つ割り当てるだけです。初回購入はコード <code>AGENT2API25</code> で 25% オフ。</td>
</tr>

</table>

## 💬 コミュニティとサポート

- **QQ グループ**: Agent2API の交流グループ —— **グループ 1（`1127656296`）とグループ 2（`1130894674`）はまもなく満員のため、グループ 3（`1108276647`）にご参加ください**（QQ で番号を検索して参加できます）。
- **Telegram グループ**: <https://t.me/+9gM8YPvgRL02NGRl>
- **💖 支援**: Agent2API がお役に立ったら、[作者にコーヒーをおごる](./SPONSOR.md)のも歓迎です。支援は任意で、機能の解放などはありません。
- **🤝 スポンサー / ビジネス**: スポンサーになりたい、またはここに製品を掲載したい場合は [541675139@qq.com](mailto:541675139@qq.com) までメールでご連絡ください。

## 🚀 クイックスタート

3 つの形態からお使いの環境に合うものを選んでください。**インストール・アップグレード・アンインストール・トラブルシューティングの詳細は [インストールとデプロイ](./install.ja.md) にあります**。

| 形態 | 成果物 | インターフェース |
| --- | --- | --- |
| **デスクトップ**（Windows / macOS / Linux） | `Agent2API_<バージョン>_x64-setup.exe`、`_universal.dmg`、`_amd64.deb`、`_amd64.AppImage` | アプリウィンドウ |
| **サーバー**（Windows Server / Linux、GUI なし） | `agent2api-server-windows-amd64.zip`、`agent2api-server-linux-<arch>.tar.gz` | ブラウザーパネル |
| **Docker** | `aimodcc/agent2api:latest`（amd64 / arm64） | ブラウザーパネル |

**デスクトップ**: カスタムプロバイダーとアカウントを追加し、ゲートウェイキーを作成。「Agent」ページで接続設定を適用するか、クライアントに `http://127.0.0.1:3065/v1` とそのキーを設定してください。追加の機能は対応プラグインのインストールが必要です。

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

**モバイル**: ブラウザーで `http://<ゲートウェイIP>:3065/m` を開けば使えます（「ホーム画面に追加」で全画面 PWA に）。独立したアイコンが欲しい場合は、Release の `Agent2API-<バージョン>.apk`（Android）または `Agent2API-<バージョン>.ipa`（iOS、未署名・自己再署名が必要）をインストールしてください。詳細: [インストールとデプロイ → モバイル](install.ja.md#モバイル)。

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

### 認証情報とデータの安全性

認証情報はローカルデータベースで暗号化されますが、エクスポート、バックアップ、鍵ファイルには機密データが含まれる可能性があります。公開リポジトリやクラウドにアップロードしたり、他者と共有したりしないでください。

### 無保証

本プロジェクトは「現状のまま」提供され、可用性、安定性、安全性、特定目的への適合性を保証しません。商標とサービスの権利は各所有者に帰属します。完全な条件は [LICENSE](../LICENSE) を参照してください。

## 📄 ライセンス

プラグインと同じ [GNU Affero General Public License v3.0（AGPL-3.0）](../LICENSE) を採用しています。商用利用を含む使用・改変・配布が可能です。著作権とライセンス表示を保持し、該当するソース提供義務を履行してください。改変版をネットワーク経由で提供する場合、その利用者に当該版の対応するソースを提供する必要があります。

## ⭐ Star History

<a href="https://star-history.com/#aimod-cc/Agent2API&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date" />
  </picture>
</a>

<div align="center">

**[最新版をダウンロード](https://github.com/aimod-cc/Agent2API/releases/latest)** • **[問題を報告](https://github.com/aimod-cc/Agent2API/issues)**

このプロジェクトが役に立ったら、⭐️ Star をもらえるとうれしいです！

</div>

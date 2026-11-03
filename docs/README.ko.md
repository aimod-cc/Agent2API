<div align="center">

# Agent2API

**AI 라우팅 게이트웨이 + Agent 클라이언트 설정 관리로 모델 API와 로컬 설정을 통합**

[English](../README.md) | [简体中文](./README.zh-CN.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | **한국어** | [Português (BR)](./README.pt-BR.md)

[![license](https://img.shields.io/badge/license-AGPL--3.0-green)](../LICENSE)
[![release](https://img.shields.io/github/v/release/aimod-cc/Agent2API?include_prereleases)](https://github.com/aimod-cc/Agent2API/releases/latest)
[![docker pulls](https://img.shields.io/docker/pulls/aimodcc/agent2api)](https://hub.docker.com/r/aimodcc/agent2api)
[![stars](https://img.shields.io/github/stars/aimod-cc/Agent2API?style=flat&color=yellow)](https://github.com/aimod-cc/Agent2API/stargazers)
[![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux%20%7C%20Docker-blue)](https://github.com/aimod-cc/Agent2API/releases/latest)

[✨ 핵심 기능](#-핵심-기능) • [💬 커뮤니티 및 지원](#-커뮤니티-및-지원) • [🚀 빠른 시작](#-빠른-시작) • [📦 설치와 배포](./install.ko.md) • [📸 화면 미리보기](#-화면-미리보기) • [🔧 개발 및 빌드](#-개발-및-빌드) • [📜 사용 고지](#-사용-고지)

</div>

---

Agent2API는 **AI 라우팅 게이트웨이와 Agent 클라이언트 설정 관리** 프로젝트입니다. 사용자 지정 AI 공급자를 통합 모델 API에 연결하고 계정 라우팅, 모델 매핑, 게이트웨이 키와 요청 리포트를 한곳에서 관리합니다. 로컬 Agent 클라이언트의 연결 프로필 가져오기, 전환, 쓰기 미리보기도 지원합니다.

## ✨ 핵심 기능

- 🔌 **통합 모델 API** — OpenAI Chat / Responses / Images, Anthropic Messages 및 통합 모델 목록. Raccoon, Loomy, Antigravity, ModelScope, WorkBuddy, Doubao Work가 이미지 생성을 제공하며 Raccoon, Loomy, Antigravity, ModelScope, WorkBuddy는 참조 이미지 편집도 지원. 생성과 편집은 AI 어시스턴트 도구로도 사용 가능
- 🧭 **AI 라우팅** — 모델 매핑, 계정 우선순위, 부하 분산, 세션 고정 및 장애 전환
- 🤖 **Agent 클라이언트 설정 관리** — 여러 연결 프로필, 기존 설정 가져오기, 쓰기 미리보기 및 서비스 전환
- 🧩 **플러그인 확장** — 추가 공급자 구현은 본체에 포함되지 않고 별도 설치·업데이트. 사용자 지정 공급자에는 플러그인 불필요
- 🛡️ **아웃바운드 프록시** — 계정과 업데이트 작업에 프록시 풀의 출구 선택
- 🔑 **게이트웨이 보안** — 권한별 키, 패널 로그인 사람 확인, 키 미설정 시 모델 요청 기본 거부
- 📊 **요청 리포트** — 요청 수, 성공률, 토큰 사용량, 캐시 적중률, 365일 히트맵
- ⏰ **백그라운드 관리** — 모델 목록, 자격 증명 및 소프트웨어 업데이트 확인. 매일 체크인은 각 공급자의 자체 접수 시작 시각에 실행되고(Qoder 10:01, ZCode 19:01, 나머지는 일정 시각), 아직 체크인하지 않은 계정만 대상으로 하는 보충 실행도 있습니다. 추가 공급자별 작업은 플러그인이 제공
- 🖥️ **세 가지 실행 방식** — Windows / macOS / Linux 데스크톱, Windows / Linux 헤드리스 서버, Docker(amd64 / arm64). Node 런타임 불필요, UI 6개 언어

## ❤️ 스폰서

<a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><img src="../assets/sponsor-proxylane.png" alt="ProxyLane(스폰서)" width="100%" /></a>

<table>

<tr>
<td width="240"><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><picture><source media="(prefers-color-scheme: dark)" srcset="../assets/proxylane-logo-for-dark-bg.png" /><img src="../assets/proxylane-logo-for-light-bg.png" alt="ProxyLane" width="220" /></picture></a></td>
<td><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme">ProxyLane</a>가 이 프로젝트를 후원합니다! ProxyLane은 AI 계정 전용 네이티브 정적 주거용 IP(Static Residential ISP)를 제공합니다. 실제 가정용 브로드밴드 사업자의 IP를 사용하며 Scamalytics 사기 점수가 낮고, IP는 1개씩 전용, 트래픽 무제한, $3.90부터. "네트워크 프록시" 페이지에서 계정마다 출구 하나를 지정하면 됩니다. 첫 구매 시 코드 <code>AGENT2API25</code>로 25% 할인됩니다.</td>
</tr>

</table>

## 💬 커뮤니티 및 지원

- **QQ 그룹**: Agent2API 교류 그룹 —— **그룹 1(`1127656296`)과 그룹 2(`1130894674`)은 곧 정원이 마감되므로 그룹 3(`1108276647`)에 참여해 주세요** (QQ에서 번호를 검색해 참여하세요).
- **Telegram 그룹**: <https://t.me/+9gM8YPvgRL02NGRl>
- **💖 후원**: Agent2API가 도움이 되셨다면 [작성자에게 커피 한 잔](./SPONSOR.md)도 환영합니다. 후원은 자발적이며 아무 기능도 해제되지 않습니다.
- **🤝 스폰서 / 비즈니스**: 프로젝트를 후원하거나 여기에 제품을 소개하고 싶다면 [541675139@qq.com](mailto:541675139@qq.com)으로 메일 주세요.

## 🚀 빠른 시작

세 가지 형태 중 환경에 맞는 것을 고르세요. **설치, 업그레이드, 제거, 문제 해결의 전체 안내는 [설치와 배포](./install.ko.md) 에 있습니다**.

| 형태 | 산출물 | 인터페이스 |
| --- | --- | --- |
| **데스크톱**(Windows / macOS / Linux) | `Agent2API_<버전>_x64-setup.exe`, `_universal.dmg`, `_amd64.deb`, `_amd64.AppImage` | 앱 창 |
| **서버**(Windows Server / Linux, GUI 없음) | `agent2api-server-windows-amd64.zip`, `agent2api-server-linux-<arch>.tar.gz` | 브라우저 패널 |
| **Docker** | `aimodcc/agent2api:latest`(amd64 / arm64) | 브라우저 패널 |

**데스크톱**: 사용자 지정 공급자와 계정을 추가하고 게이트웨이 키를 만드세요. "Agent" 페이지에서 연결 설정을 적용하거나 클라이언트에 `http://127.0.0.1:3065/v1`과 해당 키를 입력하세요. 추가 기능은 해당 플러그인을 먼저 설치해야 합니다.

**서버**(GUI 없음. 시스템 서비스로 등록되어 부팅 시 자동 시작):

```bash
# Linux
tar -xzf agent2api-server-linux-amd64.tar.gz && cd agent2api-server-linux-amd64 && sudo bash install.sh
```

```powershell
# Windows Server (관리자 PowerShell)
Expand-Archive agent2api-server-windows-amd64.zip -DestinationPath C:\agent2api-pkg
cd C:\agent2api-pkg; .\install-service.ps1
```

**Docker**:

```bash
docker run -d --name agent2api --restart unless-stopped -p 3065:3065 -v ./data:/data aimodcc/agent2api:latest
```

어느 형태든 설치 후 패널(`http://127.0.0.1:3065/`. 서버와 Docker 는 호스트 IP 로 바꿔서)을 열어 관리자 계정을 등록하고, "게이트웨이 키" 페이지에서 키를 만드세요. 연결 확인: `curl http://127.0.0.1:3065/health`

**모바일**: 브라우저에서 `http://<게이트웨이 IP>:3065/m`를 열면 됩니다("홈 화면에 추가"로 전체 화면 PWA 가능). 독립 아이콘이 필요하면 Release의 `Agent2API-<버전>.apk`(Android) 또는 `Agent2API-<버전>.ipa`(iOS, 미서명 — 직접 재서명 필요)를 설치하세요. 자세한 내용: [설치 및 배포 → 모바일](install.ko.md#모바일).

## 📸 화면 미리보기

### 계정

![계정 관리 페이지: 전역 큐, 모델별 한도 쿨다운, 유효기간과 잔액](../assets/screenshots/accounts.png)

계정 추가:

![계정 추가: 프로바이더와 버전을 고른 뒤 웹 로그인](../assets/screenshots/add-account.png)

### 모델 관리

![모델 관리 페이지: 공급자별 모델 목록, 배율, 컨텍스트 및 모델 매핑](../assets/screenshots/models.png)

### 리포트

개요: 요청 수 / 성공률 / 토큰 / 상위 모델과 계정·프로바이더 순위, 사용량 도넛과 365일 활동 히트맵:

![리포트 개요: 통계 카드, 상위 계정 / 프로바이더, 모델별·프로바이더별 도넛](../assets/screenshots/report-overview.png)

추세: 24시간 **캐시 적중률**(선)과 **토큰 소비**(면)를 이중 축으로 겹쳐 표시, 아래에 일별 토큰 막대 차트:

![리포트 추세: 캐시 적중률과 토큰 소비 이중 축 추세, 일별 토큰 막대 차트](../assets/screenshots/report-trends.png)

## 🔧 개발 및 빌드

### 요구 사항

- Rust >= 1.77 와 Tauri 2 도구 체인(데스크톱 앱 본체 컴파일용. Windows 에서는 WebView2 런타임도 필요)
- Node.js >= 18.17(`npm run tauri:*` 과 프런트엔드 빌드 스크립트 실행용. 데스크톱 앱은 실행 시 Node 에 의존하지 않습니다)
- 대상 플랫폼: Windows / macOS / Linux 데스크톱(플랫폼별 bundle: Windows NSIS, macOS app+dmg, Linux deb+AppImage)과 Docker 서버

### 자주 쓰는 스크립트

```bash
npm run tauri:install      # 데스크톱 의존성 설치(npm --prefix desktop-tauri install 과 동일)
npm run tauri:dev          # 개발 모드로 데스크톱 앱 실행(핫 리로드 포함)
npm run tauri:build        # 데스크톱 설치 프로그램 빌드

npm run build:icon         # 아이콘 소스 이미지 생성(아이콘 디자인을 바꾼 뒤 실행하고, 이어서 tauri icon 실행)
```

패키징 산출물은 `target/release/bundle/nsis/Agent2API_<버전>_x64-setup.exe`(3.0 기준 약 8.6 MB. `src-tauri/.cargo/config.toml` 이 cargo 의 `target-dir` 을 프로젝트 루트의 `target/` 으로 지정합니다).

macOS 의 `.dmg` 는 **macOS 에서만 빌드할 수 있습니다**(`npm --prefix desktop-tauri run build -- --target universal-apple-darwin`). Windows 에서 크로스 컴파일할 수 없어 공식 산출물은 CI 의 `build` 워크플로(macos-latest 에서 두 타깃을 병합)가 만듭니다. Linux 데스크톱 패키지는 같은 워크플로의 `linux-desktop` job 이 만들고(deb + AppImage), Docker 이미지와 컨테이너 내 업데이트에 쓰는 서버 패키지는 `linux-server` job 이 만듭니다. 후자는 Dockerfile 의 컴파일 경로를 재사용하므로 패키지 안의 실행 파일이 이미지와 동일한 도구 체인의 산출물입니다.

## 📜 사용 고지

### 자격 증명과 데이터 보안

자격 증명은 로컬 데이터베이스에 암호화하여 저장하지만 내보내기, 백업 및 키 파일에는 민감한 데이터가 포함될 수 있습니다. 공개 저장소나 클라우드에 업로드하거나 타인과 공유하지 마세요.

### 무보증

프로젝트는 "있는 그대로" 제공하며 가용성, 안정성, 보안성이나 특정 목적 적합성을 보증하지 않습니다. 상표와 서비스의 권리는 각 소유자에게 있습니다. 전체 조건은 [LICENSE](../LICENSE)를 참고하세요.

## 📄 라이선스

플러그인 저장소와 동일한 [GNU Affero General Public License v3.0(AGPL-3.0)](../LICENSE)을 사용합니다. 상업적 이용을 포함한 사용·수정·배포가 가능하며 저작권 및 라이선스 고지와 해당 소스 제공 의무를 지켜야 합니다. 수정 버전을 네트워크 서비스로 제공할 때 해당 사용자에게 그 버전의 대응 소스를 제공해야 합니다.

## ⭐ Star History

<a href="https://star-history.com/#aimod-cc/Agent2API&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date" />
  </picture>
</a>

<div align="center">

**[최신 버전 내려받기](https://github.com/aimod-cc/Agent2API/releases/latest)** • **[문제 보고](https://github.com/aimod-cc/Agent2API/issues)**

이 프로젝트가 도움이 되었다면 ⭐️ Star 를 눌러 주세요!

</div>

<div align="center">

# Agent2API

**여러 AI 데스크톱 클라이언트의 로그인 상태를 하나의 로컬 OpenAI 호환 API 게이트웨이로**

[简体中文](../README.md) | [English](./README.en.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | **한국어** | [Português (BR)](./README.pt-BR.md)

[![license](https://img.shields.io/badge/license-MIT%20%2B%20고지-green)](../LICENSE)
[![release](https://img.shields.io/github/v/release/Agent2API/agent2api?include_prereleases)](https://github.com/Agent2API/agent2api/releases/latest)
[![docker pulls](https://img.shields.io/docker/pulls/aimodcc/agent2api)](https://hub.docker.com/r/aimodcc/agent2api)
[![stars](https://img.shields.io/github/stars/Agent2API/agent2api?style=flat&color=yellow)](https://github.com/Agent2API/agent2api/stargazers)
[![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux%20%7C%20Docker-blue)](https://github.com/Agent2API/agent2api/releases/latest)

[✨ 핵심 기능](#-핵심-기능) • [💬 커뮤니티 및 지원](#-커뮤니티-및-지원) • [🤝 지원 공급자](#-지원-공급자) • [🚀 빠른 시작](#-빠른-시작) • [📦 설치와 배포](./install.ko.md) • [📸 화면 미리보기](#-화면-미리보기) • [🔧 개발 및 빌드](#-개발-및-빌드) • [📜 사용 고지](#-사용-고지)

</div>

---

Agent2API는 여러 AI 데스크톱 클라이언트에 로그인된 계정을 하나로 모아 OpenAI 호환 엔드포인트로 쓸 수 있게 합니다. 사용자 지정 `base_url`을 지원하는 클라이언트에 `http://127.0.0.1:3065/v1`을 넣으면 이 계정들의 모델 한도를 그대로 호출할 수 있습니다 — **업스트림 API 키도, 클라이언트 수정도 필요 없습니다**.

## ✨ 핵심 기능

- 🔌 **OpenAI 호환** —— 클라이언트에 `base_url`만 넣으면 바로 사용. Chat / Responses / Messages 세 가지 프로토콜 모두 지원
- 🏪 **내장 공급자 17곳** —— WorkBuddy, 小浣熊, CatPaw, Qoder, Trae 등 바로 사용 가능하며 사용자 지정 공급자도 지원
- 🔁 **자격 증명 자동 갱신** —— 토큰은 만료 전 자동 교체, 만료 임박분은 일괄 갱신, 모델 카탈로그도 함께 최신화
- 🪂 **다중 계정 로테이션** —— 우선순위대로 큐잉, 429는 다음 계정으로, 한도 쿨다운 중엔 기다리지 않고 건너뜀
- 🛡️ **아웃바운드 프록시** —— 계정마다 전용 출구 IP 지정, 아웃바운드 핑거프린트는 공식 클라이언트로 위장
- 🔑 **게이트웨이 보안** —— 권한별 다중 게이트웨이 키, 로그인 사람 확인, 키 미설정 시 기본 거부
- 📊 **요청 리포트** —— 요청량·성공률·토큰 소비를 한눈에, 캐시 적중률과 365일 히트맵 포함
- ⏰ **예약 작업** —— 자동 출석, 자격 증명 유지보수, 모델 카탈로그 갱신을 한 페이지에서 관리
- 💰 **잔액과 사용량** —— 계정 행에서 바로 잔액과 사용량 표시, 계정 간 동시 조회 지원
- 🖥️ **네 가지 실행 방식** —— Windows / macOS / Linux 데스크톱 앱(트레이 상주, Node 불필요)과 패널·게이트웨이가 같은 포트를 쓰는 Docker 이미지(amd64 / arm64). UI 6개 언어

## ❤️ 스폰서

<a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><img src="../assets/sponsor-proxylane.png" alt="ProxyLane(스폰서)" width="100%" /></a>

<table>

<tr>
<td width="240"><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><picture><source media="(prefers-color-scheme: dark)" srcset="../assets/proxylane-logo-for-dark-bg.png" /><img src="../assets/proxylane-logo-for-light-bg.png" alt="ProxyLane" width="220" /></picture></a></td>
<td><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme">ProxyLane</a>가 이 프로젝트를 후원합니다! ProxyLane은 AI 계정 전용 네이티브 정적 주거용 IP(Static Residential ISP)를 제공합니다. 실제 가정용 브로드밴드 사업자의 IP를 사용하며 Scamalytics 사기 점수가 낮고, IP는 1개씩 전용, 트래픽 무제한, $3.90부터. "네트워크 프록시" 페이지에서 계정마다 출구 하나를 지정하면 됩니다. 첫 구매 시 코드 <code>AGENT2API25</code>로 25% 할인됩니다.</td>
</tr>

</table>

## 💬 커뮤니티 및 지원

- **QQ 그룹**: Agent2API 교류 그룹 —— **그룹 1(`1127656296`)은 곧 정원이 차므로 그룹 2(`1130894674`)에 먼저 참여해 주세요** (QQ에서 번호를 검색해 참여하세요).
- **Telegram 그룹**: <https://t.me/+9gM8YPvgRL02NGRl>
- **Discord 서버**: <https://discord.gg/z9GzSJMWp>
- **💖 후원**: Agent2API가 도움이 되셨다면 [작성자에게 커피 한 잔](./SPONSOR.md)도 환영합니다. 후원은 자발적이며 아무 기능도 해제되지 않습니다.
- **🤝 스폰서 / 비즈니스**: 프로젝트를 후원하거나 여기에 제품을 소개하고 싶다면 [541675139@qq.com](mailto:541675139@qq.com)으로 메일 주세요.

## 🤝 지원 공급자

플랫폼별 리버스 프록시 지원 현황(✓ 지원 · ✗ 미지원 · — 해당 없음):

| 플랫폼 | LLM 요청 | 토큰 자동 갱신 | 모델 목록 | 잔액 조회 | 출석 | 수령 항목 |
| --- | :--: | :--: | :--: | :--: | :--: | :--: |
| WorkBuddy 중국판 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| WorkBuddy 국제판 | ✓ | ✓ | ✓ | ✓ | ✗ | — |
| 小浣熊 | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| CatPaw | ✓ | ✗ | ✓ | ✓ | ✗ | — |
| AutoClaw(중국판 / 국제판) | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Qoder | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Cline(Free / Pass) | ✓ | ✓ | ✓ | ✓ | — | — |
| Accio(국제판 / 중국판) | ✓ | ✓ | ✓ | ✓ | — | — |
| ZCode(중국판 / 국제판) | ✓ | ✗ | ✗ | ✓ | — | ✓ |
| CodeArts | ✓ | ✓ | ✓ | ✓ | — | ✓ |
| Trae | ✓ | ✓ | ✓ | ✓ | — | — |
| Loomy(iFlytek) | ✓ | ✗ | ✓ | ✓ | ✓ | — |
| KukuAI(Baidu Wenku) | ✓ | ✗ | ✓ | ✓ | ✓ | — |
| MonkeyCode(长亭科技, 중국판 / 국제판) | ✓ | ✗ | ✓ | — | — | — |
| Command Code | ✓ | ✗ | ✓ | — | — | — |
| Antigravity(Google, Gemini / Claude / GPT-OSS) | ✓ | ✓ | ✓ | — | — | — |
| OfficeAce(Huawei Cloud, OfficeClaw) | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| OrcaTerm(Tencent Cloud) | ✓ | ✓ | ✓ | ✓ | — | — |
| 智谱清言(Zhipu Qingyan) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 사용자 지정 프로바이더 | ✓ | — | ✓ | — | — | — |

> 참고: 모델 목록의 ✓는 온라인 자동 가져오기를 지원한다는 뜻입니다(일부 플랫폼은 내장 정적 목록도 함께 사용). ZCode는 내장 정적 목록만 제공합니다. 토큰 자동 갱신의 ✗는 해당 플랫폼에 사용 가능한 갱신 수단이 없다는 뜻입니다(계정은 자격 증명이 유효한 동안 정상적으로 사용 가능). Qoder 출석은 중국판에서만 지원됩니다.
>
> **智谱清言**은 이 프로젝트에서 **유일하게 "게스트 모드"를 지원**하는 제공자입니다. 로그인 없이 클릭 한 번으로 바로 대화에 쓸 수 있는 자격 증명을 받습니다(무료 할당량은 업스트림이 기기 단위로 발급합니다). 대신 게스트 계정에는 포인트 체계가 없습니다 —— 위 표의 "잔액 조회 / 출석 / 수령"과 **작업 센터의 초보자 과제·일일 과제**는 **로그인된 계정**에만 적용되며(토큰 붙여넣기 또는 클라이언트 로그인 상태 가져오기), 게스트는 건너뜁니다.

`/v1/chat/completions`, `/v1/responses`, `/v1/messages`(`count_tokens` 포함)와 `/v1/models`는 모든 플랫폼에 동일하게 적용됩니다. 모델 매핑, 전역 우선순위 큐, 429 폴백, 이그레스 프록시, 아웃바운드 핑거프린트 마스킹, 요청 리포트 역시 전 플랫폼 공통입니다.

> **이 프로젝트는 학습과 교류 목적으로만 제공됩니다.** 비공식 클라이언트 형태로 자신의 계정 로그인 상태를 재사용하므로 업스트림 서비스 약관에 부합하지 않을 수 있습니다. 위험(계정 정지 등 포함)은 사용자가 부담하며, 상업적 사용과 과금 우회는 금지됩니다. 자세한 내용은 [사용 고지](#-사용-고지)와 [LICENSE](../LICENSE)를 참고하세요.
>
> 이 프로젝트는 개인용 로컬 프록시 도구로, 각 업스트림 벤더 및 공식 제품과 아무런 관련이 없습니다(목록은 [사용 고지](#-사용-고지) 참고). 인터페이스 형태는 각 사 클라이언트 통신을 관찰한 것이며 업스트림은 언제든 바뀔 수 있습니다.

## 🚀 빠른 시작

세 가지 형태 중 환경에 맞는 것을 고르세요. **설치, 업그레이드, 제거, 문제 해결의 전체 안내는 [설치와 배포](./install.ko.md) 에 있습니다**.

| 형태 | 산출물 | 인터페이스 |
| --- | --- | --- |
| **데스크톱**(Windows / macOS / Linux) | `Agent2API_<버전>_x64-setup.exe`, `_universal.dmg`, `_amd64.deb`, `_amd64.AppImage` | 앱 창 |
| **서버**(Windows Server / Linux, GUI 없음) | `agent2api-server-windows-amd64.zip`, `agent2api-server-linux-<arch>.tar.gz` | 브라우저 패널 |
| **Docker** | `aimodcc/agent2api:latest`(amd64 / arm64) | 브라우저 패널 |

**데스크톱**: 설치 후 실행 → "계정" 페이지에서 계정 추가 → 클라이언트 `base_url` 을 `http://127.0.0.1:3065/v1` 로 설정(`api_key` 는 아무 값이나 가능합니다. 예: `sk-local`). Node 나 다른 런타임은 필요 없고, 게이트웨이는 앱 프로세스 안에서 돌며 트레이에 상주합니다.

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

### 출석 체크 센터

![출석 체크 센터: 출석 요약과 공급자별 일일 출석](../assets/screenshots/checkin-center.png)

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

### 학습과 교류 목적에 한정

이 프로젝트는 HTTP 리버스 프록시, SSE 스트리밍 패스스루, 다중 업스트림 프로토콜 적응, 데스크톱 패키징(Tauri) 같은 기술 주제를 익히기 위한 실습 프로젝트이며, **개인 학습과 연구 목적으로만** 제공됩니다. 공식 제품이 아니며 Tencent 및 WorkBuddy / CodeBuddy, Meituan 및 CatPaw, SenseTime 및 小浣熊, Zhipu 및 AutoClaw / autoglm, Alibaba 및 Qoder / Accio, Huawei Cloud 및 CodeArts / OfficeAce, ByteDance 및 Trae, iFlytek 및 Loomy, Baidu Wenku 및 KukuAI, 长亭科技 및 MonkeyCode, Command Code, Google 및 Antigravity 와 아무런 관련이 없고, 그 승인이나 후원도 받지 않았습니다.

### 리버스 프록시 동작에 대하여

이 프로젝트가 구현하는 것은 로컬 리버스 프록시입니다. 자신의 컴퓨터에서 자신의 계정 로그인 상태를 재사용해 요청을 공식 업스트림 게이트웨이로 전달합니다. 유료 기능이나 권한 검사를 뚫거나 우회하지 않으며, 사용하는 할당량은 항상 자신의 계정이 원래 가진 범위에서 나옵니다. 다만 분명히 해 둘 점은, 이렇게 "비공식 클라이언트 형태로 로그인 상태를 재사용하는" 전달 방식이 **업스트림 서비스의 이용약관이나 이용 조건에 부합하지 않을 수 있다**는 것입니다. 사용 여부와 그로 인한 모든 결과(속도 제한, 리스크 관리 대상화, 계정 정지나 차단을 포함하되 이에 한정되지 않음)는 전적으로 사용자가 부담합니다.

### 금지 용도

이 프로젝트를 상업적 목적, 이익을 위한 재배포, 대량 계정 운영, 업스트림 과금이나 할당량 제한 우회, 기타 현지 법규를 위반하는 활동에 사용하는 것을 금지합니다. 관련 모델 서비스를 프로덕션 환경이나 상업적 상황에서 호출하려면 공식 채널과 공식 API를 사용하세요.

### 자격 증명과 데이터 위험

이 프로젝트는 계정 자격 증명(`accessToken` / `refreshToken` 등)을 **평문**으로 이 PC의 설정 디렉터리(기본 `~/.agent2api/`)에 저장하며, 내보내기 기능이 만든 파일에도 평문 자격 증명이 들어갑니다. 잘 보관하고, 공개 저장소에 커밋하거나 클라우드 스토리지에 업로드하거나 타인과 공유하지 마세요. 자격 증명 유출로 인한 손실은 사용자가 부담합니다.

### 무보증 및 권리 고지

이 프로젝트는 "있는 그대로" 제공되며, 작성자는 가용성, 안정성, 보안성, 특정 목적 적합성에 대해 어떤 보증도 하지 않습니다. 업스트림 인터페이스는 언제든 바뀔 수 있고 이 프로젝트는 예고 없이 동작하지 않게 되어 유지 보수가 중단될 수 있습니다. 전체 조건은 [LICENSE](../LICENSE)를 참고하세요. 이 프로젝트의 인터페이스 형태와 프로토콜 필드 등의 정보는 각 공식 클라이언트의 공개된 네트워크 통신을 관찰·정리한 것이며, 관련 상표와 서비스의 권리는 각 소유자에게 있습니다. 권리자가 이 프로젝트가 부적절하다고 판단하면 작성자에게 연락해 주세요. 즉시 조정하거나 삭제하겠습니다.

## 📄 라이선스

이 프로젝트는 [MIT License](../LICENSE)로 배포되며, 저작권 표시를 유지하는 한 자유롭게 사용·수정·배포할 수 있습니다.

유의할 점은 LICENSE 본문 뒤에 **사용 고지**가 붙어 있고, 그 제3조가 MIT 위에 **제한을 추가**한다는 것입니다(상업적 사용 금지, 이익을 위한 재배포 금지, 대량 계정 운영 금지). 따라서 이 프로젝트는 **순수한 MIT가 아닙니다** — **MIT 조건과 사용 고지가 함께 완전한 라이선스와 사용 조건을 이루며**, 같은 행위에 대해 둘이 다른 결론을 내면 더 엄격한 쪽이 우선합니다. 이 때문에 `Cargo.toml` 이 SPDX `"MIT"` 를 선언하지 않고 `license-file` 로 LICENSE 를 가리킵니다.

## ⭐ Star History

<a href="https://star-history.com/#Agent2API/agent2api&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date" />
  </picture>
</a>

<div align="center">

**[최신 버전 내려받기](https://github.com/Agent2API/agent2api/releases/latest)** • **[문제 보고](https://github.com/Agent2API/agent2api/issues)**

이 프로젝트가 도움이 되었다면 ⭐️ Star 를 눌러 주세요!

</div>

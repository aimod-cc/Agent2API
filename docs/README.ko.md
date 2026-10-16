<div align="center">

# Agent2API

**여러 AI 데스크톱 클라이언트의 로그인 상태를 하나의 로컬 OpenAI 호환 API 게이트웨이로**

[简体中文](../README.md) | [English](./README.en.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | **한국어** | [Português (BR)](./README.pt-BR.md)

[![license](https://img.shields.io/badge/license-MIT%20%2B%20고지-green)](../LICENSE)
[![release](https://img.shields.io/github/v/release/aimod-cc/agent2api?include_prereleases)](https://github.com/aimod-cc/agent2api/releases/latest)
[![docker pulls](https://img.shields.io/docker/pulls/aimodcc/agent2api)](https://hub.docker.com/r/aimodcc/agent2api)
[![stars](https://img.shields.io/github/stars/aimod-cc/agent2api?style=flat&color=yellow)](https://github.com/aimod-cc/agent2api/stargazers)
[![platform](https://img.shields.io/badge/platform-Windows%20%7C%20Docker-blue)](https://github.com/aimod-cc/agent2api/releases/latest)

[✨ 핵심 기능](#-핵심-기능) • [💬 커뮤니티 및 지원](#-커뮤니티-및-지원) • [🤝 지원 공급자](#-지원-공급자) • [🚀 빠른 시작](#-빠른-시작) • [🐳 Docker 배포](#-docker-배포) • [📸 화면 미리보기](#-화면-미리보기) • [🔧 개발 및 빌드](#-개발-및-빌드) • [📜 사용 고지](#-사용-고지)

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
- 🖥️ **두 가지 실행 방식** —— 트레이 상주 데스크톱 앱(Node 불필요) 또는 패널·게이트웨이가 같은 포트를 쓰는 Docker. UI 6개 언어

## ❤️ 스폰서

<a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><img src="../assets/sponsor-proxylane.png" alt="ProxyLane(스폰서)" width="100%" /></a>

<table>

<tr>
<td width="240"><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><picture><source media="(prefers-color-scheme: dark)" srcset="../assets/proxylane-logo-for-dark-bg.png" /><img src="../assets/proxylane-logo-for-light-bg.png" alt="ProxyLane" width="220" /></picture></a></td>
<td><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme">ProxyLane</a>가 이 프로젝트를 후원합니다! ProxyLane은 AI 계정 전용 네이티브 정적 주거용 IP(Static Residential ISP)를 제공합니다. 실제 가정용 브로드밴드 사업자의 IP를 사용하며 Scamalytics 사기 점수가 낮고, IP는 1개씩 전용, 트래픽 무제한, $3.90부터. "네트워크 프록시" 페이지에서 계정마다 출구 하나를 지정하면 됩니다. 첫 구매 시 코드 <code>AGENT2API25</code>로 25% 할인됩니다.</td>
</tr>

</table>

## 💬 커뮤니티 및 지원

- **QQ 그룹**: Agent2API 교류 그룹, 그룹 번호 `1127656296`(QQ에서 번호를 검색해 참여하세요). 질문과 피드백은 그룹에서 환영합니다.
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
| Antigravity(Google, Gemini) | ✓ | ✓ | ✓ | — | — | — |
| OfficeAce(Huawei Cloud, OfficeClaw) | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| 사용자 지정 프로바이더 | ✓ | — | ✓ | — | — | — |

> 참고: 모델 목록의 ✓는 온라인 자동 가져오기를 지원한다는 뜻입니다(일부 플랫폼은 내장 정적 목록도 함께 사용). ZCode는 내장 정적 목록만 제공합니다. 토큰 자동 갱신의 ✗는 해당 플랫폼에 사용 가능한 갱신 수단이 없다는 뜻입니다(계정은 자격 증명이 유효한 동안 정상적으로 사용 가능). Qoder 출석은 중국판에서만 지원됩니다.

`/v1/chat/completions`, `/v1/responses`, `/v1/messages`(`count_tokens` 포함)와 `/v1/models`는 모든 플랫폼에 동일하게 적용됩니다. 모델 매핑, 전역 우선순위 큐, 429 폴백, 이그레스 프록시, 아웃바운드 핑거프린트 마스킹, 요청 리포트 역시 전 플랫폼 공통입니다.

> **이 프로젝트는 학습과 교류 목적으로만 제공됩니다.** 비공식 클라이언트 형태로 자신의 계정 로그인 상태를 재사용하므로 업스트림 서비스 약관에 부합하지 않을 수 있습니다. 위험(계정 정지 등 포함)은 사용자가 부담하며, 상업적 사용과 과금 우회는 금지됩니다. 자세한 내용은 [사용 고지](#-사용-고지)와 [LICENSE](../LICENSE)를 참고하세요.
>
> 이 프로젝트는 개인용 로컬 프록시 도구로, 각 업스트림 벤더 및 공식 제품과 아무런 관련이 없습니다(목록은 [사용 고지](#-사용-고지) 참고). 인터페이스 형태는 각 사 클라이언트 통신을 관찰한 것이며 업스트림은 언제든 바뀔 수 있습니다.

## 🚀 빠른 시작

[Releases](https://github.com/aimod-cc/agent2api/releases/latest)에서 설치 프로그램(NSIS, 중국어 간체, 기본 설치 경로 `C:\Program Files\Agent2API`, 설치 시 관리자 권한 필요)을 내려받아 설치 후 실행하면 됩니다. **Node나 다른 런타임은 필요하지 않습니다.**

1. 첫 실행 시 앱 프로세스 안에서 로컬 게이트웨이(포트 3065)가 시작되고 메인 창이 열립니다. 이전 버전의 데이터 디렉터리나 데이터 파일이 발견되면 마이그레이션 안내 대화상자가 표시되며, 안내에 따라 진행하면 됩니다.
2. "계정" 페이지에서 "계정 추가"를 누르고 프로바이더를 고른 뒤, 대화상자 안내대로 로그인하거나 자격 증명을 입력하면 됩니다(웹 로그인 / SMS 인증 코드 / 자격 증명 붙여넣기 / 이 PC 로그인 상태 가져오기).
3. OpenAI 클라이언트의 `base_url`을 `http://127.0.0.1:3065/v1`로 설정하고 `api_key`는 아무 값이나 넣으면 됩니다(예: `sk-local`. 인증을 켜지 않으면 서버는 검사하지 않습니다).

창을 닫아도 기본적으로 트레이로 최소화될 뿐이며 게이트웨이는 백그라운드에서 계속 전달합니다. 완전히 종료하려면 트레이 아이콘을 오른쪽 클릭해 "종료"를 선택하세요.

### 확인

게이트웨이가 뜨면 curl로 연결을 확인합니다(`model`에는 `GET /v1/models`에 실제로 있는 이름을 넣으세요):

```bash
curl http://127.0.0.1:3065/health
curl http://127.0.0.1:3065/v1/models

curl http://127.0.0.1:3065/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"deepseek-v4.1-flash","messages":[{"role":"user","content":"你好"}],"stream":true}'
```

클라이언트 연동 예시(Python SDK):

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:3065/v1", api_key="sk-local")
resp = client.chat.completions.create(
    model="deepseek-v4.1-flash",     # 목록에 있는 아무 프로바이더의 모델 — GET /v1/models 참고
    messages=[{"role": "user", "content": "你好"}],
)
print(resp.choices[0].message.content)
```

**브라우저에서 도는 페이지**(자체 제작 Web UI 등)가 `fetch`로 바로 붙으면 교차 출처 프리플라이트에 거부됩니다 — 게이트웨이는 기본적으로 CORS에 응답하지 않아 프리플라이트가 API 키 검사에 걸려 401이 됩니다. 해법: ① 설정의 "보안 → 게이트웨이 교차 출처 접근"을 켜기(출처 `*`. 어떤 웹 페이지든 이 PC 게이트웨이를 두드릴 수 있으므로 "게이트웨이 키"도 함께 설정하세요); ② 페이지를 게이트웨이와 동일 출처로 — 로컬 정적 서버가 페이지를 제공하고 `/v1`을 `127.0.0.1:3065`로 리버스 프록시.

### LAN 접근

기본적으로 `127.0.0.1`만 수신합니다. "설정 → 일반 → LAN 접근"을 켜면 모든 네트워크 어댑터에서 수신하고, LAN 기기는 API 주소를 이 PC의 IP로 지정하기만 하면 됩니다(화면에 전체 주소 표시). 보안을 위해 켜기 전에 패널 관리자 등록이 필요합니다. 관리 API는 관리자 세션 또는 게이트웨이 키를 요구하며, 활성화된 키가 없으면 전달 API도 거부합니다(켜는 과정에서 "기본" 키가 자동 추가됩니다). 웹 패널의 LAN 공개도 선택할 수 있습니다(기본 비공개). 변경은 재시작 후 적용됩니다.

## 🐳 Docker 배포

```bash
docker run -d --name agent2api --restart unless-stopped \
  -p 3065:3065 -v ./data:/data \
  aimodcc/agent2api:latest
```

브라우저에서 `http://<호스트>:3065`를 열면 첫 방문 시 **관리자 계정 등록**으로 안내합니다. 로그인 후 "게이트웨이 키" 페이지에서 클라이언트용 API 키를 만들면 됩니다 — `http://<호스트>:3065/v1`이 OpenAI 호환 엔드포인트이며, 키를 만들기 전에는 전달을 거부합니다. 모든 상태(SQLite 데이터베이스 / 설정 / 로그)는 `./data` 볼륨 하나에 저장됩니다.

compose 사용자용(amd64 / arm64 이미지 모두 제공):

```yaml
services:
  agent2api:
    image: aimodcc/agent2api:latest
    container_name: agent2api
    restart: unless-stopped
    ports:
      - "3065:3065"
    volumes:
      - ./data:/data
```

환경 변수(모두 선택 사항):

| 변수 | 설명 |
| --- | --- |
| `AGENT2API_ADMIN_USER` + `AGENT2API_ADMIN_PASSWORD` | 관리자 계정과 비밀번호 사전 설정(비밀번호는 평문으로 입력, 시작 시 자동 해시). 미설정 시 패널에서 등록 |
| `AGENT2API_PANEL_PORT` | 패널을 별도 포트로 분리: 설정하면 패널(UI + `/api/*`)이 해당 포트만 수신하고, 공개 측에는 메인 포트만 매핑해 관리 표면을 내부에 둘 수 있습니다(패널 포트는 루프백에 바인딩. `127.0.0.1:3066:3066`처럼 기재) |
| `AGENT2API_HOST` / `AGENT2API_PROXY_PORT` | 수신 주소(기본 `0.0.0.0`) / 포트(기본 `3065`) |
| `AGENT2API_ALLOW_NO_KEY` | `1`이면 fail-closed 해제(키 미설정이어도 `/v1` 허용, 순수 내부망 한정) |
| `AGENT2API_CAPTCHA_ENABLED` | 로그인 페이지 사람 확인 위젯: 기본 `1`(켜짐), `0`은 끔 |
| `AGENT2API_SELF_UPDATE` | 컨테이너 안의 업데이트 창구: 기본 `1`(켜짐. 패널에서 "다운로드 후 업데이트" 가능). `0`이면 꺼지고 수동 업그레이드만 가능 |
| `WORKBUDDY_UPDATE_REPO` + `WORKBUDDY_UPDATE_API_BASE` | 심화: "업데이트 확인 / 업데이트 다운로드"를 자체 소스로 교체합니다. 전자는 저장소를 바꾸고(기본값은 업스트림 `aimod-cc/agent2api`, fork 후 변경 가능), 후자는 API 루트를 바꿉니다(기본 `https://api.github.com`, GitHub와 같은 형태의 release JSON을 반환해야 함). 자체 소스 설정 후 다운로드 허용 목록은 **그 소스 자신의 호스트**만 추가로 허용합니다(스킴은 소스를 따름: `http://`를 설정하면 평문 다운로드도 허용. 로컬 연습 / 내부 미러용) |

소스에서 빌드하려면: 이 저장소를 클론한 뒤 `docker compose up -d --build`(이미지에는 게이트웨이와 패널만 들어가며 Rust 도구 체인은 포함되지 않습니다).

**웹 패널 기능 차이**(이 PC에 데스크톱 클라이언트가 없음에서 비롯됩니다): 웹 로그인(WorkBuddy / Qoder / Cline), SMS 인증 코드, 자격 증명 붙여넣기는 완전히 사용 가능합니다. 이 PC 콜백이 필요한 웹 로그인(AutoClaw / CatPaw / Accio / CodeArts / Trae)과 "이 PC 로그인 상태 가져오기"는 사용할 수 없으니 자격 증명 붙여넣기를 쓰세요.

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

### 자주 쓰는 스크립트

```bash
npm run tauri:install      # 데스크톱 의존성 설치(npm --prefix desktop-tauri install 과 동일)
npm run tauri:dev          # 개발 모드로 데스크톱 앱 실행(핫 리로드 포함)
npm run tauri:build        # 데스크톱 설치 프로그램 빌드

npm run build:icon         # 아이콘 소스 이미지 생성(아이콘 디자인을 바꾼 뒤 실행하고, 이어서 tauri icon 실행)
```

패키징 산출물은 `target/release/bundle/nsis/Agent2API_<버전>_x64-setup.exe`(현재 약 3.0 MB. `src-tauri/.cargo/config.toml` 이 cargo 의 `target-dir` 을 프로젝트 루트의 `target/` 으로 지정합니다).

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

<a href="https://star-history.com/#aimod-cc/agent2api&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date" />
  </picture>
</a>

<div align="center">

**[최신 버전 내려받기](https://github.com/aimod-cc/agent2api/releases/latest)** • **[문제 보고](https://github.com/aimod-cc/agent2api/issues)**

이 프로젝트가 도움이 되었다면 ⭐️ Star 를 눌러 주세요!

</div>

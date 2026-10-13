# Agent2API · 멀티 프로바이더 로컬 게이트웨이

[简体中文](./README.md) | [English](./README.en.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | **한국어** | [Português (BR)](./README.pt-BR.md)

여러 AI 데스크톱 클라이언트의 로그인 상태를 로컬 **OpenAI 호환 API 게이트웨이**로 묶어 하나의 `base_url`로 제공합니다. 멀티 프로바이더 계정 관리, 모델 관리(활성화 / 삭제 / 매핑), 아웃바운드 핑거프린트 마스킹, 이그레스 프록시, 요청 리포트를 갖추고 있으며 바로 쓸 수 있는 Tauri 데스크톱 앱도 함께 제공합니다. 사용자 지정 `base_url`을 지원하는 OpenAI 클라이언트라면 무엇이든 `http://127.0.0.1:3065/v1`을 엔드포인트로 삼아 이들 각 사의 모델 한도를 호출할 수 있습니다 — API 키도 필요 없고, 클라이언트 소스 수정도 필요 없습니다.

플랫폼별 리버스 프록시 지원 현황(✓ 지원 · ✗ 미지원 · — 해당 없음):

| 플랫폼 | LLM 요청 | 토큰 자동 갱신 | 모델 목록(원격 갱신) | 잔액 조회 | 출석 | 수령 항목 |
| --- | :--: | :--: | :--: | :--: | :--: | :--: |
| WorkBuddy 중국판 | ✓ | ✓ | ✓ 원격 + 정적 폴백 | ✓ | ✓ 일일 출석 | — |
| WorkBuddy 국제판 | ✓ | ✓ | ✓ 원격 + 정적 폴백 | ✓ | ✗ 출석 이벤트 없음 | — |
| 小浣熊 | ✓ | ✓ | ✓ 원격 + 정적 폴백 | ✓ | ✓ 데스크톱 로그인 포인트 | — |
| CatPaw | ✓ | ✗ 갱신 수단 없음 | ✓ 원격 + 정적 폴백 | ✓ | ✗ | — |
| AutoClaw(중국판 / 국제판) | ✓ | ✓ | ✓ 원격 + 정적 폴백 | ✓ | ✓ 일일 출석 | — |
| Qoder | ✓ | ✓ | ✓ 원격(지역별) + 정적 폴백 | ✓ | ✓ 중국판만 | — |
| Cline(Free / Pass) | ✓ | ✓ | ✓ 원격 + 정적 폴백 | ✓ | — | — |
| Accio(국제판 / 중국판) | ✓ | ✓ | ✓ 원격 + 정적 폴백 | ✓ 사용률만 | — | — |
| ZCode(중국판 / 국제판) | ✓ | ✗ | ✗ 정적 목록 | ✓ 플랜 잔액 | — | ✓ 기간 한정 플랜(수동) |
| CodeArts | ✓ | ✓ 원샷 로테이션 | ✓ 원격(3개 소스 병합) | ✓ 두 개의 장부 | — | ✓ 일일 보너스(수동) |
| Trae | ✓ | ✓ 매회 교체 | ✓ 원격만 | ✓ 두 개의 장부 | — | — |
| Loomy(iFlytek) | ✓ | ✗ 갱신 API 없음 | ✓ 원격만 | ✓ 두 개의 포인트 장부 | ✓ 일일 지급 포인트 갱신 | — |
| KukuAI(Baidu Wenku) | ✓ | ✗ 갱신 API 없음 | ✓ 원격 + 정적 폴백 | ✓ 포인트 잔액 | ✓ 일일 출석(무료 포인트) | — |
| MonkeyCode(长亭科技, 중국판 / 국제판) | ✓ | ✗ 갱신 API 없음 | ✓ 원격만 | — | — | — |
| Command Code | ✓ | ✗ 정적 API 키 | ✓ 원격 + 정적 폴백 | — | — | — |
| Antigravity(Google, Gemini) | ✓ | ✓ | ✓ 원격 + 정적 폴백 | — | — | — |
| OfficeAce(Huawei Cloud, OfficeClaw) | ✓ | ✓ 제어 영역 자동 갱신 | ✓ 원격만 | ✓ 포인트 잔액 | ✓ 일일 보너스(출석 + 신규 혜택) | — |
| 사용자 지정 프로바이더 | ✓ Chat 패스스루 / Responses / Anthropic | — | ✓ 수동 등록 + 서버 측 가져오기 | — | — | — |

세 가지 대화 프로토콜 진입점(`/v1/chat/completions`, `/v1/responses`, `/v1/messages`, 그리고 `/v1/messages/count_tokens`)과 `/v1/models`는 모든 플랫폼에 동일하게 적용되며, 차이는 각 업스트림이 표의 항목을 할 수 있는지에만 있습니다. 모델 매핑, 전역 우선순위 큐, 429 폴백, 이그레스 프록시, 아웃바운드 핑거프린트 마스킹, 요청 리포트 역시 전 플랫폼 공통입니다.

> **이 프로젝트는 학습과 교류 목적으로만 제공됩니다.** 비공식 클라이언트 형태로 자신의 계정 로그인 상태를 재사용하므로 업스트림 서비스 약관에 부합하지 않을 수 있습니다. 위험(계정 정지 등 포함)은 사용자가 부담하며, 상업적 사용과 과금 우회는 금지됩니다. 자세한 내용은 [사용 고지](#사용-고지)와 [LICENSE](./LICENSE)를 참고하세요.
>
> 이 프로젝트는 개인용 로컬 프록시 도구로, 각 업스트림 벤더 및 공식 제품과 아무런 관련이 없습니다(목록은 [사용 고지](#사용-고지) 참고). 인터페이스 형태는 각 사 클라이언트 통신을 관찰한 것이며 업스트림은 언제든 바뀔 수 있습니다.

---

## 목차

- [빠른 시작](#빠른-시작)
- [Docker 배포](#docker-배포)
- [화면 미리보기](#화면-미리보기)
- [프로젝트 구조](#프로젝트-구조)
- [개발 및 빌드](#개발-및-빌드)
- [사용 고지](#사용-고지)
- [라이선스](#라이선스)

---

## 빠른 시작

Releases에서 설치 프로그램(NSIS, 중국어 간체, 기본 설치 경로 `C:\Program Files\Agent2API`, 설치 시 관리자 권한 필요)을 내려받아 설치 후 실행하면 됩니다. **Node나 다른 런타임은 필요하지 않습니다.**

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

---

## Docker 배포

```bash
docker run -d --name agent2api --restart unless-stopped \
  -p 3065:3065 -v ./data:/data \
  aimodcc/agent2api:latest
```

브라우저에서 `http://<호스트>:3065`를 열면 첫 방문 시 **관리자 계정 등록**으로 안내합니다(이후 로그인에 사용). 로그인 후 "게이트웨이 키" 페이지에서 클라이언트용 API 키를 만들면 됩니다 — `http://<호스트>:3065/v1`이 OpenAI 호환 엔드포인트이며, 키를 만들기 전에는 전달을 거부하고 첫 키를 만들면 자동으로 복구됩니다. 모든 상태(SQLite 데이터베이스 / 설정 / 로그)는 `./data` 볼륨 하나에 저장됩니다.

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

환경 변수(모두 선택 사항이며, 미리 준비할 것은 없습니다):

| 변수 | 설명 |
| --- | --- |
| `AGENT2API_ADMIN_USER` + `AGENT2API_ADMIN_PASSWORD` | 관리자 계정과 비밀번호 사전 설정(비밀번호는 평문으로 입력, 시작 시 자동 해시). 미설정 시 패널에서 등록 |
| `AGENT2API_PANEL_PORT` | 패널을 별도 포트로 분리: 설정하면 패널(UI + `/api/*`)이 해당 포트만 수신하고, 공개 측에는 메인 포트만 매핑해 관리 표면을 내부에 둘 수 있습니다(패널 포트는 루프백에 바인딩. `127.0.0.1:3066:3066`처럼 기재) |
| `AGENT2API_HOST` / `AGENT2API_PROXY_PORT` | 수신 주소(기본 `0.0.0.0`) / 포트(기본 `3065`) |
| `AGENT2API_ALLOW_NO_KEY` | `1`이면 fail-closed 해제(키 미설정이어도 `/v1` 허용, 순수 내부망 한정) |
| `AGENT2API_CAPTCHA_ENABLED` | 로그인 페이지 사람 확인 위젯: 기본 `1`(켜짐), `0`은 끔 |

소스에서 빌드하려면: 이 저장소를 클론한 뒤 `docker compose up -d --build`(이미지에는 게이트웨이와 패널만 들어가며 Rust 도구 체인은 포함되지 않습니다).

**웹 패널 기능 차이**(이 PC에 데스크톱 클라이언트가 없음에서 비롯됩니다): 웹 로그인(WorkBuddy / Qoder / Cline), SMS 인증 코드, 자격 증명 붙여넣기는 완전히 사용 가능합니다. 이 PC 콜백이 필요한 웹 로그인(AutoClaw / CatPaw / Accio / CodeArts / Trae)과 "이 PC 로그인 상태 가져오기"는 사용할 수 없으니 자격 증명 붙여넣기를 쓰세요.

---

## 화면 미리보기

### 계정

모든 계정이 하나의 큐에 있고(왼쪽에서 두 번째 열이 우선순위) 개별로 켜고 끌 수 있습니다. 한도 쿨다운·유효기간·잔액은 행 안에 표시되고, 잔액 부족은 기본적으로 임계값 미만이면 건너뜁니다.

![계정 관리 페이지: 전역 큐, 모델별 한도 쿨다운, 유효기간과 잔액](./assets/screenshots/accounts.png)

프로바이더를 고르고 로그인하면 끝. 같은 프로바이더의 여러 버전 계정을 보관할 수 있고(예: WorkBuddy 중국판 / 국제판) 전달 시 모델 이름으로 자동 선택됩니다:

![계정 추가: 프로바이더와 버전을 고른 뒤 웹 로그인](./assets/screenshots/add-account.png)

### 리포트

개요: 요청 수 / 성공률 / 토큰 / 상위 모델과 계정·프로바이더 순위, 사용량 도넛과 365일 활동 히트맵:

![리포트 개요: 통계 카드, 상위 계정 / 프로바이더, 모델별·프로바이더별 도넛](./assets/screenshots/report-overview.png)

추세: 24시간 **캐시 적중률**(선)과 **토큰 소비**(면)를 이중 축으로 겹쳐 표시, 아래에 일별 토큰 막대 차트:

![리포트 추세: 캐시 적중률과 토큰 소비 이중 축 추세, 일별 토큰 막대 차트](./assets/screenshots/report-trends.png)

### 예약 작업

백그라운드 작업은 이 페이지에 모여 있습니다(켜기/끄기 · 간격 / 지난 결과 / 지금 실행). 목록은 `~/.agent2api/config.json`의 `scheduledTasks`에 저장되고 변경은 즉시 반영됩니다.

![예약 작업 페이지: 자동 출석, 자격 증명 유지 관리, 모델 카탈로그 갱신 등 백그라운드 작업의 켜기/끄기와 간격](./assets/screenshots/scheduled-tasks.png)

---

## 프로젝트 구조

게이트웨이와 데스크톱 앱은 모두 `desktop-tauri/`에 있습니다. 백엔드는 `src-tauri/` 아래의 Rust 인프로세스 HTTP 서버이고, 프런트엔드는 `ui/` 아래의 순수 HTML/CSS/JS입니다.

```
agent2api/
├─ desktop-tauri/
│  ├─ src-tauri/
│  │  ├─ server/                 게이트웨이 본체 crate(agent2api-server, 단독 빌드 가능:
│  │  │                          데스크톱판과 headless 바이너리가 공유. src/server/ 의
│  │  │                          구현과 bin/agent2api-server.rs 에 GUI 의존 없음)
│  │  │  ├─ mod.rs               서비스 조립: ServerState, 시작, 종료, 시작 시 마이그레이션
│  │  │  ├─ http.rs              라우트 테이블, CORS, API 키 미들웨어, body 제한, headless 정적 호스팅
│  │  │  ├─ config.rs / logging.rs / logs_store.rs / errors.rs
│  │  │  ├─ config_migration.rs  1.x 설정 디렉터리 마이그레이션(~/.workbuddy-proxy → ~/.agent2api, 시작 첫 단계)
│  │  │  ├─ request_stats.rs + request_stats/   통계의 시간 창, 기록, 집계, 정리
│  │  │  ├─ core/
│  │  │  │  ├─ providers/        ★ 멀티 프로바이더 계층(이번 개편의 핵심)
│  │  │  │  │  ├─ mod.rs        ProviderKind(내장 각 사. Cline은 할당량 풀로 2개, Accio와
│  │  │  │  │  │                ZCode는 지역으로 2개로 분리) + PROVIDERS 레지스트리 + id 상호 조회
│  │  │  │  │  ├─ adapter.rs    ProviderAdapter trait + adapter_for + implemented_kinds
│  │  │  │  │  ├─ router.rs     모델 이름 → 후보 프로바이더 집합(집계 카탈로그)
│  │  │  │  │  ├─ catalog.rs    집계 모델 카탈로그(목록 병합 / 동명 중복 제거 / 가용성 판정)
│  │  │  │  │  ├─ catalog_cache.rs  각 사 원격 목록의 영속 캐시(재시작 후 읽어오며 내장 목록으로 되돌아가지 않음)
│  │  │  │  │  ├─ refresh_flight.rs  자격 증명 갱신의 싱글플라이트 중복 제거
│  │  │  │  │  ├─ workbuddy.rs  WorkBuddy 어댑터(헤더 집합 / system 주입 / 6004 / 11-128)
│  │  │  │  │  ├─ raccoon/      小浣熊: mod / models / credentials / jwt / oauth / balance
│  │  │  │  │  ├─ catpaw/       CatPaw: adapter(is_stateful) / conversation(턴 상태 기계) /
│  │  │  │  │  │                turn_executor / prepare / decision(턴 판정) / fingerprint /
│  │  │  │  │  │                registry/(세션 레지스트리: 테이블과 핸들 / 계정 동일성 / 무효화) /
│  │  │  │  │  │                messages / blocks / tools / openai(번역 계층) /
│  │  │  │  │  │                upstream_http / image_compress / models / credentials / balance
│  │  │  │  │  ├─ autoclaw/     Zhipu autoglm(중국판 + 국제판 2개 사): region(양 지역 도메인과 동일성) /
│  │  │  │  │  │                adapter / credentials / refresh / crypto / models /
│  │  │  │  │  │                balance / login(SMS 인증 코드, 중국판만) /
│  │  │  │  │  │                oauth(Zai / Google 웹 로그인, 국제판만) / checkin(일일 출석 작업)
│  │  │  │  │  ├─ qoder/        Qoder: adapter / endpoints(두 사이트 주소) / oauth(기기 인증) /
│  │  │  │  │  │                auth / cosy(COSY 서명과 body 인코딩) / protocol(엔벨로프 디코딩) /
│  │  │  │  │  │                chat(세션 방식 전달) / stream / machine(PKCE와 기기 식별) /
│  │  │  │  │  │                credentials / refresh / models / balance
│  │  │  │  │  ├─ cline/        Cline: adapter(Bearer + 제품면 헤더) / credentials(workos: 접두사
│  │  │  │  │  │                + 데스크톱 로그인 상태 + 이름 파싱) / login(WorkOS 기기 인증) /
│  │  │  │  │  │                refresh(싱글플라이트 갱신) / models(두 할당량 풀 + 기본 매핑 시드) /
│  │  │  │  │  │                balance(credit 잔액, 마이크로 credit ÷1e6)
│  │  │  │  │  ├─ accio/        Accio(국제판 + 중국판 2개 사): endpoints(두 지역과 엔드포인트) /
│  │  │  │  │  │                credentials / auth / refresh(싱글플라이트 갱신) /
│  │  │  │  │  │                oauth(PKCE 웹 로그인 + loopback 콜백) /
│  │  │  │  │  │                models(정적 폴백 + /api/llm/config/v2) /
│  │  │  │  │  │                protocol(OpenAI ↔ ADK의 Gemini 스타일 엔벨로프) /
│  │  │  │  │  │                chat(세션 방식 전달) / stream(ADK SSE 풀기) / balance
│  │  │  │  │  ├─ zcode/        ZCode(Zhipu Z.AI, 중국판 + 국제판 2개 사): region(두 지역 도메인과 동일성) /
│  │  │  │  │  │                adapter(무상태, 지역별 파라미터화) / credentials(토큰 + 플랜 JWT) /
│  │  │  │  │  │                oauth(CLI 폴링 로그인) / coding_key(추론용 API 키 교환) / models(정적 테이블) /
│  │  │  │  │  │                balance(플랜 잔액) / plan + claim(플랜 채널과 기간 한정 수령) /
│  │  │  │  │  │                captcha(사람 확인 토큰 풀) / reasoning(GLM-5.3 사고 예산) /
│  │  │  │  │  │                zcode_system.json(시스템 프롬프트)
│  │  │  │  │  ├─ codearts/     CodeArts(Huawei Cloud): signer(Huawei Cloud SDK-HMAC-SHA256,
│  │  │  │  │  │                참조 구현과 바이트 단위 대조) / credentials / dpop(ES256 DPoP proof) /
│  │  │  │  │  │                oauth(PKCE 웹 로그인 + loopback 콜백) / refresh(싱글플라이트 갱신) /
│  │  │  │  │  │                session(chat-session 하트비트와 계정별 동시 입장 제어) / chat(세션 방식 전달) /
│  │  │  │  │  │                stream_fault(HTTP 200 내부의 스트림 오류 엔벨로프) / redact(오류 본문 마스킹) /
│  │  │  │  │  │                models(agent / builtin / 복지 게이트웨이 3소스 병합) /
│  │  │  │  │  │                balance(구독 통계 + 복지 게이트웨이 두 장부) /
│  │  │  │  │  │                welfare(일일 보너스 수령: 멱등 키를 먼저 저장하고 재조회로 이중 확인)
│  │  │  │  │  ├─ trae/         Trae(ByteDance AI IDE SOLO 채널): credentials / device(기기 키 쌍) /
│  │  │  │  │  │                login + oauth(PKCE 웹 로그인 + 토큰 교환 후보) / callback_server
│  │  │  │  │  │                (이 PC의 임의 포트 콜백과 노이즈 필터) / refresh(싱글플라이트 갱신) /
│  │  │  │  │  │                payload(SOLO 엔벨로프를 화이트리스트로 재구성) / headers(SOLO 헤더 집합) /
│  │  │  │  │  │                stream(SSE→chunk 번역) / forward(상태 기반 전달) /
│  │  │  │  │  │                errors(오류 분류와 죽은 설정 목록) / models(get_detail_param 카탈로그) /
│  │  │  │  │  │                usage(혜택 팩 + 플랜 quota 두 장부) / profile(동일성 파싱)
│  │  │  │  │  ├─ loomy/        Loomy(iFlytek): login(SMS 인증 코드) / credentials(session 14일, 갱신 API 없음) /
│  │  │  │  │  │                sign(클라이언트의 HMAC-SHA1 서명 헤더 복제) / endpoints / client(통합 게이트웨이) /
│  │  │  │  │  │                models(/api/v1/models 원격 카탈로그만, 업스트림에 내장 폴백 목록 없음) /
│  │  │  │  │  │                balance(영구 포인트 + 일일 지급 두 장부) / checkin(그날 첫 로그인 시 지급 포인트 갱신)
│  │  │  │  │  ├─ kuku/         KukuAI(Baidu Wenku "Kuku AI / GenFlowPro", kuku.baidu.com):
│  │  │  │  │  │                adapter(is_stateful) / session(bdstoken/uinfo/uk 3종 캐시) /
│  │  │  │  │  │                engine(STOKEN 교환) / http / login(본사이트 웹 로그인 + 셸 측 Cookie 수집) /
│  │  │  │  │  │                credentials(BDUSS Cookie) / models(정적 폴백 + 원격 갱신) /
│  │  │  │  │  │                chat(세션 생성 → 연산 할당 → SSE) / balance(포인트 잔액) /
│  │  │  │  │  │                checkin(일일 무료 포인트)
│  │  │  │  │  ├─ monkeycode/   MonkeyCode(长亭科技, 중국판 + 국제판 2개 사): region(두 사이트 도메인과 동일성) /
│  │  │  │  │  │                adapter / endpoints(경로 / cookie 이름 / interface_type→CLI 매핑) /
│  │  │  │  │  │                client / credentials(session + imageId) / login(session 붙여넣기와 자동 탐색) /
│  │  │  │  │  │                models(두 사이트 각 한 칸 목록, 원격만) / task(작업 생성) / stream(WS 작업 스트림) /
│  │  │  │  │  │                translate(ACP 이벤트 → chat 프레임. 도구 자동 승인과 질문 자동 응답 포함)
│  │  │  │  │  ├─ commandcode/  Command Code: adapter / endpoints / credentials / login /
│  │  │  │  │  │                fingerprint(결정적 기기 핑거프린트와 8h 보고) / models /
│  │  │  │  │  │                plan(8키 엔벨로프와 params 재작성, session id 파생)
│  │  │  │  │  ├─ antigravity/  Antigravity(Google, Gemini): oauth(refresh token 갱신 + 싱글플라이트) /
│  │  │  │  │                   project(loadCodeAssist → onboardUser 탐색) / credentials /
│  │  │  │  │                   endpoints(3개 환경 베이스와 헤더 집합) / login /
│  │  │  │  │                   models(fetchAvailableModels + 내장 폴백 + 업스트림 실제 이름 매핑) / adapter
│  │  │  │  │  └─ officeace/    OfficeAce(Huawei Cloud, OfficeClaw): oauth(PKCE + P-256 DPoP 셀프 서비스 로그인)/
│  │  │  │  │                   signer(SDK-HMAC 과 V11-HMAC 두 가지 서명 + 벡터 픽스처)/
│  │  │  │  │                   credentials(게이트웨이 Basic + 제어 영역 임시 AK/SK)/
│  │  │  │  │                   chat(무상태 OpenAI 호환)/ models(원격 목록 + 모델별 배율)/
│  │  │  │  │                   probe(사용 가능성 프로브. 연결되지 않는 모델은 기본 숨김)/
│  │  │  │  │                   subscription(구독 / 할당량 / 보상)/ onboarding / checkin / balance
│  │  │  │  ├─ protocol/        프로토콜 변환 계층(각 사 wire ↔ 표준 chat SSE 요청/응답 번역:
│  │  │  │  │                    Anthropic Messages / Responses / NDJSON(Command Code) /
│  │  │  │  │                    Gemini 엔벨로프(Antigravity), 그리고 도구 plan 과 기록 복구)
│  │  │  │  ├─ upstream/        전달 오케스트레이션: 전역 계정 큐 루프(provider_loop) + 전송 본문 처리
│  │  │  │  │                    (payload) + SSE 패스스루/집계 + usage 바이패스 추출 + 비 chat 프로토콜의
│  │  │  │  │                    번역 스트림 연결(translate: Anthropic / NDJSON / Gemini 3계통)
│  │  │  │  ├─ account_store/   계정 저장(전역 우선순위, 한도 쿨다운, 각 사 추가와 가져오기)
│  │  │  │  ├─ models/          모델 카탈로그 기반(workbuddy 내장 목록 + /v3/config 갱신)
│  │  │  │  ├─ model_rules.rs   모델 관리 규칙(비활성화 / 숨김 / 매핑 alias)
│  │  │  │  ├─ api_keys.rs      게이트웨이 키 목록(여러 키, 활성화된 것이 하나라도 있으면 통과)
│  │  │  │  ├─ auth.rs / auth_http.rs / login.rs   세션, 이그레스 전송, 헤드리스 로그인
│  │  │  │  │                    (login/ 아래는 각 사 고유 로그인 흐름: catpaw 의
│  │  │  │  │                    loopback 콜백, qoder 의 기기 인증)
│  │  │  │  ├─ routing.rs / billing/   계정 경로 선택(전역 우선순위 + 한도 쿨다운) / 포인트 출석 운영
│  │  │  │  ├─ proxies.rs / clash.rs / egress.rs   이그레스 프록시와 출구별 캐시 Client
│  │  │  │  ├─ sanitize.rs      아웃바운드 핑거프린트 마스킹(하드코딩 규칙 집합: 헤더 제거 + 템플릿 문장 최소 재작성)
│  │  │  │  ├─ prompt.rs        게이트웨이 자체 시스템 프롬프트(패스스루 / 교체 / 추가 3모드)
│  │  │  │  ├─ degrade.rs       콘텐츠 차단 열화 상태 기계(심사 오탐에 걸리면 다음 날 00:00까지 중립 프롬프트)
│  │  │  │  ├─ credential_maintenance.rs  만료 / 만료 임박 자격 증명의 일괄 갱신
│  │  │  │  ├─ usage_query.rs     잔액 / 포인트 조회(계정 횡단 동시 실행 + 정기 실행 회차의 스냅샷)
│  │  │  │  ├─ scheduled_tasks.rs  간격형 예약 작업 레지스트리와 디스패치 루프(켜기 / 끄기 / 간격 /
│  │  │  │  │                      지난 결과. 설정은 config.json 의 scheduledTasks)
│  │  │  │  └─ account_transfer.rs + account_transfer/ / auto_checkin.rs / update/
│  │  │  │                       가져오기/내보내기(동일성 정규화 포함) / 정기 출석 / 소프트웨어 업데이트
│  │  │  └─ api/                 라우트별 핸들러(health/session/accounts/accounts_usage/
│  │  │                          chat/models/keys/model_manage/stats/logs/billing/
│  │  │                          sanitize/prompt/auto-checkin/scheduled-tasks/update/…)
│  │  ├─ lib.rs                 앱 진입점(설정 디렉터리 마이그레이션 → 설정 → 트레이 → 메인 창 → 백엔드 시작)
│  │  ├─ backend.rs              인프로세스 서버 수명 주기
│  │  ├─ legacy_install.rs       구 "현재 사용자" 설치 정리(디렉터리 / 바로 가기 / 제거 항목 / 자동 시작. release 전용)
│  │  ├─ gateway.rs              셸 측에서 관리 API 를 호출하는 HTTP 클라이언트
│  │  ├─ login.rs / commands.rs  로그인 창과 폴링, 프런트엔드에 노출하는 invoke 명령
│  │  ├─ login_profile.rs        웹 로그인마다 전용 임시 WebView2 데이터 디렉터리(끝나면 삭제)
│  │  ├─ bridge.rs               window.workbuddyDesktop 을 주입하는 브리지 스크립트
│  │  └─ update.rs / settings.rs / state.rs / tray.rs
│  ├─ ui/                        프런트엔드(순수 HTML/CSS/JS, 프레임워크 없음)
│  └─ src-tauri/tauri.conf.json  패키징 설정(NSIS)
├─ build/make-icon.mjs           앱 아이콘 소스 이미지 생성
├─ assets/screenshots/           README 용 이미지(UI 스크린샷)
├─ Dockerfile / .dockerignore    headless 이미지(멀티 스테이지 빌드, 게이트웨이와 패널만 포함)
├─ docker-compose.yml / .env.example   배포(단일 컨테이너: 패널 + 게이트웨이 같은 포트)
└─ package.json                  빌드 스크립트 진입점(tauri:dev / tauri:build / build:icon)
```

---

## 개발 및 빌드

### 요구 사항

- Rust >= 1.77 와 Tauri 2 도구 체인(데스크톱 앱 본체 컴파일용. Windows 에서는 WebView2 런타임도 필요)
- Node.js >= 18.17(`npm run tauri:*` 와 `build/make-icon.mjs` 같은 프런트엔드 빌드 스크립트 실행용. 데스크톱 앱은 실행 시 Node 에 의존하지 않으며 Node 산출물도 포함하지 않습니다)

### 자주 쓰는 스크립트

```bash
npm run tauri:install      # 데스크톱 의존성 설치(npm --prefix desktop-tauri install 과 동일)
npm run tauri:dev          # 개발 모드로 데스크톱 앱 실행(핫 리로드 포함)
npm run tauri:build        # 데스크톱 설치 프로그램 빌드

npm run build:icon         # 아이콘 소스 이미지 생성(아이콘 디자인을 바꾼 뒤 실행하고, 이어서 tauri icon 실행)
```

루트 프로젝트 자체에는 런타임 의존성이 없고, `package.json` 은 위 단축 스크립트 진입점만 제공합니다. 패키징 산출물은 `target/release/bundle/nsis/Agent2API_<버전>_x64-setup.exe`(현재 약 3.0 MB. `src-tauri/.cargo/config.toml` 이 cargo 의 `target-dir` 을 프로젝트 루트의 `target/` 으로 지정합니다).

---

## 사용 고지

### 학습과 교류 목적에 한정

이 프로젝트는 HTTP 리버스 프록시, SSE 스트리밍 패스스루, 다중 업스트림 프로토콜 적응, 데스크톱 패키징(Tauri) 같은 기술 주제를 익히기 위한 실습 프로젝트이며, **개인 학습과 연구 목적으로만** 제공됩니다. 공식 제품이 아니며 Tencent 및 WorkBuddy / CodeBuddy, Meituan 및 CatPaw, SenseTime 및 小浣熊, Zhipu 및 AutoClaw / autoglm, Alibaba 및 Qoder / Accio, Huawei Cloud 및 CodeArts / OfficeAce, ByteDance 및 Trae, iFlytek 및 Loomy, Baidu Wenku 및 KukuAI, 长亭科技 및 MonkeyCode, Command Code, Google 및 Antigravity 와 아무런 관련이 없고, 그 승인이나 후원도 받지 않았습니다.

### 리버스 프록시 동작에 대하여

이 프로젝트가 구현하는 것은 로컬 리버스 프록시입니다. 자신의 컴퓨터에서 자신의 계정 로그인 상태를 재사용해 요청을 공식 업스트림 게이트웨이로 전달합니다. 유료 기능이나 권한 검사를 뚫거나 우회하지 않으며, 사용하는 할당량은 항상 자신의 계정이 원래 가진 범위에서 나옵니다. 다만 분명히 해 둘 점은, 이렇게 "비공식 클라이언트 형태로 로그인 상태를 재사용하는" 전달 방식이 **업스트림 서비스의 이용약관이나 이용 조건에 부합하지 않을 수 있다**는 것입니다. 사용 여부와 그로 인한 모든 결과(속도 제한, 리스크 관리 대상화, 계정 정지나 차단을 포함하되 이에 한정되지 않음)는 전적으로 사용자가 부담합니다.

### 금지 용도

이 프로젝트를 상업적 목적, 이익을 위한 재배포, 대량 계정 운영, 업스트림 과금이나 할당량 제한 우회, 기타 현지 법규를 위반하는 활동에 사용하는 것을 금지합니다. 관련 모델 서비스를 프로덕션 환경이나 상업적 상황에서 호출하려면 공식 채널과 공식 API를 사용하세요.

### 자격 증명과 데이터 위험

이 프로젝트는 계정 자격 증명(`accessToken` / `refreshToken` 등)을 **평문**으로 이 PC의 설정 디렉터리(기본 `~/.agent2api/`)에 저장하며, 내보내기 기능이 만든 파일에도 평문 자격 증명이 들어갑니다. 잘 보관하고, 공개 저장소에 커밋하거나 클라우드 스토리지에 업로드하거나 타인과 공유하지 마세요. 자격 증명 유출로 인한 손실은 사용자가 부담합니다.

### 무보증 및 권리 고지

이 프로젝트는 "있는 그대로" 제공되며, 작성자는 가용성, 안정성, 보안성, 특정 목적 적합성에 대해 어떤 보증도 하지 않습니다. 업스트림 인터페이스는 언제든 바뀔 수 있고 이 프로젝트는 예고 없이 동작하지 않게 되어 유지 보수가 중단될 수 있습니다. 전체 조건은 [LICENSE](./LICENSE)를 참고하세요. 이 프로젝트의 인터페이스 형태와 프로토콜 필드 등의 정보는 각 공식 클라이언트의 공개된 네트워크 통신을 관찰·정리한 것이며, 관련 상표와 서비스의 권리는 각 소유자에게 있습니다. 권리자가 이 프로젝트가 부적절하다고 판단하면 작성자에게 연락해 주세요. 즉시 조정하거나 삭제하겠습니다.

---

## 라이선스

이 프로젝트는 [MIT License](./LICENSE)로 배포되며, 저작권 표시를 유지하는 한 자유롭게 사용·수정·배포할 수 있습니다.

유의할 점은 LICENSE 본문 뒤에 **사용 고지**가 붙어 있고, 그 제3조가 MIT 위에 **제한을 추가**한다는 것입니다(상업적 사용 금지, 이익을 위한 재배포 금지, 대량 계정 운영 금지). 따라서 이 프로젝트는 **순수한 MIT가 아닙니다** — **MIT 조건과 사용 고지가 함께 완전한 라이선스와 사용 조건을 이루며**, 같은 행위에 대해 둘이 다른 결론을 내면 더 엄격한 쪽이 우선합니다. 이 때문에 `Cargo.toml` 이 SPDX `"MIT"` 를 선언하지 않고 `license-file` 로 LICENSE 를 가리킵니다.

---

## Star History

<a href="https://star-history.com/#aimod-cc/agent2api&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date" />
  </picture>
</a>

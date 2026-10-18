# 설치와 배포

이 페이지는 세 가지 제공 형태 —— **데스크톱**(Windows / macOS / Linux), **서버**(Windows / Linux, 헤드리스), **Docker** —— 의 **전체 설치와 운영 안내**입니다. 무엇을 고를지 모르겠다면 아래 대조표를 먼저 보세요.

어느 형태든 설치 후 관리(계정 추가, 게이트웨이 키 생성, 리포트 확인)는 **브라우저 패널**에서 합니다 —— 패널은 게이트웨이와 같은 포트를 쓰고, 데스크톱 버전만 앱 창이 하나 더 있습니다.

[简体中文](./install.md) | [English](./install.en.md) | [繁體中文](./install.zh-Hant.md) | [日本語](./install.ja.md) | **한국어** | [Português (BR)](./install.pt-BR.md)

## 목차

- [어떤 형태를 고를까](#어떤-형태를-고를까)
- [데스크톱(Windows / macOS / Linux)](#데스크톱windows--macos--linux)
- [서버(Windows / Linux, 헤드리스)](#서버windows--linux-헤드리스)
- [Docker](#docker)
- [확인과 클라이언트 연결](#확인과-클라이언트-연결)
- [LAN 접근](#lan-접근)
- [모바일](#모바일)

## 어떤 형태를 고를까

사용 중인 기기에 맞는 행을 고르세요:

| 형태 | 산출물 | 인터페이스 | 설명 |
| --- | --- | --- | --- |
| **Windows 데스크톱**(권장) | `Agent2API_<버전>_x64-setup.exe` | 앱 창 | NSIS 설치 프로그램, 기본 설치 경로 `C:\Program Files\Agent2API`, 설치 시 관리자 권한 필요 |
| **macOS 데스크톱** | `Agent2API_<버전>_universal.dmg` | 앱 창 | 유니버설 바이너리(Apple Silicon과 Intel을 각각 빌드해 병합). macOS 10.15 이상 |
| **Linux 데스크톱** | `Agent2API_<버전>_amd64.deb` 또는 `Agent2API_<버전>_amd64.AppImage` | 앱 창 | deb는 시스템에 설치(webkit2gtk-4.1 / gtk3 필요), AppImage는 설치가 필요 없는 단일 파일로 모든 배포판에서 실행 |
| **Windows 서버** (GUI 없음) | `agent2api-server-windows-amd64.zip` | **브라우저 패널** | 명령줄로 설치하고 Windows 서비스로 등록 —— [서버](#서버windows--linux-헤드리스) 참고 |
| **Linux 서버** (GUI 없음) | `agent2api-server-linux-<arch>.tar.gz` | **브라우저 패널** | 명령줄로 설치하고 systemd 로 상주. 같은 패키지가 컨테이너 내 업데이트 페이로드도 겸합니다 |
| **Docker** | `aimodcc/agent2api:latest` | **브라우저 패널** | amd64 / arm64 이미지, [Docker 배포](#docker) 참고 |

> 무엇을 고를까: **데스크톱이 있다**(트레이, 부팅 시 자동 시작, 본机 로그인 상태 가져오기가 필요) → 데스크톱 버전.
> **서버에 그래픽 인터페이스가 없다**(Windows Server / Linux 서버) → 서버 패키지.
> **이미 Docker 를 돌리고 있다** → 이미지.
>
> 데스크톱 버전은 그래픽 세션이 필요하며 **헤드리스 서버에는 설치할 수 없습니다**.
> 서버 패키지는 WebView / GTK 에 전혀 링크하지 않고, 실행 파일 하나와 `ui/` 디렉터리만 있으면 됩니다.

## 데스크톱(Windows / macOS / Linux)

**데스크톱**(Windows / macOS / Linux)은 설치 후 그냥 실행하면 됩니다. **Node나 다른 런타임이 필요 없으며**, 게이트웨이는 앱 프로세스 안에서 돌고 트레이에 상주합니다.

1. 첫 실행 시 앱 프로세스 안에서 로컬 게이트웨이(포트 3065)가 시작되고 메인 창이 열립니다. 이전 버전의 데이터 디렉터리나 데이터 파일이 발견되면 마이그레이션 안내 대화상자가 표시되며, 안내에 따라 진행하면 됩니다.
2. "계정" 페이지에서 "계정 추가"를 누르고 프로바이더를 고른 뒤, 대화상자 안내대로 로그인하거나 자격 증명을 입력하면 됩니다(웹 로그인 / SMS 인증 코드 / 자격 증명 붙여넣기 / 이 PC 로그인 상태 가져오기).
3. OpenAI 클라이언트의 `base_url`을 `http://127.0.0.1:3065/v1`로 설정하고 `api_key`는 아무 값이나 넣으면 됩니다(예: `sk-local`. 인증을 켜지 않으면 서버는 검사하지 않습니다).

창을 닫아도 기본적으로 트레이로 최소화될 뿐이며 게이트웨이는 백그라운드에서 계속 전달합니다. 완전히 종료하려면 트레이 아이콘을 오른쪽 클릭해 "종료"를 선택하세요.

> **macOS 첫 실행**: dmg 안의 앱에는 Apple 코드 서명과 공증이 적용되지 않았습니다(서명에는 유료 Apple Developer 계정이 필요합니다. `.github/workflows/build.yml` 설명 참고). 따라서 Gatekeeper가 첫 실행을 차단하며, "시스템 설정 → 개인정보 보호 및 보안"에서 한 번 수동으로 허용하면 됩니다.
>
> **Linux 데스크톱**: 트레이는 `libayatana-appindicator3`가 필요합니다(대부분의 데스크톱 환경에 포함). 아이콘이 보이지 않으면 `libayatana-appindicator3-1`을 설치하고 다시 실행하세요. AppImage는 첫 실행 전에 실행 권한이 필요합니다(`chmod +x`, "설치 프로그램 열기"를 선택하면 자동으로 부여됩니다). 업데이트: deb는 시스템 설치 프로그램을 거치고(설치 시 비밀번호를 한 번 묻습니다), AppImage는 파일을 교체하면 업그레이드됩니다.
>
> **데스크톱 버전의 차이**: AutoClaw와 智谱清言의 "이 PC 로그인 상태 가져오기"는 Windows에서만 사용할 수 있습니다(Windows DPAPI 암호문을 읽기 때문에 macOS / Linux에서는 해당 섹션이 숨겨집니다). 나머지(小浣熊 / CatPaw / Cline)는 `HOME` 아래 평문 파일을 읽으므로 세 플랫폼 모두에서 사용할 수 있습니다. 트레이, 자동 시작, LAN 접근, 웹 패널 호스팅은 macOS / Linux에서도 동일하게 지원됩니다.

## 서버(Windows / Linux, 헤드리스)

**그래픽 인터페이스가 없는 서버**(Windows Server, Linux 서버, 클라우드 인스턴스)용입니다. 산출물은 모두 같은 `agent2api-server` 바이너리로, 창도 트레이도 없고 WebView / GTK 에 링크하지 않으며 실행 시 Node 나 다른 런타임이 필요 없습니다. 명령은 **설치만** 담당하고, 그 이후의 관리(계정 추가, 키 생성, 리포트 확인)는 모두 브라우저 패널에서 합니다.

서비스는 시스템 서비스로 상주합니다: 부팅 시 자동 시작, 크래시 후 자동 재시작, 콘솔 없는 백그라운드 실행.

### Windows Server

`agent2api-server-windows-amd64.zip` 를 내려받아 압축을 풀고 **관리자 권한으로** PowerShell 을 엽니다:

```powershell
Expand-Archive agent2api-server-windows-amd64.zip -DestinationPath C:\agent2api-pkg
cd C:\agent2api-pkg
.\install-service.ps1
```

스크립트는 프로그램을 `C:\Agent2API` 에, 데이터를 `C:\ProgramData\Agent2API` 에 넣고, `Agent2API` 라는 Windows 서비스(`LocalSystem`, 부팅 시 자동 시작)를 등록하고 방화벽 인바운드 규칙을 추가한 뒤 서비스를 시작합니다.

```powershell
# 위치와 포트 지정
.\install-service.ps1 -InstallDir D:\apps\agent2api -DataDir D:\data\agent2api -Port 8080

# 일상 관리
Get-Service Agent2API
Restart-Service Agent2API
Stop-Service Agent2API

# 제거(데이터 유지). -Purge 를 붙이면 프로그램과 데이터를 함께 삭제
.\install-service.ps1 -Uninstall
```

**업그레이드**: 새 버전의 압축을 풀고 `install-service.ps1` 를 다시 실행하면 됩니다(서비스 중지 → 프로그램과 UI 덮어쓰기 → 재시작. 데이터와 설정은 그대로입니다).

> **Windows 특유의 함정 두 가지는 스크립트가 이미 처리합니다**:
> 서비스 프로세스의 작업 디렉터리는 exe 가 있는 폴더가 아니라 `C:\Windows\system32` 입니다 —— UI 디렉터리는 exe 경로로 찾으므로 영향이 없습니다. 데이터 디렉터리는 서비스 레지스트리의 `AGENT2API_PROXY_HOME` 에 명시적으로 기록합니다(설정하지 않으면 LocalSystem 의 `C:\Windows\system32\config\systemprofile` 아래로 떨어져 찾기 어렵습니다).
> 서비스에는 콘솔이 없으므로 로그는 파일 `C:\ProgramData\Agent2API\logs\server.log` 에도 남습니다.

### Linux (systemd)

```bash
tar -xzf agent2api-server-linux-amd64.tar.gz
cd agent2api-server-linux-amd64      # 압축을 푼 디렉터리 이름에 맞추세요
sudo bash install.sh
```

스크립트는 `agent2api` 시스템 계정(로그인 불가)을 만들고, 프로그램을 `/opt/agent2api` 에, 데이터를 `/var/lib/agent2api` 에, 설정을 `/etc/agent2api/agent2api.env` 에 두고 systemd 유닛을 `enable + start` 합니다.

```bash
# 업그레이드(데이터와 설정은 유지, 이전 바이너리는 .bak 으로 백업)
sudo bash install.sh --upgrade

# 제거(데이터 유지). --purge 를 붙이면 데이터와 설정을 함께 삭제
sudo bash install.sh --uninstall

# 일상 관리
systemctl status agent2api
systemctl restart agent2api
journalctl -u agent2api -f
```

부팅 시 자동 시작, 크래시 후 자동 재시작(`Restart=always`), SIGTERM 우아한 종료(처리 중인 요청을 기다린 뒤 끝나므로 SSE 장기 연결이 끊기지 않습니다)가 모두 설정되어 있습니다. `/etc/agent2api/agent2api.env` 를 수정했다면 `systemctl restart agent2api` 로 반영합니다.

> **베어메탈에서는 `AGENT2API_SELF_UPDATE` 를 켜지 마세요**: 그건 컨테이너 안의 방식입니다(파일을 바꾸고 종료한 뒤 컨테이너 restart 정책이 되살림). 베어메탈에서는 프로그램 파일 소유자가 root 이고 서비스 계정에는 교체 권한이 없어 유닛에서 명시적으로 `0` 으로 설정해 두었습니다. 업그레이드는 `install.sh --upgrade` 로 하세요.

### 설치 후

두 플랫폼 모두 같습니다 —— 브라우저를 엽니다:

```
http://<서버IP>:3065/
```

1. 첫 방문에서 **관리자 계정 등록**을 안내합니다(환경 변수로 미리 설정할 수도 있습니다. 아래 참고)
2. 로그인 후 "게이트웨이 키" 페이지에서 API 키를 만듭니다
3. 클라이언트 `base_url` 을 `http://<서버IP>:3065/v1` 로, `api_key` 를 방금 만든 키로 설정합니다

> ⚠️ **키를 만들기 전에는 `/v1/*` 가 서비스를 거부합니다**(fail-closed). 의도된 동작입니다. 서버 형태는 `0.0.0.0` 에 바인딩하므로, 키 없이 업스트림 쿼터를 네트워크 전체에 여는 건 받아들일 수 없습니다.

### 무인 배포

관리자와 게이트웨이 키 모두 환경 변수로 미리 설정해 브라우저 수작업을 건너뛸 수 있습니다:

| 변수 | 설명 |
| --- | --- |
| `AGENT2API_ADMIN_USER` + `AGENT2API_ADMIN_PASSWORD` | 관리자 미리 설정(비밀번호는 평문으로 입력, 시작 시 bcrypt 로 변환). `AGENT2API_ADMIN_PASSWORD_HASH` 로 완성된 해시를 줄 수도 있습니다 |
| `AGENT2API_PROXY_API_KEY` | 게이트웨이 키를 하나 미리 설정해 "패널에서 첫 키 만들기"를 건너뜀 |
| `AGENT2API_PROXY_HOME` | 데이터 디렉터리 |
| `AGENT2API_HOST` / `AGENT2API_PROXY_PORT` | 수신 주소(기본 `0.0.0.0`) / 포트(기본 `3065`) |
| `AGENT2API_PANEL_PORT` | 패널을 별도 포트로 분리: 관리 플레인이 그 포트에서 수신하고 주 포트는 `/v1/*` 만 제공 |
| `AGENT2API_ALLOW_NO_KEY` | `1`이면 fail-closed 해제(순수 내부망 한정) |
| `AGENT2API_LOG_FILE` | 로그 파일 경로, 기본 `{데이터 디렉터리}/logs/server.log` |
| `AGENT2API_VERBOSE` | `1`이면 debug 로그 활성화 |

Linux 에서는 `/etc/agent2api/agent2api.env` 에, Windows 에서는 서비스 레지스트리 키 `HKLM\SYSTEM\CurrentControlSet\Services\Agent2API\Environment` 에 적습니다(`install-service.ps1` 이 기본 항목을 이미 써 두었고, 스크립트를 다시 실행하면 덮어씁니다).

> **계정은 미리 설정할 수 없습니다**: 계정에는 QR 스캔 / SMS 코드 / 자격 증명 붙여넣기가 필요하고 이는 업스트림 프로토콜이 정하는 부분이라 우회할 수 없습니다. 대량 배포에서는 이미 설정한 **데이터 디렉터리를 통째로 복사**하세요(SQLite 데이터베이스와 `account.key` 가 같은 디렉터리에 있어 계정도 함께 옮겨집니다).

## Docker

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
| `WORKBUDDY_UPDATE_REPO` + `WORKBUDDY_UPDATE_API_BASE` | 심화: "업데이트 확인 / 업데이트 다운로드"를 자체 소스로 교체합니다. 전자는 저장소를 바꾸고(기본값은 업스트림 `Agent2API/agent2api`, fork 후 변경 가능), 후자는 API 루트를 바꿉니다(기본 `https://api.github.com`, GitHub와 같은 형태의 release JSON을 반환해야 함). 자체 소스 설정 후 다운로드 허용 목록은 **그 소스 자신의 호스트**만 추가로 허용합니다(스킴은 소스를 따름: `http://`를 설정하면 평문 다운로드도 허용. 로컬 연습 / 내부 미러용) |

소스에서 빌드하려면: 이 저장소를 클론한 뒤 `docker compose up -d --build`(이미지에는 게이트웨이와 패널만 들어가며 Rust 도구 체인은 포함되지 않습니다).

**웹 패널 기능 차이**( 이 PC에 데스크톱 클라이언트가 없음에서 비롯됩니다): 웹 로그인(WorkBuddy / Qoder / Cline), SMS 인증 코드, 자격 증명 붙여넣기는 완전히 사용 가능합니다. 이 PC 콜백이 필요한 웹 로그인(AutoClaw / CatPaw / Accio / CodeArts / Trae)과 "이 PC 로그인 상태 가져오기"는 사용할 수 없으니 자격 증명 붙여넣기를 쓰세요. 데스크톱에서 "웹 관리 패널도 공개"를 켠 뒤 브라우저로 열어도 같은 차이가 적용됩니다 —— 동일한 화면, 동일한 판정입니다.

### 컨테이너 내 업데이트(선택)

패널의 "소프트웨어 업데이트" 페이지에서 컨테이너 안에서 바로 "다운로드 후 업데이트"를 실행할 수 있습니다. 공식 Release의 `agent2api-server-linux-<arch>.tar.gz`를 받아(`.sha256` 검증) 실행 파일과 `ui/`를 교체한 뒤 프로세스가 정상 종료되면 `restart` 정책이 같은 컨테이너를 다시 띄웁니다. `AGENT2API_SELF_UPDATE`로 제어하며(Dockerfile에서 기본 `1`), `0`으로 두면 수동 업그레이드만 사용합니다.

같은 `agent2api-server-linux-<arch>.tar.gz` 는 [Linux 서버](#linux-systemd) 의 설치 패키지이기도 합니다: 안의 `agent2api-server` 와 `ui/` 가 이 업데이트 경로가 이름으로 가져가는 항목이고, `install.sh` / `agent2api.service` / `agent2api.env.example` 도 함께 들어 있습니다. 업데이트 경로는 앞의 두 항목만 읽고 나머지는 버리므로(`selfupdate::extract_bundle` 참고), 하나의 산출물이 두 용도를 겸합니다 —— 컨테이너에서는 풀어서 교체, 베어메탈에서는 풀어서 `install.sh` 실행.

주의할 점: 컨테이너 내 업데이트 후에는 이미지 태그와 **실제 실행 중인 버전이 어긋납니다**(`docker inspect`에는 여전히 이전 태그가 표시됩니다). `docker compose pull && up -d`로 컨테이너를 재생성하면 이미지에 포함된 버전으로 되돌아갑니다. 버전을 고정하려면 이 진입점을 켜지 마세요.

## 확인과 클라이언트 연결

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

## LAN 접근

기본적으로 `127.0.0.1`만 수신합니다. "설정 → 일반 → LAN 접근"을 켜면 모든 네트워크 어댑터에서 수신하고, LAN 기기는 API 주소를 이 PC의 IP로 지정하기만 하면 됩니다(화면에 전체 주소 표시). 보안을 위해 켜기 전에 패널 관리자 등록이 필요합니다. 관리 API는 관리자 세션 또는 게이트웨이 키를 요구하며, 활성화된 키가 없으면 전달 API도 거부합니다(켜는 과정에서 "기본" 키가 자동 추가됩니다). 웹 패널의 LAN 공개도 선택할 수 있습니다(기본 비공개). 변경은 재시작 후 적용됩니다.

데스크톱 설치 프로그램(Windows / macOS)에는 웹 패널의 `ui/`가 이미 함께 들어 있습니다. "웹 관리 패널도 공개"를 켜면 LAN 기기에서 브라우저로 같은 주소를 열면 관리 화면이 나옵니다 —— Docker 버전과 동일한 화면입니다. **데스크톱 창 자체는 웹 로그인을 거치지 않습니다**(앱에 로그인 페이지가 없습니다). 이 PC의 첫 번째 게이트웨이 키를 자격 증명으로 사용하므로, 이 스위치를 켜는 데 데스크톱에서 추가 로그인이 필요하지 않습니다. 웹 로그인 페이지는 브라우저로 패널을 열 때만 나타납니다.

## 모바일

모바일은 두 가지 형태가 있습니다. 필요에 따라 고르세요:

| 형태 | 설치 방법 | 적합한 용도 |
| --- | --- | --- |
| **브라우저 / PWA** | 휴대폰 브라우저에서 `/m` 열기, "홈 화면에 추가" 가능 | 잠깐 보기, 아무것도 설치하지 않기 |
| **Android APK** | `Agent2API-<버전>.apk` 설치 | 아이콘, 전체 화면, 여러 서버 관리 |

둘 다 같은 UI 코드입니다. 차이는 "누가 페이지를 제공하는가"입니다: PWA는 게이트웨이가 제공하므로("웹 관리 패널도 공개"가 필수) `/m`이 게이트웨이에서 나오고, APK는 UI를 패키지에 넣으므로(오프라인에서도 UI가 뜨고 그 스위치가 필요 없음) 동작이 다릅니다.

### 사전 조건 (형태별)

**데스크톱(Windows / macOS / Linux)**: 두 스위치를 모두 켜야 하며, 하나만으로는 동작하지 않습니다.

1. "설정 → 일반 → LAN 접근" — LAN 기기가 게이트웨이에 접근할 수 있게 합니다. 켜는 과정에서 패널 관리자 등록이 필요하며 "기본"이라는 이름의 키가 자동 추가됩니다.
2. "설정 → 일반 → LAN 접근 → 웹 관리 패널도 공개" — `ui/` 정적 폴더를 제공합니다. **브라우저 / PWA 형태에는 필수** (`/m`은 게이트웨이가 제공하므로 이 스위치가 꺼져 있으면 404). **APK에는 필요 없음** (UI는 패키지 안에 있음). 다만 APK도 1번 스위치는 필요합니다 — 그렇지 않으면 게이트웨이가 `127.0.0.1`만 수신해 휴대폰에서 접근할 수 없습니다.

둘 다 재시작 후 적용됩니다.

**서버 / Docker**: 추가 스위치가 필요 없습니다 — 두 형태 모두 기본적으로 모든 어댑터에서 수신하고 `ui/`를 제공합니다 (Dockerfile에 `AGENT2API_UI_DIR=/app/ui`가 고정되어 있습니다). 컨테이너 또는 서비스 포트가 LAN에서 접근 가능하면 됩니다. `AGENT2API_HOST=127.0.0.1`을 직접 설정했다면 `0.0.0.0`으로 바꾸세요. `AGENT2API_UI_DIR`이 존재하지 않는 디렉터리를 가리키면 패널은 404가 되지만 API는 계속 동작합니다.

### 휴대폰에서 열기 (브라우저 / PWA)

`http://<게이트웨이가 실행되는 기기의 LAN IP>:3065/m`

데스크톱에서는 위 설정 화면에 표시되는 주소(예: `192.168.1.23`)의 끝 `/v1`을 `/m`으로 바꿉니다. Docker / 서버는 호스트의 LAN IP와 매핑된 포트를 사용합니다.

처음 방문하면 로그인 페이지로 이동하며, 패널 관리자로 로그인하면 자동으로 모바일 UI로 돌아옵니다. 관리자가 아직 없으면 로그인 페이지에서 생성을 요청합니다.

### APK 설치 (Android)

처음 실행하면 "서버" 화면이 나옵니다. 단계는 세 가지입니다:

1. "추가"를 눌러 게이트웨이의 `IP:포트`를 입력합니다 (예: `192.168.1.23:3065`, 포트 생략 시 3065);
2. "연결 후 저장"을 누릅니다 — 먼저 연결을 확인하므로 주소 오류나 LAN 접근 미설정을 그 자리에서 알 수 있습니다;
3. 로그인 페이지가 뜹니다. 이 게이트웨이의 관리자 계정과 비밀번호를 입력합니다 (관리자가 없으면 "관리자 계정 만들기"가 됩니다).

**여러 서버**: "추가"를 한 번 더 누르면 됩니다. 로그인 상태는 서버마다 따로 저장되므로 전환해도 재로그인이 필요 없고, 돌아와도 마찬가지입니다. 진입점은 "설정 → 서버"이며 목록의 행을 누르면 전환됩니다.

**APK의 알려진 제한**:

- Android만 제공합니다. iOS는 Apple Developer 계정으로 서명하거나 Xcode로 직접 설치해야 합니다 (사이드로드 서명은 7일 후 만료).
- `http://` LAN 주소에 평문으로 접속합니다. 위험은 "같은 LAN에서 도청 가능"이며, 게이트웨이 자체는 관리자 로그인을 요구합니다. 없애려면 게이트웨이에 TLS를 설정하고 App에 `https://` 주소를 입력하세요 (파싱 로직은 같고 http를 자동 보완하지 않을 뿐입니다).
- UI는 APK 버전을 따르며 게이트웨이 업그레이드를 따라가지 않습니다. 게이트웨이 업그레이드로 새 화면이 생겼다면 그걸 보려면 새 APK가 필요합니다 (데이터 변화는 영향 없음).

### 홈 화면에 추가 (브라우저 / PWA)

선택 — 주소 표시줄 없는 전체 화면이 됩니다. 모바일 UI의 "설정 → 앱으로 설치"에 바로 진입점이 있습니다:

- **Android / Chrome**: 설정 페이지에서 "홈 화면에 추가"를 탭하면 시스템 설치 대화상자가 열립니다. 메뉴 → "홈 화면에 추가" / "앱 설치"로도 가능합니다
- **iOS / Safari**: Safari에는 설치 API가 없어 설정 페이지에 절차를 적어 두었습니다 — 공유 → "홈 화면에 추가"

홈 화면 아이콘으로 열면 모바일 홈 페이지로 바로 이동합니다. 이미 독립 창에서 실행 중이면 설정 페이지에 "설치됨"이 표시됩니다.

새 버전이 있으면 화면 하단에 "재시작 후 업데이트" 바가 뜹니다. 게이트웨이 업그레이드 후 새 UI는 이미 내려받아져 있고, 한 번 누르면 전환됩니다(무시해도 현재 사용에는 영향이 없고 다음 실행 시 적용됩니다).

> **네이티브 앱 / 스토어 패키지가 없는 이유**: 모바일 UI는 항상 **다른 기기에서 실행되는 게이트웨이**에 연결합니다 (게이트웨이는 그 기기의 클라이언트 로그인을 읽고 설정 파일을 써야 하므로 휴대폰에서는 실행할 수 없습니다). 본질은 "HTTP 서비스에 대한 원격 접근"이며 브라우저가 가장 적합한 매체입니다 — 동일 출처 인증, 설치 불필요, 게이트웨이와 함께 업그레이드됩니다. PWA 형태는 전체 화면과 홈 화면 아이콘을 추가로 제공해 네이티브 앱과 같은 사용감을 줍니다.
>
> **Service Worker(오프라인 셸)는 `https` 또는 `localhost`에서만 동작합니다**: LAN 접근은 `http://<IP>`이므로 브라우저가 SW를 제공하지 않아 오프라인일 때 모바일 UI를 열 수 없습니다. 이는 브라우저의 보안 제한이며 기능 부재가 아닙니다 — 정상 사용 시 게이트웨이가 온라인이므로 영향이 없습니다.

### 할 수 있는 일 / 없는 일

| 가능 | 불가 (데스크톱 앱 사용) |
| --- | --- |
| 리포트, 계정 상태, 잔액, 요청 로그, 실행 로그 조회 | 계정 추가 (QR / 토큰 붙여넣기 / 로그인 상태 가져오기) |
| 계정 활성화/비활성화, 자격 증명 갱신, 삭제 | 계정의 자격 증명, 프록시, 속도 제한 규칙 편집 |
| 원탭 체크인, 체크인 기록 조회 | 신규 혜택과 플랜 개별 수령 |
| 모델 활성화/비활성화, 제공자별 필터 | 모델 매핑, 추론 수준, 기능 플래그 편집 |
| 게이트웨이 키 복사, 키 활성화/비활성화 | 키 생성, 제공자/모델 허용 목록 설정 |
| 프록시 활성화/비활성화, 연결 테스트 | 프록시 추가, 구독 관리, 커널 다운로드 |
| 예약 작업 활성화/비활성화, 간격 변경 | 포트, 라우팅, 재시도, 타임아웃, 데이터 보존 변경 |
| 어시스턴트 빠른 조회 (잔액 / 중복 모델 / 오늘 사용량 / 속도 제한 계정) | 어시스턴트 자유 대화와 도구 호출 |
| Agent 테이크오버 상태 확인, 복원 | 클라이언트 설정 변경 (휴대폰은 로컬 파일을 읽을 수 없음) |

데스크톱 사이드바 하단에 "휴대폰에서 열기" 항목이 있으며 같은 페이지로 이동합니다.

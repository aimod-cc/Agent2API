# 패널 관리자 재설정(비밀번호 분실)

[简体中文](./reset-admin.md) | [English](./reset-admin.en.md) | [繁體中文](./reset-admin.zh-Hant.md) | [日本語](./reset-admin.ja.md) | **한국어** | [Português (BR)](./reset-admin.pt-BR.md)

이 문서는 **패널 관리자 비밀번호를 잊어 로그인할 수 없는** 상황을 다룹니다.

비밀번호는 bcrypt 해시로만 저장되므로 "찾아낼" 수는 없습니다. 관리자 레코드를
**삭제**해 패널을 "미등록" 상태로 되돌린 뒤 다시 등록하는 방법뿐입니다. 이 작업은
관리자 계정과 비밀번호만 지우며, 계정 풀·게이트웨이 Key·로그·기타 모든 업무 데이터에는
영향을 주지 않습니다.

> 아직 로그인할 수 있다면 이 문서를 쓰지 마세요. **설정 → 계정 → 비밀번호 변경**을
> 사용하세요. 이 경로는 현재 비밀번호를 요구하며, 변경 후 다른 기기의 로그인이
> 자동으로 해제됩니다.

## 1. 프로그램 중지

레코드는 프로그램이 **완전히 중지된** 상태에서 삭제해야 합니다. 데이터베이스는 WAL
모드로 동작하므로 실행 중에 쓰면 덮어써지거나 잠길 수 있습니다.

| 형태 | 중지 방법 |
| --- | --- |
| Windows 데스크톱 | 트레이 아이콘 우클릭 → **종료**(창만 닫으면 부족합니다. 기본값이 트레이로 숨기므로 프로세스가 남습니다) |
| macOS / Linux 데스크톱 | 트레이 / 메뉴 막대 아이콘 → **종료**(macOS는 `Cmd+Q` 후 트레이 아이콘이 사라졌는지 확인) |
| Windows 서버 | 관리자 PowerShell: `Stop-Service Agent2API` |
| Linux 서버 | `sudo systemctl stop agent2api` |
| Docker | compose 디렉터리에서 `docker compose stop` |

## 2. 데이터베이스 파일 찾기

데이터베이스는 SQLite 단일 파일 `agent2api.db`이며 "설정 디렉터리" 아래에 있습니다.

| 형태 | 경로 |
| --- | --- |
| Windows(데스크톱 / 서버) | `%USERPROFILE%\.agent2api\agent2api.db` |
| macOS / Linux(데스크톱 / 서버) | `~/.agent2api/agent2api.db` |
| Docker(compose 기본) | 호스트의 `./data/agent2api.db`(컨테이너 내부는 `/data/agent2api.db`) |

`AGENT2API_PROXY_HOME`을 설정했다면 설정 디렉터리는 그 값입니다.

패널의 **설정 → 버전 정보 → 데이터 파일** 항목에 전체 경로가 표시됩니다
(로그인할 수 있을 때 미리 적어 두면 편합니다).

## 3. 관리자 레코드 삭제

관리자는 `kv` 테이블에 키 **`panelAdmin`** 으로 저장됩니다.

### `sqlite3`가 있는 경우

```bash
# Windows / macOS / Linux 데스크톱과 서버
sqlite3 ~/.agent2api/agent2api.db "DELETE FROM kv WHERE key='panelAdmin';"

# Windows 경로 표기(PowerShell 또는 CMD)
sqlite3 "%USERPROFILE%\.agent2api\agent2api.db" "DELETE FROM kv WHERE key='panelAdmin';"

# Docker: 먼저 컨테이너를 중지한 뒤 호스트의 data 디렉터리에서 삭제
docker compose stop
sqlite3 ./data/agent2api.db "DELETE FROM kv WHERE key='panelAdmin';"
```

### `sqlite3`가 없는 경우

SQLite GUI 도구(DB Browser for SQLite, DBeaver, Navicat 등)를 써도 됩니다.
`agent2api.db` 열기 → `kv` 테이블 열기 → `key`가 `panelAdmin`인 행 삭제 → 저장.

Python(대부분의 시스템에 기본 포함)도 가능합니다.

```bash
python -c "import sqlite3,os; p=os.path.expanduser('~/.agent2api/agent2api.db'); c=sqlite3.connect(p); c.execute(\"DELETE FROM kv WHERE key='panelAdmin'\"); c.commit(); c.close(); print('done')"
```

## 4. 재시작 후 다시 등록

프로그램을 다시 시작하고(Docker는 `docker compose start`) 패널을 엽니다.

- **데스크톱**: 창이 곧바로 메인 화면으로 들어갑니다(관리자 없음 = 로그인 불필요).
  비밀번호를 다시 설정하려면 **설정 → 계정**을 열면 로그인 페이지에서 등록하라고
  안내합니다. `http://127.0.0.1:3065/login`을 직접 열어 등록해도 됩니다.
- **서버 / Docker**: 패널 주소를 열면 로그인 페이지로 이동하며
  **"처음 사용: 관리자 계정 만들기"**가 표시됩니다. 사용자 이름과 비밀번호를 입력하세요.

등록이 끝나면 패널은 다시 로그인을 요구합니다.

## 5. 주의: 환경 변수는 최초에만 적용

배포 시 다음을 설정했다면:

```bash
AGENT2API_ADMIN_USER=admin
AGENT2API_ADMIN_PASSWORD=your-password
```

이 값들은 **최초 시드 전용**입니다. 데이터베이스에 관리자가 없는 처음 한 번만 기록되고,
이후에는 데이터베이스가 기준이 됩니다(패널에서 비밀번호를 바꿀 수 있는 이유입니다).
따라서:

- 이 환경 변수를 바꿔도 **이미 등록된 비밀번호는 바뀌지 않습니다**.
- `panelAdmin`을 삭제하고 재시작할 때 환경 변수가 남아 있으면 그 값으로 관리자가
  **다시 시드**됩니다. 이 경우 "새 비밀번호"는 환경 변수의 값입니다.

자동 재생성을 원하지 않으면 재시작 전에 배포 설정에서 환경 변수를 제거하세요.

같은 설정에 `AGENT2API_ADMIN_PASSWORD_HASH`(bcrypt 해시, 평문 비밀번호보다 우선)도
있으며 동작은 동일합니다.

## 6. 보안 주의

관리자 레코드를 삭제하면 패널은 "미등록" 상태에서 **인증 없이** 동작합니다.

- 데스크톱: 이 컴퓨터에서만 접근할 수 있어 위험이 제한적이지만, "LAN 접근 허용"이
  켜져 있으면 같은 LAN의 다른 기기도 패널을 열 수 있습니다. 가능한 한 빨리 다시
  등록하세요.
- 서버 / Docker: **패널 포트에 접근할 수 있는 누구나 관리 화면을 장악할 수 있습니다**.
  삭제 후 즉시 재시작하고 등록하세요. 관리 포트가 인터넷에 노출된 경우 특히 주의해야
  합니다.

실수로 데이터베이스를 손상시켰다면 더 진행하지 말고 확인하세요. 데이터베이스 파일 옆에
`backups/` 디렉터리(전체 복원 전 자동 생성되는 안전 백업)가 있을 수 있으며,
`pre-restore-*.db`를 복사해 되돌릴 수 있습니다.

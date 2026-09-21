# lazed

GUI 에이전트 멀티플렉서 — 자체 `lazed` 데몬(헤드리스 터미널 워크스페이스
관리자) 위의 Tauri v2 + React + xterm.js 클라이언트.

데몬이 PTY와 세션 모델(`group > project > workspace > tab > terminal`)을 소유하고,
앱은 unix socket NDJSON API(`~/.local/state/lazed/lazed.sock`)로 붙는 클라이언트다.
창을 닫아도 데몬과 pane 프로세스는 살아 있고, 재실행 시 스냅샷으로 복원된다.

## 기능

- **사이드바** — group > project > workspace > tab > terminal 트리, 에이전트 상태
  배지(working/blocked/done/idle), `events.subscribe` 스트림으로 실시간 갱신.
- **네임드 에이전트** — pane에 이름 붙은 claude/codex/devin/pi. 프롬프트 회송,
  후속 지시, 출력 읽기는 이름으로 대상을 가리킨다 (`lazed agent`).
- **프롬프트 바** (⌘K) — focused pane / 전체 에이전트 / 전체 pane에 브로드캐스트.
- **blocked·done 인박스** (⇧⌘I) + macOS 알림(클릭 시 해당 pane으로 점프), Dock 뱃지.
- **worktree** — ⌘N 새 worktree, ⇧⌘F fan-out(프롬프트 하나 → N개 worktree ×
  N개 에이전트), diff 뷰 라인 주석 → 에이전트 회송, 비교 후 머지.
- **GTD Inbox** (⇧⌘4) — 데몬 소유 capture 큐(`inbox.json` 영속). done/snooze/
  원본 링크 열기/에이전트 위임으로 triage.
- **Automations** (⇧⌘2) — ready-made 프리셋(Jira assigned·Jira @me 멘션·
  GitHub 리뷰 요청·Slack 채널) + 커스텀 셸 폴러 → collect/notify/command/agent/
  inbox 액션.
- **Session Monitor** (⇧⌘3) — 워크스페이스 전체의 에이전트 상태를 한 화면에.
- **Settings** (⌘,) — Jira 등 연동 계정. 토큰은 데몬이 스폰하는 셸/폴러의
  env로 주입된다.

## 단축키

| 키 | 동작 |
|---|---|
| `⌘D` / `⌘T` / `⌘W` | pane 분할 / 새 tab / pane 닫기 |
| `⌘N` / `⇧⌘N` | 선택 프로젝트에 새 worktree / 프로젝트 import |
| `⌘[` `⌘]` / `⇧⌘[` `⇧⌘]` | tab 내 pane 순환 / workspace 순환 |
| `⌘1-9` / `⌃1-9` | N번째 workspace / N번째 tab으로 이동 |
| `⇧⌘Enter` | focused pane 줌 토글 |
| `⌘K` / `⇧⌘A` / `⇧⌘F` / `⇧⌘I` | 프롬프트 바 / 에이전트 시작 / fan-out / blocked·done |
| `⇧⌘1-4` | rail·화면: projects · automations · session · inbox |
| `⌘,` | 설정 |

## CLI

`make install`이 `~/.local/bin/lazed`에 링크하는 바이너리가 데몬이자 CLI다.

- `lazed server [--foreground]` — 데몬 기동. 앱은 실행 중이면 attach, 없으면 spawn.
- `lazed agent` / `lazed pane` / `lazed worktree` — 네임드 에이전트·레이아웃·
  worktree 조작. bare 그룹과 `--help`는 읽기 전용 디스커버리.
- `lazed inbox` — capture 큐 (`add|list|done|reopen|snooze|remove`).
- `lazed task` — 기존 작업 레코드 호환 (`status|read|tell|resume`).
- `lazed term attach <id>` — pane 컨트롤 스트림 (stdin으로 JSON 명령,
  stdout으로 프레임).
- `lazed api <method> [params-json]` — 소켓 API 원샷 호출.
- `lazed status` / `stop` / `restart`, `lazed install` / `uninstall` / `doctor`.

pane 안 프로세스는 `LAZED_TERM`(자기 pane ID)을 env로 받는다 — `lazed pane
split --current` 같은 호출이 GUI 포커스가 아니라 호출자 pane을 가리키게 한다.
pane 안의 에이전트가 lazed를 조작하는 방법은 스킬 문서(`skills/lazed/SKILL.md`) 참조.

## 설치

- **개발**: `make deps` 후 `make dev` (`make web`은 프론트만 :1420).
- **CLI·스킬 링크**: `make install` (release 데몬을 빌드하고 `lazed install` 실행).
  - `~/.local/bin/lazed` → 데몬/CLI 바이너리
  - `~/.agents/skills/lazed`, `~/.claude/skills/lazed` → 에이전트 스킬
  - 만든 링크는 `~/.local/state/lazed/install.json` 매니페스트에 기록된다.
- **앱 번들**: `make dist` → `src-tauri/target/release/bundle/macos/lazed.app`.
  `bin/lazed`(데몬)와 `skills/lazed`를 함께 싣기 때문에 소스 저장소 없이 동작한다.
- **점검**: `lazed doctor` — 링크·데몬·에이전트 바이너리 상태를 출력한다.
- 첫 실행 시 앱이 링크 누락을 감지하면 상단 배너의 Install 버튼이 같은 일을 한다.

## 개발

- `make check` — tsc + biome + `cargo check` (src-tauri, daemon).
- `make test` — vitest + cargo test + `scripts/test_task_runtime.py`
  (임시 state dir의 일회용 데몬에 대한 블랙박스 테스트).
- `make format` / `make icon` / `make clean`.

## 상태 위치

- `~/.local/state/lazed/` — `lazed.sock`, `session.json`, `inbox.json`, `tasks/`,
  `install.json`, `lazed.log` (`LAZED_STATE_DIR`로 재지정).
- `~/.config/lazed/` — 사용자 설정 (`LAZED_CONFIG_DIR`).
- `~/.lazed/worktrees/` — worktree 체크아웃 (`LAZED_WORKTREE_DIR`).

## 제거

- `lazed uninstall` — 매니페스트가 기록한 링크만 제거한다. 상태·설정·워크트리는 남는다.
- `lazed uninstall --purge` — 여기에 더해 `~/.local/state/lazed`, `~/.config/lazed`,
  `~/.lazed/worktrees`, `~/Library/*/com.lazed.app*`까지 지운다. 커밋 안 된
  워크트리 변경이 있으면 `--yes` 없이는 중단한다.
- 구버전 herdr 흔적(`~/.local/bin/herdr`, `~/.herdr` 등)은 건드리지 않고 목록만 출력한다.
- `/Applications/lazed.app`은 직접 휴지통으로 옮긴다.
- `make uninstall`은 `lazed uninstall`의 얇은 래퍼다 (`PURGE=1`, `YES=1` 지원).

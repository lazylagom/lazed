# lazed

GUI 에이전트 멀티플렉서 — **[herdr](https://github.com/safishamsi/herdr)** 위의
Tauri v2 + React + xterm.js 클라이언트.

herdr가 코어다: workspace / tab / pane과 그 ID(`w1`, `w1:t1`, `w1:p1`), PTY, 프레임
스트림, 에이전트 감지·이름·라이프사이클, 네이티브 세션 복원, 원격 SSH는 전부
herdr의 것이다. lazed는 herdr에 없는 것만 덧붙인다:

- **조직층** — `group > project` 계층. herdr workspace를 저장소(project) 아래에
  파일링하고, main 체크아웃과 linked worktree를 구분한다.
- **worktree 파이프라인** — 생성(`git worktree add` → herdr workspace로 열기),
  fan-out, 안전 제거(에이전트 점유 확인·재시도·branch 정리).
- **tasks** — worktree + 에이전트 + 첫 프롬프트를 하나의 `request_id`로 묶은 내구 기록.
- **GTD Inbox / Automations / 알림 / install·update.**

두 데몬은 서로 독립이다. `lazed` 데몬(`~/.local/state/lazed/lazed.sock`)은 조직층
API를 제공하고 herdr 이벤트를 `herdr.*`로 재방송한다. herdr 서버(`herdr --session
lazed`)는 pane과 에이전트를 소유한다. 앱은 부팅 시 둘을 각각 auto-detect-launch하며,
herdr가 없으면 조직층만 동작하는 degraded 모드로 뜬다. 창을 닫아도 둘 다 살아 있다.

## 기능

- **사이드바** — group > project > herdr workspace > tab > pane 트리, herdr의 에이전트
  상태 배지(working/blocked/done/idle), 이벤트 스트림으로 실시간 갱신.
- **네임드 에이전트** — herdr 에이전트. 프롬프트 회송·후속 지시·출력 읽기는
  `herdr agent …`로 이름을 가리킨다.
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
- **Settings** (⌘,) — Jira 등 연동 계정. 토큰은 데몬이 스폰하는 폴러의 env로 주입된다.

## 단축키

| 키 | 동작 |
|---|---|
| ⌘D | 현재 tab에서 focused pane 옆에 pane 분할 |
| ⌘T | 현재 workspace에 새 tab |
| ⌘W | focused pane 닫기 |
| ⌘N | 선택된 project에 새 worktree |
| ⇧⌘N | project import |
| ⌘[ / ⌘] | 같은 tab의 이전/다음 pane |
| ⇧⌘[ / ⇧⌘] | 이전/다음 workspace |
| ⌘1-9 / ⌃1-9 | N번째 workspace / tab |
| ⇧⌘Enter | focused pane 확대 토글 |
| ⌘K / ⇧⌘A / ⇧⌘F | 프롬프트 바 / 에이전트 시작 / fan-out |
| ⇧⌘1-4 | projects / automations / session / inbox |

## CLI

에이전트(그리고 사람)가 pane·에이전트를 다룰 때는 **herdr CLI**를 그대로 쓴다 —
`herdr pane split --current`, `herdr agent start reviewer --kind codex --pane w1:p3`,
`herdr agent prompt reviewer "…" --wait`. 자세한 것은 `herdr --skill`.

lazed CLI는 herdr에 없는 것만 제공한다:

- `lazed worktree create --branch NAME [--repo PATH] [--base REF]` — git worktree를
  만들고 herdr workspace로 열어 project 아래에 파일링. 결과의 `pane_id`로 에이전트를
  시작한다. `lazed worktree list|remove WORKSPACE_ID [--force] [--kill-agents]`.
- `lazed task start --branch NAME --kind KIND -- "프롬프트"` — worktree + 에이전트 +
  첫 프롬프트를 한 `request_id`로. `task status|read|tell|resume|list`.
- `lazed inbox add|list|done|reopen|snooze|remove`.
- `lazed api <method> [params]` — 데몬 API 직접 호출. `lazed api herdr.call
  '{"method":"pane.list"}'`처럼 herdr 패스스루도 된다.
- `lazed herdr status|snapshot|call` — herdr 어댑터 상태·스냅샷(데몬 없이 herdr에
  직접).
- `lazed install|uninstall|doctor`, `lazed status|stop|restart`.

`skills/lazed/SKILL.md`가 에이전트용 계약이다 — herdr 스킬 위에 위 명령만 덧붙인다.

## 설치

herdr가 먼저 있어야 한다(앱 번들에는 포함된다; 개발 시에는 `mise`/Homebrew/`~/.local/bin`).
lazed는 고정 버전(0.9.0)을 기대하고, 다른 버전이 떠 있으면 `herdr.status`의
`compat_warning`으로 알린다. 버전을 올릴 때는 `make schema-check`로 의존 필드를 확인한다.

```sh
make install     # daemon 빌드 + ~/.local/bin/lazed + 에이전트 스킬 링크
```

앱 첫 실행 배너의 Install 버튼도 같은 일을 한다.

## 개발

```sh
make dev         # daemon 릴리즈 빌드 → tauri dev
make check       # tsc + biome + cargo check (app, daemon)
make test        # vitest + cargo test + 데몬 업데이트 라이프사이클 스크립트
make dist        # herdr 바이너리·daemon·스킬을 스테이징하고 앱 번들
```

`LAZED_HERDR_SESSION`(기본 `lazed`)으로 앱·데몬이 붙는 herdr 세션을 바꿀 수 있다.
같은 세션에 `herdr --session lazed`로 TUI를 붙여도 된다. `HERDR_BIN`은 바이너리
override.

## 상태 위치

- `~/.local/state/lazed/` — `lazed.sock`, `session.json`(group/project/workspace 주석),
  `inbox.json`, `tasks/`, `lazed.log`
- `~/.config/lazed/` — `agents.json`(launch spec), 연동 설정
- `~/.lazed/worktrees/<repo>/<branch>` — worktree 체크아웃
- herdr 자체 상태는 `~/.config/herdr/sessions/<session>/`에 있다(lazed가 건드리지 않음).

## 제거

```sh
make uninstall            # CLI/스킬 링크 제거
make uninstall PURGE=1    # + state/config/worktrees
```

- herdr는 별도 제품이므로 제거하지 않는다. 구버전 herdr 흔적(`~/.local/bin/herdr`,
  `~/.herdr` 등)은 건드리지 않고 목록만 출력한다.

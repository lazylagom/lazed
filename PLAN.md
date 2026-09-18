# Lazed — Product & Architecture Plan (v2)

> herdr(런타임·도메인 모델 그대로 사용) + Tauri GUI(Orca UX 패턴)
> 작성일: 2026-09-14 / 상태: draft v2.1 — **Phase 0-4 완료 + §7 백로그 대부분 처리**
> - herdr 0.9.0 (mise) 헤드리스 서버 + `terminal session control` NDJSON 프레임 → xterm.js 렌더/입력 왕복 확인
> - pane opaque ID(w1:p1…)는 서버 재시작 후에도 유지됨 (스냅샷 복원 시 동일 ID/cwd)
> v1→v2 변경: 자체 데몬 재구현(Path A) 폐기 → herdr 바이너리를 런타임으로 사용(Path B).
> laze 도메인 모델(Workspace/Action/Project) 폐기 → herdr의 workspace/tab/pane 그대로 사용.

---

## 1. 제품 정의

herdr 서버를 런타임으로 쓰는 **GUI 에이전트 멀티플렉서**. Tauri v2 + React + xterm.js.
herdr TUI는 사용하지 않는다 — lazed가 유일한 UI 클라이언트.
(단, 멀티 클라이언트 특성상 원하면 별도 터미널에서 `herdr` TUI로 같은 세션에 attach 가능)

사용자가 겪는 것: Orca 수준의 GUI — 사이드바 에이전트 상태 배지, 마우스 네이티브 분할,
worktree fan-out, diff 주석 회송, blocked 인박스. TUI prefix 키와의 단축키 충돌 없음.

---

## 2. 세 제품 역할

| 출처 | 가져갈 것 | 안 가져갈 것 |
|---|---|---|
| herdr | **바이너리 그대로**: headless server daemon, socket API, terminal session 스트림, manifest 기반 상태 감지(22종), 네이티브 세션 복원(`claude --resume` 등), workspace/tab/pane 도메인 모델, worktree API, 원격 SSH | TUI 자체, 플러그인 마켓 |
| Orca | UX 패턴: 사이드바 상태 롤업, worktree fan-out + 비교/머지, diff 라인 주석→에이전트 회송, blocked 수거함, 완료 알림, (후반) 모바일 모니터링 | Electron, 내장 Chromium/Design Mode, Computer Use, 클라우드 계정/아티팩트 |
| laze | 기술 스택 참조: Tauri 셸 구성, xterm.js+WebGL 세팅, FSD 프론트 구조, Biome/Vitest/릴리즈 파이프라인, macOS IME 처리(korean-ime.md) | 도메인 모델 전부(Action/Project/Workspace), PtyManager(PTY는 herdr 소유), tmux 백엔드, auth-server, harness hooks |

---

## 3. 목표 아키텍처

```
┌──────────────────────────────────────────────┐
│ lazed (Tauri v2 — React + xterm.js)       │
│  - herdr workspace/tab/pane를 GUI로 렌더링   │
│  - 상태 배지, 알림, fan-out, diff 리뷰       │
│  - RuntimeClient 어댑터 뒤에서만 herdr 접촉  │
└──────────────┬───────────────────────────────┘
               │ ① socket API: workspace.*/tab.*/pane.*/agent.*
               │ ② terminal session 스트림: NDJSON ANSI 프레임
               │    (observe=read-only / control=read-write)
┌──────────────▼───────────────────────────────┐
│ herdr server (headless daemon, 번들/스폰)    │
│  - PTY 소유, 세션 영속화·복원, 상태 감지     │
│  - lazed가 spawn하거나 실행 중이면 attach │
└──────────────┬───────────────────────────────┘
               │ PTY
        claude / codex / shell …
```

**핵심 결정:**

- **어댑터 경계(`RuntimeClient`)**: lazed 코드는 herdr CLI/socket을 직접 부르지 않고 trait 인터페이스 뒤에서 접근. 두 가지 채널:
  - 제어面: socket API (`workspace.create`, `pane.split`, `agent.prompt`, `event.subscribe` …)
  - 데이터面: pane당 `herdr terminal session control <id> --cols --rows` 프로세스 → stdout의 `terminal.frame`(base64 ANSI)을 xterm.js로, stdin으로 `terminal.input`/`resize`/`scroll` 전송
- **도메인 모델 = herdr 것 그대로**: workspace/tab/pane + agent 상태(blocked/working/done/idle/unknown). lazed 자체 store는 UI 설정, fan-out 그룹핑 메타 등 최소한만.
- **데몬 생명주기**: herdr의 auto-detect-launch 패턴 — 앱 부팅 시 실행 중인 서버에 attach, 없으면 spawn. 앱 종료 = detach (에이전트 생존).
- **진화 옵션 유지**: herdr 한계에 닿으면 `RuntimeClient` 구현체만 자체 데몬(v1 계획의 lazedd)으로 교체 가능하도록 경계 유지. v1 문서의 Phase 1-3이 그 설계 초안.

---

## 4. 단계별 로드맵

### Phase 0 — 스파이크: herdr 프레임 → xterm.js 파이프라인
- herdr 설치/버전 고정, `herdr server` spawn → socket attach 확인
- `terminal session control`로 pane 열어 프레임을 xterm.js에 렌더, 키 입력 회송
- **성공 기준**: Tauri 창 안에서 claude/codex TUI를 마우스+키보드로 조작 가능, 지연 체감 없음
- 확인: pane opaque ID가 서버 재시작 후에도 안정적인지 (session.json)

### Phase 1 — 코어 셸 ✅ (완료)
- workspace/tab/pane 사이드바 + 분할 레이아웃 UI (herdr 모델 그대로)
- `events.subscribe`(unix socket 직접 연결)로 상태 배지 실시간 반영
- detach/reattach, 앱 재실행 후 세션 복원 UX
- 키보드 단축키 체계 (Tauri/webview 레벨 — prefix 불필요) + 마우스 드래그 분할
- 구현 노트:
  - 이벤트 엔벨로프는 `{"data":{...},"event":"<name>"}` — 이름이 혼용됨(`pane_created` vs `pane.agent_status_changed`) → 정규화해서 처리
  - `pane.agent_status_changed`/`pane.scroll_changed`/`pane.output_matched`는 per-pane 구독(`pane_id` 필수). 나머지는 전역
  - pane 포커스는 클라이언트-로컬 상태 (herdr API에 pane focus-by-id 없음 — `pane.focus`는 방향 탐색 전용)
  - 스냅샷은 `api snapshot` → `result.snapshot`; pane의 에이전트 표시명은 `agents[]`에서 pane_id로 머지
  - 컨트롤 스트림/이벤트 소켓 모두 재연결 루프 보유 → `herdr server stop` 시 앱이 자동으로 서버 재기동+복원

### Phase 2 — 에이전트 UX ✅
- agent start/prompt/wait를 UI로 노출 (새 에이전트 스폰 모달, 프롬프트 브로드캐스트)
- blocked 인박스: 승인 필요 에이전트를 한 목록에 → 클릭 시 해당 pane으로 점프
- macOS 알림 (done/blocked 전이 시)
- `skills/lazed/SKILL.md`: pane 안 에이전트가 herdr CLI로 다른 pane 조작 가능하게

구현 노트:
- `⇧⌘A` AgentPicker (23종 manifest 카탈로그 필터링) → focused pane에 `agent start`
- `⌘K` PromptBar — 대상: focused pane / all agents / all panes. agent pane은 `agent prompt`, 일반 pane은 `pane send-text`(+\n)
- `⇧⌘I` 또는 titlebar inbox 버튼 → blocked/done pane 목록, 클릭 시 workspace+tab 포커스 체이닝 후 pane 점프
- `tauri-plugin-notification` — blocked/done 전이 시 네이티브 알림 (permission은 첫 전이 시 요청, 실패 무시)

### Phase 3 — Orca식 멀티에이전트 ✅
- worktree fan-out: 프롬프트 하나 → N개 worktree(`worktree.create`) × N개 에이전트 pane
- worktree별 diff 뷰 → 라인 주석 → 해당 에이전트 pane에 회송
- 결과 비교 후 승자 머지 플로우

구현 노트:
- `⇧⌘F` Fanout 모달: repo/base/prefix/에이전트 종류(칩 다중선택)/공통 프롬프트 → kind별 `worktree create --branch <prefix>-<kind>` → root pane에 `agent start` → `agent prompt`
- worktree 응답에 `workspace` + `root_pane` 포함 → 추가 생성 불필요. agent start 전 1.5s 대기(새 셸 init), prompt 전 3s 대기(TUI 부팅)
- 사이드바 worktree workspace에 `⑂` 버튼 → DiffView: `git diff <merge-base>` 파싱(커밋+작업트리 전부), 라인 클릭 → 코멘트 → `agent prompt`로 `file:line — text` 회송
- merge 버튼 → 확인 → `git -C <repo_root> merge --no-ff` (컨플릭트는 에러 문자열로 표시)
- 검증: claude+codex fan-out, feat-a diff → "hi instead of hello" 코멘트 → claude가 브랜치 수정 커밋 → main에 머지됨
- 주의: 에이전트 첫 실행 시 신뢰 프롬프트(claude trust dialog, codex confirm)는 수동 승인 필요 — 자동화하지 않음

### Phase 4 — (후반) 원격/멀티 ✅ (최소 구현)
- SSH 머신 등록 → 원격 herdr 서버 attach (`herdr --remote` 모델 재사용)
- 멀티 클라이언트/TUI 공존 polish, 모바일 read-only (Orca 패턴)

구현 노트:
- `⇧⌘R` 또는 titlebar `⇄ remote` → `user@host` 입력 또는 저장된 머신 선택(`herdr machine list --json` 연동, 로컬 클라이언트 상태라 attach 중에도 항상 로컬 실행) → SSH attach
- named session 지원: 저장 머신의 `session`이 `default`가 아니면 모든 원격 호출이 `herdr --session '<name>'`로 라우팅 (수동 입력은 default 세션)
- 메커니즘: `ssh -- target 'herdr status --json'` → 원격 unix socket 경로 획득 → `ssh -N -L <localsock>:<remotesock>` 포워딩 → 이벤트 스트림은 포워드된 로컬 소켓에 연결
- 원격 시 모든 CLI 호출이 `ssh -- <target> herdr <args>`(shell-quoted)로 라우팅, control 스트림은 ssh 파이프; `git` diff/merge(`worktree_diff`/`worktree_merge`)도 ssh 경유 — 원격 worktree 경로를 로컬 git에 넘기지 않음
- ssh target은 항상 `--` 뒤에 위치시켜 `-o`/`-F` 등 옵션 주입 차단; ssh 실패(transport)와 원격 herdr 에러를 구분해 transport 실패 시 즉시 bail
- 컨텍스트 전환 시 이벤트 소켓 shutdown → 재연결 루프가 새 대상으로 자동 재구독; 연결 중 컨텍스트가 바뀌면 generation 카운터로 스테일 연결 abort. ssh forward 자식이 죽으면 자동 respawn (ServerAliveInterval로 사일런트 단절도 감지)
- control 스트림은 attach **성공 후에만** kill — probe 도중 pane 재연결 retry가 로컬 스트림을 붙여버리는 레이스 방지
- 원격 서버가 안 떠 있으면 `ssh -f -- target 'herdr server'`로 기동 시도 (attach 시 + ensure_server 재연결 경로 모두)
- **미검증 항목**: 실제 SSH 호스트가 없어 e2e 미검증 — 실패 경로(unreachable → 빠른 에러 표시, 로컬 컨텍스트 유지)와 shell quoting/에러 분류는 단위 테스트로 확인. `herdr machine add`로 원격 서버를 준비한 뒤 사용 권장
- 미구현: 머신 add/remove 등 관리 UI(attach 목록만), 동시 멀티 원격, 모바일 read-only — 상세·구현 메모는 §7 백로그 참조

---

## 5. 기술 결정 & 리스크

| 항목 | 결정 | 리스크/메모 |
|---|---|---|
| herdr 의존 | 고정 버전 번들 + `RuntimeClient` 경계 | pre-1.0 스키마 변동 — 업그레이드는 의식적 이벤트로 |
| 배포 | 앱 번들에 herdr 바이너리 포함 vs 미설치 시 `brew`/install.sh 유도 | 번들이 UX상 안전. 라이선스 Apache-2.0 — 재배포 OK, NOTICE/attribution 필요 |
| 프레임 스트림 | `terminal session control` NDJSON | 대량 출력 시 지연/배칭 성능을 Phase 0에서 계측 |
| pane ID 안정성 | Phase 0에서 검증 | 재시작 후 w1:p1 유지 안 되면 매핑 테이블 필요 |
| 도메인 모델 | herdr 것 그대로 | lazed 전용 개념 필요 시 adapter 뒤에서 확장, UI 모델 오염 금지 |
| 멀티 클라이언트 | 초기엔 lazed 1개 가정, herdr의 per-client seen 상태 활용 | 같은 tab을 두 클라이언트가 볼 때 리사이즈 소유권 규칙 있음(문서 확인됨) |
| 플랫폼 | macOS 우선, Linux 지원. Windows는 herdr beta 성숙 후 | |
| UI 스택 | React 전면 (laze의 imperative DOM 혼합은 가져오지 않음) | xterm.js만 canvas 렌더 |

---

## 6. AI 코딩 어시스턴트용 프롬프트

### 프롬프트 1 — 스캐폴딩 + 스파이크
```
Lazed는 Tauri v2 + Rust + React(xterm.js) 데스크탑 앱으로, herdr 서버(헤드리스 데몬)의
GUI 클라이언트입니다. 도메인 모델은 herdr의 workspace/tab/pane을 그대로 사용합니다.

1. Tauri v2 + React 19 + xterm.js(@xterm/addon-webgl) + Vite + Biome 프로젝트를 스캐폴딩해줘.
   프론트는 FSD(app/entities/features/widgets/shared) 구조.
2. Rust 측에 RuntimeClient trait을 정의해줘:
   - control: herdr socket API 호출 (workspace/tab/pane/agent CRUD, event subscribe)
   - data: `herdr terminal session control <pane> --cols N --rows M` 자식 프로세스 관리,
     stdout NDJSON terminal.frame(base64 ANSI) → 프론트로 전달, stdin으로 terminal.input 등 전송
3. 스파이크 검증: herdr server spawn → pane 생성 → 프레임이 xterm.js에 렌더되고
   키 입력이 프로세스에 도달하는 것까지.
```

### 프롬프트 2 — 코어 셸
```
herdr의 workspace/tab/pane 모델을 미러링하는 GUI를 구현해줘:
1. 사이드바: workspace > tab > pane 트리 + agent 상태 배지 (event.subscribe로 실시간 갱신)
2. pane 레이아웃: split right/down, 드래그 리사이즈, 탭 전환. 각 pane은 session control 스트림에 바인딩된 xterm.js 인스턴스.
3. 앱 재시작 시: 실행 중 herdr 서버에 reattach → session.snapshot으로 레이아웃 복원 →
   각 pane에 observe/control 스트림 재연결. 프로세스가 살아있으므로 화면 그대로여야 함.
4. 단축키: 앱 레벨(cmd+계열) 단축키 맵. prefix 개념 없이 설계.
```

### 프롬프트 3 — 에이전트 UX
```
1. 새 에이전트 모달: kind 선택(claude/codex/…), workspace/tab 지정 → pane.split + pane.run
2. agent 상태 전이 감지 시 사이드바 배지 + blocked 인박스 뷰 + macOS 알림
3. 프롬프트 브로드캐스트: 선택한 여러 pane에 같은 텍스트 전송
4. skills/lazed/SKILL.md 작성: pane 안 에이전트가 herdr CLI로 다른 pane/agent를 조작하는 가이드
```

### 프롬프트 4 — fan-out + 리뷰 루프
```
1. fan-out 모달: base repo + N개 에이전트 → worktree.create N개 + 각각 에이전트 pane,
   같은 프롬프트 일괄 전송
2. worktree별 git diff 뷰, 라인 주석 → 해당 에이전트 pane에 프롬프트로 전송
3. 비교 후 선택 머지 + 나머지 worktree 정리(worktree.remove)
```

---

## 7. 오픈 이슈 & 다음 작업

### 검증

- [ ] **실기 SSH e2e** — `herdr machine add <host>`로 원격 준비 → `⇧⌘R` attach → pane 조작/이벤트 스트림/git diff·merge 라우팅/원격 서버 재기동 확인. 실패 경로·셸 quoting·컨텍스트 전환은 단위 테스트로만 확인된 상태. (localhost SSH·등록 머신 없어 아직 미검증)
- [x] **프레임 스트림 고부하 성능 계측** — `scripts/perf_stream.py` (throwaway `--session lazed-perf`). 측정 결과 (200x50 pane): `seq 1 300000` flood → 246 frames/0.13MB/0.33s (herdr가 viewport diff로 coalesce — 스크롤 flood는 클라이언트에 안 닿음). 지속 전면 repaint → ~54 frames/s, ~26KB/s, 평균 프레임 ~480B. idle round-trip(input→echo frame) 15-55ms. 결론: `Channel.send`/`atob`/`term.write` 병목 없음 — 프레임 배칭 불필요. 주의: 마커 검출 시 타이핑된 명령행 echo와 ANSI diff 분할에 유의(스크립트에 split-marker + `clear` 패턴으로 처리).

### 기능 (Phase 4 후속)

- [x] **머신 관리 UI** — `⇧⌘R` 모달에서 목록/attach + rename(✎ 인라인)/remove(✕ 확인) + add는 focused pane에 `pane run 'herdr machine add …'`로 실행해 pane에서 승인. `machine_remove`/`machine_rename`은 로컬 전용 실행(`run_local` — attach 중에도 remote 라우팅 안 함).
- [x] **프로젝트 그룹 (Orca 패턴)** — `session > group(gN) > project > terminal`. 그룹 = 프로젝트의 이름 있는 순서 컬렉션(최대 1개 소속, 미소속은 ungrouped로 플랫 렌더). `group.create/list/rename/remove/assign` + `project.create`의 `group_id` 파라미터. session.json 영속화(`#[serde(default)]`로 구 파일 호환), `group.created/updated/removed` 이벤트 → 스냅샷 리프레시. 사이드바: 접이식 그룹 섹션(헤더에 멤버 수 + 상태 롤업, 클릭 접기/더블클릭 rename, localStorage에 collapse 상태), ⋯ 메뉴로 rename/remove, 프로젝트 ⋯ 메뉴의 "Move to group"으로 배정/해제/새 그룹 생성.
- [x] **GTD Inbox (rail ⇧⌘4)** — 데몬 소유 범용 capture 큐: `inbox.add/list/update/remove` 소켓 API + `lazed inbox` CLI + `<state>/inbox.json` 영속화 (세션과 독립 — 데몬/앱 재시작 무관). `(source,key)` 중복 시 내용만 갱신, done/snoozed 상태 유지; snooze는 list 읽기 시 lazy expiry로 open 복귀 (타이머 스레드 없음). 수집 경로: automation의 새 `inbox` 액션 (기존 폴러 프레임워크 재사용 — Jira/Slack 프리셋 포함, JSON line의 `url|link|permalink` 필드 또는 plain line의 첫 http(s) 토큰이 source 링크가 됨). Rail 두 번째 아이콘 + open 개수 배지; triage = done / snooze(1h·내일 9시·3일) / 원본 링크 열기(`open_url` — http(s)만) / 위임(`task.start` worktree+agent, request_id `inbox-<itemid>` 멱등 — 또는 focused pane 전송). Dock 뱃지 = agent attention + open inbox 합산. 에이전트 blocked/done 모달(⇧⌘I)은 별도 유지.
- [x] **Jira Mention automation 프리셋** — `jira @me` 칩: `/myself`로 accountId 해석 → JQL `(comment ~ "[~accountid:…]" OR description ~ "[~accountid:…]") AND updated >= -14d`로 검색 → 코멘트 ADF의 `mention` 노드를 accountId로 대조해 **멘션 1건당 아이템 1개**를 emit (`id = ISSUE-KEY#commentId`이라 같은 이슈의 후속 멘션도 중복 제거에 걸리지 않음, `url`은 `?focusedCommentId=`로 해당 코멘트를 바로 염). Cloud(api/3 ADF)와 self-hosted(api/2 위키마크업 `[~user]`, 이메일 없으면 Bearer PAT) 모두 대응 — 검색 응답에 `comment` 필드가 없으면 이슈별 `/comment`로 폴백. 폴러는 `python3` 힙독 스크립트(백엔드 변경 없음), `requires`는 `JIRA_BASE`/`JIRA_API_TOKEN`. notify/inbox 액션과 조합해 쓰는 것을 전제.
- [x] **Automations 카탈로그(켜기/끄기)** — Automations 화면 상단 "Ready-made" 카드에 `custom`을 제외한 모든 프리셋(Jira mention @me · Jira assigned · GitHub review · Slack)을 토글 스위치로 나열. 켜면 프리셋 기본값(`inbox` 액션, 5분 주기)으로 자동화를 생성·시작하고, 끄면 일시 중지 — 에디터를 열지 않아도 됨. 저장된 자동화는 새 `preset` 필드(Rust `Automation.preset`, save 입력에 포함)로 카탈로그 행에 매핑되고, 필드가 없는 기존 자동화는 프리셋 명령과 일치할 때만 매핑. 카탈로그 행에서도 편집/즉시 실행/아이템 펼치기 가능, 연동 미설정 시 "open integrations" 링크 표시. 직접 만든 자동화는 아래 "Custom" 섹션에 기존 행(삭제 포함)으로 유지.
- [ ] **동시 멀티 원격** — 현재 단일 `REMOTE` static(`remote_ctx()` 경계). 멀티화하려면 target별 context map + 이벤트/control 스트림을 context별로 라우팅 + pane id 네임스페이스 처리 필요 (서로 다른 서버가 같은 `w1:p1`을 가질 수 있음 — UI에서 서버 프리픽스 필요, herdr 스킬 문서도 동일 경고)
- [ ] **모바일 read-only 뷰** — Orca 모니터링 패턴 참고

### 배포/운영

- [x] **herdr 바이너리 번들** — `bun run dist` = `scripts/fetch-herdr`(→`src-tauri/bin/herdr`, gitignore·shim 거부·PATH shim이면 known dirs까지 계속 탐색) + `tauri build --config src-tauri/tauri.bundle.json`(resources merge — base conf에 두면 bin 없을 때 `cargo test`가 깨져서 분리). `setup()`이 `resource_dir()/bin/herdr` 존재 시 `BUNDLED_HERDR`에 등록 → `herdr_bin()` 해석 순서: `HERDR_BIN`(명시적 override 최우선) → 번들 → PATH → known dirs. `NOTICE`에 herdr(Apache-2.0) 표기. 검증: `lazed.app/Contents/Resources/bin/herdr` 확인됨.
- [x] **herdr 업그레이드 정책** — bootstrap + remote attach/detach 시 `compat_warning()` 재검사 → 타이틀바 `⚠` 표시 (`compatible`/`endpoint_compatible` false 또는 server 버전 ≠ pinned `EXPECTED_HERDR_VERSION` = 0.9.0). 스키마 diff 체크 = `scripts/check_herdr_schema.py` (status/snapshot/machine-list의 의존 필드 존재 검증, herdr 버전업 때 실행).
- [x] pane/session ID 재시작 후 안정성 — Phase 0 검증 완료: 스냅샷 복원 시 동일 ID/cwd 유지.
- [x] **브랜딩/attribution** — `NOTICE` 추가 + 앱 내 표기 완료: 타이틀바 `about` 버튼 → About 모달 (이름/`getVersion()` 버전/NOTICE 전문, `NOTICE?raw` 임포트로 단일 소스 유지).

### 코드 정리 (v2.1)

- **제어面 socket API 전환** — 모든 workspace/tab/pane/agent/worktree/snapshot 호출이 호출당 `herdr` 프로세스 spawn(`run_cli`)에서 unix socket request/response(`api_call` → `{"id","method","params"}`)로 이전. 서버는 연결당 요청 1개 처리(구독 연결만 장기 유지)라 호출마다 단기 연결. 소켓 경로는 CONTEXT_GEN 키 캐시 — remote attach/detach 시 자동 무효화, 원격 호출은 포워드된 로컬 소켓 경유(호출당 `ssh herdr` 제거). transport 실패 시 캐시 재해석+포워드 respawn 후 1회 재시도, 서버 error 응답(`code`/`message`)은 즉시 반환. CLI로 남은 것: `status --json`(소켓 경로/버전 발견), `machine *`(로컬 클라이언트 상태), `server` spawn, `terminal session control`(socket에 스트림 메서드 없음). `pane run`은 `pane.send_input {text, keys:["enter"]}`로 대체(live 세션 검증). 이벤트 스트림은 연결당 요청 1개 제약으로 snapshot → 단일 `events.subscribe`(global+per-pane 통합) 유지 — 신규 pane의 `pane_created` 수신 시 스트림 재시작으로 sub set 갱신(기존 `add_sub` 인-스트림 추가 구독은 서버가 연결을 닫아 실제로는 매번 재접속이었던 것을 명시적으로 정정).
- `DiffView`에 `remove`(worktree 정리) 버튼 + 머지 성공 후 "remove worktree?" 제안 — `worktree_remove` 연결. `parseDiff` 버그 수정: `---`/`+++` 헤더가 del/add로 잘못 분류되던 것 → meta 라인은 파일의 첫 `@@` 이전에만 적용(in-hunk `+++i`/`---x` 콘텐츠 라인은 유지), `\ No newline` 주석은 라인 번호 미소비 (vitest 커버).
- Inbox: 항목별 dismiss ✕ + clear all — working/idle 전이 시 dismiss 자동 해제.
- Sidebar: workspace/tab 라벨 더블클릭 → 인라인 rename (`workspace rename`/`tab rename` 연결).
- Fanout: 고정 3s sleep → `agent get` 폴링(최대 4s, status != unknown 감지) + 1s settle.
- vitest 도입 (`bun run test`) — parseDiff/shQuote 단위 테스트.
- 미사용 `base64` 크레이트 제거, `document.title` 디버그 잔재 제거.
- **완료 알림 (Orca 패턴)** — 데몬이 `working→idle` 전이를 `done`으로 승격(동일 kind가 idle 유지되는 동안 sticky, 다음 전이에서 해제) → `agent.status` 이벤트 → macOS 알림(`notify-rust` 직접 사용: plugin `onAction`은 모바일 전용이라 데스크탑 클릭이 안 옴). 클릭 시 `notification.jump` 이벤트 → 창 raise + 해당 터미널로 점프. 이미 보고 있는 터미널이면 억제(`isFocused` + focused term). Dock 뱃지 = 미해소 inbox 수(blocked+done). `agent.wait`은 `idle` 조건이 `done`도 만족시키며, 폴링 중 세션 락을 잡지 않게 수정(기존엔 until 미충족 wait가 session mutex를 영구 점유해 데몬 전체가 행됐음). `agent.start`는 `agent.status` 이벤트 즉시 브로드캐스트.

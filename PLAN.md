# StayLazy — Product & Architecture Plan (v2)

> herdr(런타임·도메인 모델 그대로 사용) + Tauri GUI(Orca UX 패턴)
> 작성일: 2026-09-14 / 상태: draft v2 — **Phase 0·1 완료**
> - herdr 0.9.0 (mise) 헤드리스 서버 + `terminal session control` NDJSON 프레임 → xterm.js 렌더/입력 왕복 확인
> - pane opaque ID(w1:p1…)는 서버 재시작 후에도 유지됨 (스냅샷 복원 시 동일 ID/cwd)
> v1→v2 변경: 자체 데몬 재구현(Path A) 폐기 → herdr 바이너리를 런타임으로 사용(Path B).
> laze 도메인 모델(Workspace/Action/Project) 폐기 → herdr의 workspace/tab/pane 그대로 사용.

---

## 1. 제품 정의

herdr 서버를 런타임으로 쓰는 **GUI 에이전트 멀티플렉서**. Tauri v2 + React + xterm.js.
herdr TUI는 사용하지 않는다 — staylazy가 유일한 UI 클라이언트.
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
│ staylazy (Tauri v2 — React + xterm.js)       │
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
│  - staylazy가 spawn하거나 실행 중이면 attach │
└──────────────┬───────────────────────────────┘
               │ PTY
        claude / codex / shell …
```

**핵심 결정:**

- **어댑터 경계(`RuntimeClient`)**: staylazy 코드는 herdr CLI/socket을 직접 부르지 않고 trait 인터페이스 뒤에서 접근. 두 가지 채널:
  - 제어面: socket API (`workspace.create`, `pane.split`, `agent.prompt`, `event.subscribe` …)
  - 데이터面: pane당 `herdr terminal session control <id> --cols --rows` 프로세스 → stdout의 `terminal.frame`(base64 ANSI)을 xterm.js로, stdin으로 `terminal.input`/`resize`/`scroll` 전송
- **도메인 모델 = herdr 것 그대로**: workspace/tab/pane + agent 상태(blocked/working/done/idle/unknown). staylazy 자체 store는 UI 설정, fan-out 그룹핑 메타 등 최소한만.
- **데몬 생명주기**: herdr의 auto-detect-launch 패턴 — 앱 부팅 시 실행 중인 서버에 attach, 없으면 spawn. 앱 종료 = detach (에이전트 생존).
- **진화 옵션 유지**: herdr 한계에 닿으면 `RuntimeClient` 구현체만 자체 데몬(v1 계획의 staylazyd)으로 교체 가능하도록 경계 유지. v1 문서의 Phase 1-3이 그 설계 초안.

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
- `skills/staylazy/SKILL.md`: pane 안 에이전트가 herdr CLI로 다른 pane 조작 가능하게

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
- `⇧⌘R` 또는 titlebar `⇄ remote` → `user@host` 입력 → SSH attach
- 메커니즘: `ssh -- target 'herdr status --json'` → 원격 unix socket 경로 획득 → `ssh -N -L <localsock>:<remotesock>` 포워딩 → 이벤트 스트림은 포워드된 로컬 소켓에 연결
- 원격 시 모든 CLI 호출이 `ssh -- <target> herdr <args>`(shell-quoted)로 라우팅, control 스트림은 ssh 파이프; `git` diff/merge(`worktree_diff`/`worktree_merge`)도 ssh 경유 — 원격 worktree 경로를 로컬 git에 넘기지 않음
- ssh target은 항상 `--` 뒤에 위치시켜 `-o`/`-F` 등 옵션 주입 차단; ssh 실패(transport)와 원격 herdr 에러를 구분해 transport 실패 시 즉시 bail
- 컨텍스트 전환 시 이벤트 소켓 shutdown → 재연결 루프가 새 대상으로 자동 재구독; 연결 중 컨텍스트가 바뀌면 generation 카운터로 스테일 연결 abort. ssh forward 자식이 죽으면 자동 respawn (ServerAliveInterval로 사일런트 단절도 감지)
- control 스트림은 attach **성공 후에만** kill — probe 도중 pane 재연결 retry가 로컬 스트림을 붙여버리는 레이스 방지
- 원격 서버가 안 떠 있으면 `ssh -f -- target 'herdr server'`로 기동 시도 (attach 시 + ensure_server 재연결 경로 모두)
- **미검증 항목**: 실제 SSH 호스트가 없어 e2e 미검증 — 실패 경로(unreachable → 빠른 에러 표시, 로컬 컨텍스트 유지)와 shell quoting/에러 분류는 단위 테스트로 확인. `herdr machine add`로 원격 서버를 준비한 뒤 사용 권장
- 미구현: 머신 저장/목록(`herdr machine` 연동 UI), 동시 멀티 원격, 모바일 read-only

---

## 5. 기술 결정 & 리스크

| 항목 | 결정 | 리스크/메모 |
|---|---|---|
| herdr 의존 | 고정 버전 번들 + `RuntimeClient` 경계 | pre-1.0 스키마 변동 — 업그레이드는 의식적 이벤트로 |
| 배포 | 앱 번들에 herdr 바이너리 포함 vs 미설치 시 `brew`/install.sh 유도 | 번들이 UX상 안전. 라이선스 Apache-2.0 — 재배포 OK, NOTICE/attribution 필요 |
| 프레임 스트림 | `terminal session control` NDJSON | 대량 출력 시 지연/배칭 성능을 Phase 0에서 계측 |
| pane ID 안정성 | Phase 0에서 검증 | 재시작 후 w1:p1 유지 안 되면 매핑 테이블 필요 |
| 도메인 모델 | herdr 것 그대로 | staylazy 전용 개념 필요 시 adapter 뒤에서 확장, UI 모델 오염 금지 |
| 멀티 클라이언트 | 초기엔 staylazy 1개 가정, herdr의 per-client seen 상태 활용 | 같은 tab을 두 클라이언트가 볼 때 리사이즈 소유권 규칙 있음(문서 확인됨) |
| 플랫폼 | macOS 우선, Linux 지원. Windows는 herdr beta 성숙 후 | |
| UI 스택 | React 전면 (laze의 imperative DOM 혼합은 가져오지 않음) | xterm.js만 canvas 렌더 |

---

## 6. AI 코딩 어시스턴트용 프롬프트

### 프롬프트 1 — 스캐폴딩 + 스파이크
```
StayLazy는 Tauri v2 + Rust + React(xterm.js) 데스크탑 앱으로, herdr 서버(헤드리스 데몬)의
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
4. skills/staylazy/SKILL.md 작성: pane 안 에이전트가 herdr CLI로 다른 pane/agent를 조작하는 가이드
```

### 프롬프트 4 — fan-out + 리뷰 루프
```
1. fan-out 모달: base repo + N개 에이전트 → worktree.create N개 + 각각 에이전트 pane,
   같은 프롬프트 일괄 전송
2. worktree별 git diff 뷰, 라인 주석 → 해당 에이전트 pane에 프롬프트로 전송
3. 비교 후 선택 머지 + 나머지 worktree 정리(worktree.remove)
```

---

## 7. 오픈 이슈

- [ ] herdr 바이너리 배포: 앱 번들 포함(서명/크기) vs 런타임 설치 유도 — Phase 0에서 결정
- [ ] pane/session ID의 재시작 후 안정성 (session.json 스키마 확인)
- [ ] `terminal session control` 프레임 스트림의 고부하 성능 (scrollback 재생, 대량 출력)
- [ ] herdr 업그레이드 정책: 스키마 diff 체크 루틴
- [ ] staylazy 이름/브랜딩, herdr attribution 표기 위치

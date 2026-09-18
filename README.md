# lazed

GUI 에이전트 멀티플렉서 — `lazed` 데몬(헤드리스 터미널 워크스페이스 관리자) 위의
Tauri v2 + React + xterm.js 클라이언트.

## 설치

- **개발**: `make deps` 후 `make dev`.
- **CLI·스킬 링크**: `make install` (release 데몬을 빌드하고 `lazed install` 실행).
  - `~/.local/bin/lazed` → 데몬 바이너리
  - `~/.agents/skills/lazed`, `~/.claude/skills/lazed` → 에이전트 스킬
  - 만든 링크는 `~/.local/state/lazed/install.json` 매니페스트에 기록된다.
- **앱 번들**: `make dist` → `src-tauri/target/release/bundle/macos/lazed.app`.
  데몬과 스킬을 함께 싣기 때문에 소스 저장소 없이 동작한다.
- **점검**: `lazed doctor` — 링크·데몬·에이전트 바이너리 상태를 출력한다.
- 첫 실행 시 앱이 링크 누락을 감지하면 상단 배너의 Install 버튼이 같은 일을 한다.

## 제거

- `lazed uninstall` — 매니페스트가 기록한 링크만 제거한다. 상태·설정·워크트리는 남는다.
- `lazed uninstall --purge` — 여기에 더해 `~/.local/state/lazed`, `~/.config/lazed`,
  `~/.lazed/worktrees`, `~/Library/*/com.lazed.app*`까지 지운다. 커밋 안 된
  워크트리 변경이 있으면 `--yes` 없이는 중단한다.
- `/Applications/lazed.app`은 직접 휴지통으로 옮긴다.
- `make uninstall`은 `lazed uninstall`의 얇은 래퍼다 (`PURGE=1`, `YES=1` 지원).

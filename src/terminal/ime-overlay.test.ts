// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IMEOverlay } from "./ime-overlay";

const tauriWindowState = vi.hoisted(() => ({
  onDragDropEventMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onDragDropEvent: tauriWindowState.onDragDropEventMock,
  }),
}));

function createMockTerminal() {
  return {
    attachCustomKeyEventHandler: vi.fn(),
    getSelection: vi.fn(() => ""),
    modes: { applicationCursorKeysMode: false },
  };
}

function createTestEnv() {
  const terminal = createMockTerminal();
  const container = document.createElement("div");
  const xtermEl = document.createElement("div");
  xtermEl.className = "xterm";
  container.appendChild(xtermEl);
  document.body.appendChild(container);

  const writeToPty = vi.fn();
  // biome-ignore lint/suspicious/noExplicitAny: test mock
  const overlay = new IMEOverlay(terminal as any, container, writeToPty);
  const input = container.querySelector(
    ".ime-overlay-input",
  ) as HTMLInputElement;
  const preview = container.querySelector(
    ".ime-composition-preview",
  ) as HTMLDivElement;

  return { terminal, container, writeToPty, overlay, input, preview, xtermEl };
}

function fireCompositionEvent(
  el: HTMLElement,
  type: "compositionstart" | "compositionupdate" | "compositionend",
  data = "",
) {
  let event: Event;
  try {
    event = new CompositionEvent(type, { data });
  } catch {
    event = new Event(type);
    Object.defineProperty(event, "data", { value: data });
  }
  el.dispatchEvent(event);
}

function fireKeydown(
  el: HTMLElement,
  opts: KeyboardEventInit & { keyCode?: number; isComposing?: boolean } = {},
) {
  const event = new KeyboardEvent("keydown", { bubbles: true, ...opts });
  if (opts.keyCode !== undefined) {
    Object.defineProperty(event, "keyCode", { value: opts.keyCode });
  }
  if (opts.isComposing !== undefined) {
    Object.defineProperty(event, "isComposing", { value: opts.isComposing });
  }
  el.dispatchEvent(event);
}

function fireInput(
  el: HTMLInputElement,
  value: string,
  opts: {
    inputType?: string;
    isComposing?: boolean;
    data?: string | null;
  } = {},
) {
  el.value = value;
  const event = new Event("input", { bubbles: true });
  if (opts.inputType !== undefined) {
    Object.defineProperty(event, "inputType", { value: opts.inputType });
  }
  if (opts.isComposing !== undefined) {
    Object.defineProperty(event, "isComposing", { value: opts.isComposing });
  }
  if (opts.data !== undefined) {
    Object.defineProperty(event, "data", { value: opts.data });
  }
  el.dispatchEvent(event);
}

describe("IMEOverlay", () => {
  let env: ReturnType<typeof createTestEnv>;

  beforeEach(() => {
    vi.clearAllMocks();
    tauriWindowState.onDragDropEventMock.mockResolvedValue(vi.fn());
    env = createTestEnv();
  });

  afterEach(() => {
    env.overlay.dispose();
    env.container.remove();
  });

  it("xterm의 자체 키보드 처리를 차단한다", () => {
    const handler = env.terminal.attachCustomKeyEventHandler.mock.calls[0][0];
    expect(handler()).toBe(false);
  });

  it("compositionstart/end → 확정 문자열 전송", () => {
    vi.useFakeTimers();
    try {
      fireCompositionEvent(env.input, "compositionstart");
      fireCompositionEvent(env.input, "compositionupdate", "한");
      fireCompositionEvent(env.input, "compositionend", "한");

      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledWith("한");
      expect(env.input.value).toBe("");
    } finally {
      vi.useRealTimers();
    }
  });

  it("일반 문자는 input 이벤트에서 전송한다", () => {
    fireKeydown(env.input, { key: "a" });
    fireInput(env.input, "a");
    expect(env.writeToPty).toHaveBeenCalledWith("a");
  });

  it("한글 자모 keydown은 IME에 위임한다", () => {
    fireKeydown(env.input, { key: "ㅊ" });
    expect(env.writeToPty).not.toHaveBeenCalled();
  });

  it("compositionstart 누락 시 자모-only input은 지연 플러시로 조합된다", () => {
    vi.useFakeTimers();
    try {
      fireInput(env.input, "ㄱ", { inputType: "insertText" });
      fireInput(env.input, "ㅏ", { inputType: "insertText" });
      expect(env.writeToPty).not.toHaveBeenCalled();

      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledTimes(1);
      expect(env.writeToPty).toHaveBeenCalledWith("가");
    } finally {
      vi.useRealTimers();
    }
  });

  it("compositionend 직후 동일한 plain input은 중복 전송하지 않는다", () => {
    vi.useFakeTimers();
    try {
      fireCompositionEvent(env.input, "compositionstart");
      fireCompositionEvent(env.input, "compositionend", "한");
      fireInput(env.input, "한", { inputType: "insertText" });

      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledTimes(1);
      expect(env.writeToPty).toHaveBeenCalledWith("한");
    } finally {
      vi.useRealTimers();
    }
  });

  it("조합 중 특수키는 전송하지 않는다", () => {
    fireCompositionEvent(env.input, "compositionstart");
    fireKeydown(env.input, { key: "Enter" });
    expect(env.writeToPty).not.toHaveBeenCalled();
  });

  /* ── WKWebView insertReplacementText 경로 ──
   * 입력 소스를 한글로 바꾼 직후 첫 조합은 composition* 이벤트 없이
   * insertText(첫 자모) + insertReplacementText(조합 갱신)로 도착한다. */

  it("insertReplacementText 조합은 한 번만 완성형으로 전송된다", () => {
    vi.useFakeTimers();
    try {
      fireKeydown(env.input, { key: "ㅎ" });
      fireInput(env.input, "ㅎ", {
        inputType: "insertText",
        data: "ㅎ",
      });
      fireInput(env.input, "하", {
        inputType: "insertReplacementText",
        data: "하",
      });
      fireInput(env.input, "한", {
        inputType: "insertReplacementText",
        data: "한",
      });

      expect(env.writeToPty).not.toHaveBeenCalled();
      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledTimes(1);
      expect(env.writeToPty).toHaveBeenCalledWith("한");
    } finally {
      vi.useRealTimers();
    }
  });

  it("insertReplacementText 경로에서 여러 음절이 이어진다", () => {
    vi.useFakeTimers();
    try {
      fireInput(env.input, "ㅎ", { inputType: "insertText", data: "ㅎ" });
      fireInput(env.input, "한", {
        inputType: "insertReplacementText",
        data: "한",
      });
      fireInput(env.input, "ㄱ", { inputType: "insertText", data: "ㄱ" });
      fireInput(env.input, "글", {
        inputType: "insertReplacementText",
        data: "글",
      });

      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledTimes(1);
      expect(env.writeToPty).toHaveBeenCalledWith("한글");
    } finally {
      vi.useRealTimers();
    }
  });

  it("영문 입력 직후 insertReplacementText 조합이 와도 영문은 유지된다", () => {
    vi.useFakeTimers();
    try {
      fireKeydown(env.input, { key: "a" });
      fireInput(env.input, "a", { inputType: "insertText", data: "a" });
      expect(env.writeToPty).toHaveBeenCalledWith("a");
      env.writeToPty.mockClear();

      fireInput(env.input, "ㅎ", { inputType: "insertText", data: "ㅎ" });
      fireInput(env.input, "하", {
        inputType: "insertReplacementText",
        data: "하",
      });
      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledTimes(1);
      expect(env.writeToPty).toHaveBeenCalledWith("하");
    } finally {
      vi.useRealTimers();
    }
  });

  it("insertReplacementText pending 중 Enter는 pending을 먼저 보낸다", () => {
    fireInput(env.input, "ㅎ", { inputType: "insertText", data: "ㅎ" });
    fireInput(env.input, "한", {
      inputType: "insertReplacementText",
      data: "한",
    });
    fireKeydown(env.input, { key: "Enter" });

    expect(env.writeToPty).toHaveBeenNthCalledWith(1, "한");
    expect(env.writeToPty).toHaveBeenNthCalledWith(2, "\r");
  });

  it("isComposing 플래그만 붙은 input은 세션 없이도 조합으로 버퍼링한다", () => {
    vi.useFakeTimers();
    try {
      // 세션 추적 없이 isComposing=true로만 도착하는 전환 직후 경로 —
      // 자모가 input.value에 갇혀 유실되면 안 된다
      fireInput(env.input, "ㅇ", {
        inputType: "insertText",
        isComposing: true,
      });
      fireInput(env.input, "이", {
        inputType: "insertText",
        isComposing: true,
      });
      fireInput(env.input, "ㅅ", {
        inputType: "insertText",
        isComposing: true,
      });
      fireInput(env.input, "상", {
        inputType: "insertText",
        isComposing: true,
      });

      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledTimes(1);
      expect(env.writeToPty).toHaveBeenCalledWith("이상");
    } finally {
      vi.useRealTimers();
    }
  });

  it("세션 없는 isComposing 텍스트 뒤에 compositionend가 오면 확정값이 우선한다", () => {
    vi.useFakeTimers();
    try {
      fireInput(env.input, "ㅇ", {
        inputType: "insertText",
        isComposing: true,
      });
      fireInput(env.input, "이", {
        inputType: "insertText",
        isComposing: true,
      });
      fireCompositionEvent(env.input, "compositionend", "이");

      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledTimes(1);
      expect(env.writeToPty).toHaveBeenCalledWith("이");
    } finally {
      vi.useRealTimers();
    }
  });

  it("insertReplacementText가 누적 문자열을 담으면 이전 pending을 대체한다", () => {
    vi.useFakeTimers();
    try {
      fireInput(env.input, "ㄴ", {
        inputType: "insertText",
        isComposing: true,
      });
      fireInput(env.input, "나", {
        inputType: "insertReplacementText",
        isComposing: true,
      });
      fireInput(env.input, "나는", {
        inputType: "insertReplacementText",
        isComposing: true,
      });

      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledTimes(1);
      expect(env.writeToPty).toHaveBeenCalledWith("나는");
    } finally {
      vi.useRealTimers();
    }
  });

  it("insertReplacementText가 새 음절이면 이전 pending에 이어 붙인다", () => {
    vi.useFakeTimers();
    try {
      fireInput(env.input, "한", {
        inputType: "insertReplacementText",
        data: "한",
      });
      fireInput(env.input, "그", {
        inputType: "insertReplacementText",
        data: "그",
      });

      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledTimes(1);
      expect(env.writeToPty).toHaveBeenCalledWith("한그");
    } finally {
      vi.useRealTimers();
    }
  });

  it("입력 소스 전환으로 삼켜진 첫 키는 물리 키코드로 자모를 복원한다", () => {
    vi.useFakeTimers();
    try {
      // "안녕" 입력: ㅇ(KeyD)은 keydown만 오고 input이 삼켜짐
      fireKeydown(env.input, { key: "d", code: "KeyD" });
      fireKeydown(env.input, { key: "ㅏ", code: "KeyK" });
      fireInput(env.input, "ㅏ", { inputType: "insertText" });
      fireKeydown(env.input, { key: "ㄴ", code: "KeyS" });
      fireInput(env.input, "ㄴ", { inputType: "insertText" });
      vi.advanceTimersByTime(70); // 사람 타이핑 간격 — 같은 자모 연타
      fireKeydown(env.input, { key: "ㄴ", code: "KeyS" });
      fireInput(env.input, "ㄴ", { inputType: "insertText" });
      fireKeydown(env.input, { key: "ㅕ", code: "KeyJ" });
      fireInput(env.input, "ㅕ", { inputType: "insertText" });
      fireKeydown(env.input, { key: "ㅇ", code: "KeyD" });
      fireInput(env.input, "ㅇ", { inputType: "insertText" });

      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledTimes(1);
      expect(env.writeToPty).toHaveBeenCalledWith("안녕");
    } finally {
      vi.useRealTimers();
    }
  });

  it("삼켜진 키가 Shift 조합이면 쌍자음/복합모음으로 복원한다", () => {
    vi.useFakeTimers();
    try {
      fireKeydown(env.input, { key: "T", code: "KeyT", shiftKey: true });
      fireKeydown(env.input, { key: "ㅣ", code: "KeyL" });
      fireInput(env.input, "ㅣ", { inputType: "insertText" });

      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledWith("씨");
    } finally {
      vi.useRealTimers();
    }
  });

  it("삼켜진 keydown 뒤에 영문이 오면 자모를 복원하지 않는다", () => {
    fireKeydown(env.input, { key: "d", code: "KeyD" });
    fireKeydown(env.input, { key: "a", code: "KeyA" });
    fireInput(env.input, "a", { inputType: "insertText" });

    expect(env.writeToPty).toHaveBeenCalledTimes(1);
    expect(env.writeToPty).toHaveBeenCalledWith("a");
  });

  it("삼켜진 키의 e.key가 자모이면 그 자모를 그대로 복원한다", () => {
    vi.useFakeTimers();
    try {
      // 전환 직후 keydown이 이미 한글 자모를 보고하는 변형
      fireKeydown(env.input, { key: "ㅇ", code: "KeyD" });
      fireKeydown(env.input, { key: "ㅏ", code: "KeyK" });
      fireInput(env.input, "ㅏ", { inputType: "insertText" });
      fireKeydown(env.input, { key: "ㄴ", code: "KeyS" });
      fireInput(env.input, "ㄴ", { inputType: "insertText" });

      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledWith("안");
    } finally {
      vi.useRealTimers();
    }
  });

  it("composition 입력이 잃어버린 자모를 이미 포함하면 이중 복원하지 않는다", () => {
    vi.useFakeTimers();
    try {
      // 삼켜진 ㅇ 이후 IME가 "안"을 통째로 전달하는 변형 — 복원하면 ㅇ안이 됨
      fireKeydown(env.input, { key: "ㅇ", code: "KeyD" });
      fireKeydown(env.input, { key: "Process", code: "KeyK", keyCode: 229 });
      fireInput(env.input, "안", {
        inputType: "insertCompositionText",
        data: "안",
        isComposing: true,
      });

      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledWith("안");
    } finally {
      vi.useRealTimers();
    }
  });

  it("조합 세션 활성 중의 insertReplacementText는 무시한다", () => {
    vi.useFakeTimers();
    try {
      fireCompositionEvent(env.input, "compositionstart");
      fireInput(env.input, "하", {
        inputType: "insertReplacementText",
        data: "하",
        isComposing: true,
      });
      fireCompositionEvent(env.input, "compositionend", "한");

      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledTimes(1);
      expect(env.writeToPty).toHaveBeenCalledWith("한");
    } finally {
      vi.useRealTimers();
    }
  });

  it("한글이 아닌 insertReplacementText는 일반 텍스트로 전송한다", () => {
    fireInput(env.input, "the", {
      inputType: "insertReplacementText",
      data: "the",
    });
    expect(env.writeToPty).toHaveBeenCalledWith("the");
  });

  it("pending 조합 중 preview에 진행 중 음절을 표시한다", () => {
    fireInput(env.input, "ㅎ", { inputType: "insertText", data: "ㅎ" });
    expect(env.preview.style.display).toBe("block");
    expect(env.preview.textContent).toBe("ㅎ");

    fireInput(env.input, "하", {
      inputType: "insertReplacementText",
      data: "하",
    });
    expect(env.preview.textContent).toBe("하");
  });

  it("blur 시 pending 상태가 다음 입력에 누수되지 않는다", () => {
    fireInput(env.input, "ㅎ", { inputType: "insertText", data: "ㅎ" });
    fireCompositionEvent(env.input, "compositionstart");
    env.input.dispatchEvent(new Event("blur"));

    fireKeydown(env.input, { key: "a" });
    fireInput(env.input, "a");
    expect(env.writeToPty).toHaveBeenCalledTimes(1);
    expect(env.writeToPty).toHaveBeenCalledWith("a");
  });

  /* ── 실제 WKWebView 로그 재현 ──
   * 입력 소스 전환 직후 "안녕" 입력 시 관측된 스트림:
   * 첫 자모가 insertText로 keydown보다 먼저 도착하고, 나머지 자모가
   * 각각 별도 composition 세션("ㅏ", "ㄴ", "녕")으로 커밋된다. */

  it("실제 WKWebView 스트림: 전환 직후 안녕은 조각 세션들을 재조합한다", () => {
    vi.useFakeTimers();
    try {
      // 첫 자모 ㅇ: insertText가 keydown/compositionstart보다 먼저 도착
      fireInput(env.input, "ㅇ", { inputType: "insertText", data: "ㅇ" });
      fireKeydown(env.input, { key: "ㅇ", code: "KeyD", keyCode: 229 });

      // ㅏ: 단독 composition 세션으로 즉시 커밋
      fireCompositionEvent(env.input, "compositionstart");
      fireInput(env.input, "ㅏ", {
        inputType: "insertCompositionText",
        data: "ㅏ",
        isComposing: true,
      });
      fireKeydown(env.input, {
        key: "ㅏ",
        code: "KeyK",
        keyCode: 229,
        isComposing: true,
      });
      fireCompositionEvent(env.input, "compositionend", "ㅏ");

      // ㄴ: 단독 세션
      fireCompositionEvent(env.input, "compositionstart");
      fireInput(env.input, "ㄴ", {
        inputType: "insertCompositionText",
        data: "ㄴ",
        isComposing: true,
      });
      fireKeydown(env.input, {
        key: "ㄴ",
        code: "KeyS",
        keyCode: 229,
        isComposing: true,
      });
      fireCompositionEvent(env.input, "compositionend", "ㄴ");

      // 녕: ㄴ→녀→녕 한 세션으로 정상 조합
      fireCompositionEvent(env.input, "compositionstart");
      fireInput(env.input, "ㄴ", {
        inputType: "insertCompositionText",
        data: "ㄴ",
        isComposing: true,
      });
      fireInput(env.input, "녀", {
        inputType: "insertCompositionText",
        data: "녀",
        isComposing: true,
      });
      fireInput(env.input, "녕", {
        inputType: "insertCompositionText",
        data: "녕",
        isComposing: true,
      });
      fireCompositionEvent(env.input, "compositionend", "녕");

      vi.advanceTimersByTime(400);
      // 완성된 음절은 다음 세션 커밋 시 즉시 전송되고 마지막 음절은 타이머로
      expect(env.writeToPty).toHaveBeenCalledTimes(2);
      expect(env.writeToPty).toHaveBeenNthCalledWith(1, "안");
      expect(env.writeToPty).toHaveBeenNthCalledWith(2, "녕");
    } finally {
      vi.useRealTimers();
    }
  });

  it("실제 WKWebView 스트림: 전환 후 님+공백도 재조합한다", () => {
    vi.useFakeTimers();
    try {
      // 영문 타이핑 후 전환 — 첫 자모 ㄴ이 plain insertText로 선도착
      fireKeydown(env.input, { key: "h", code: "KeyH" });
      fireInput(env.input, "h", { inputType: "insertText", data: "h" });
      fireInput(env.input, "ㄴ", { inputType: "insertText", data: "ㄴ" });
      fireKeydown(env.input, { key: "ㄴ", code: "KeyS", keyCode: 229 });

      fireCompositionEvent(env.input, "compositionstart");
      fireInput(env.input, "ㅣ", {
        inputType: "insertCompositionText",
        data: "ㅣ",
        isComposing: true,
      });
      fireCompositionEvent(env.input, "compositionend", "ㅣ");

      // "ㅁ " — 공백까지 composition에 포함되어 커밋되는 변형
      fireCompositionEvent(env.input, "compositionstart");
      fireInput(env.input, "ㅁ", {
        inputType: "insertCompositionText",
        data: "ㅁ",
        isComposing: true,
      });
      fireInput(env.input, "ㅁ ", {
        inputType: "insertCompositionText",
        data: "ㅁ ",
        isComposing: true,
      });
      fireCompositionEvent(env.input, "compositionend", "ㅁ ");

      vi.advanceTimersByTime(400);
      expect(env.writeToPty).toHaveBeenCalledTimes(3);
      expect(env.writeToPty).toHaveBeenNthCalledWith(1, "h");
      expect(env.writeToPty).toHaveBeenNthCalledWith(2, "님");
      expect(env.writeToPty).toHaveBeenNthCalledWith(3, " ");
    } finally {
      vi.useRealTimers();
    }
  });
});

import { describe, expect, it } from "vitest";
import { shQuote } from "./herdr";

describe("shQuote", () => {
  it("wraps in single quotes", () => {
    expect(shQuote("w1:p1")).toBe("'w1:p1'");
    expect(shQuote("")).toBe("''");
  });

  it("escapes embedded single quotes", () => {
    expect(shQuote("it's")).toBe("'it'\\''s'");
  });

  it("cannot be broken out of", () => {
    expect(shQuote("x'; rm -rf /; echo '")).toBe(
      "'x'\\''; rm -rf /; echo '\\'''",
    );
    expect(shQuote("$(whoami)`id`")).toBe("'$(whoami)`id`'");
  });
});

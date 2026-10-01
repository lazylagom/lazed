import { expect, it, vi } from "vitest";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
import { jiraIssueOf, providerOf } from "./inbox";

it("explicit provider wins and normalizes case", () => {
  expect(providerOf({ source: "manual", provider: "Jira" })).toBe("jira");
  expect(providerOf({ source: "poller", provider: " slack " })).toBe("slack");
});

it("manual and empty sources group under manual", () => {
  expect(providerOf({ source: "manual" })).toBe("manual");
  expect(providerOf({ source: "" })).toBe("manual");
});

it.each([
  ["Jira — issues assigned to me (REST)", "jira"],
  ["Jira mention @me", "jira"],
  ["Slack — channel messages (REST)", "slack"],
  ["GitHub — PRs requesting my review", "github"],
])("legacy automation names infer their provider: %s", (source, want) => {
  expect(providerOf({ source })).toBe(want);
});

it("a provider id alone does not infer (needs a word boundary)", () => {
  expect(providerOf({ source: "jirarest" })).toBe("jirarest");
  expect(providerOf({ source: "jira-rest" })).toBe("jira");
});

it("unknown automation names group under the source itself", () => {
  expect(providerOf({ source: "Standup notes" })).toBe("Standup notes");
});

it("jiraIssueOf reads the dedupe key's head before the comment id", () => {
  expect(jiraIssueOf({ key: "CS-1#12345", title: "x" })).toBe("CS-1");
  expect(jiraIssueOf({ key: "CS-1", title: "x" })).toBe("CS-1");
  expect(jiraIssueOf({ key: "cs-9#description", title: "x" })).toBe("CS-9");
});

it("jiraIssueOf falls back to a PROJ-123 token in title or url", () => {
  expect(jiraIssueOf({ title: "fix for CS-42" })).toBe("CS-42");
  expect(jiraIssueOf({ title: "x", url: "https://j/browse/cs-7" })).toBe(
    "CS-7",
  );
  expect(jiraIssueOf({ title: "no key here" })).toBeNull();
});

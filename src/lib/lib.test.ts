import { describe, expect, it } from "vitest";
import { fuzzyFilter, fuzzyScore } from "./fuzzy";
import { diffLines, hunks } from "./diff";
import { matches, parseBinding } from "./keys";
import { sessionKeyOfRow } from "./rowkey";
import { diffInput } from "./mobileinput";

describe("fuzzy", () => {
  it("matches subsequences and rejects non-matches", () => {
    expect(fuzzyScore("mtm", "myterm main")).not.toBeNull();
    expect(fuzzyScore("xyz", "myterm")).toBeNull();
    expect(fuzzyScore("", "anything")).toBe(0);
  });

  it("ranks substring and boundary matches first", () => {
    const items = ["api-gateway /srv/api", "my project /home/u/proj", "proj /x"];
    expect(fuzzyFilter(items, "proj", (s) => s)[0]).toBe("proj /x");
    expect(fuzzyFilter(items, "api", (s) => s)[0]).toBe("api-gateway /srv/api");
  });

  it("requires every word", () => {
    const items = ["web host1 /srv/web", "web host2 /srv/web"];
    expect(fuzzyFilter(items, "web host2", (s) => s)).toEqual(["web host2 /srv/web"]);
  });
});

describe("diff", () => {
  it("produces adds and deletes", () => {
    const d = diffLines("a\nb\nc\n", "a\nc\nd\n");
    expect(d.map((l) => l.op + l.text)).toEqual([" a", "-b", " c", "+d"]);
  });

  it("collapses unchanged runs", () => {
    const before = Array.from({ length: 20 }, (_, i) => `l${i}`).join("\n");
    const after = before.replace("l10", "X");
    const h = hunks(diffLines(before, after), 1);
    expect(h[0]).toBeNull();
    expect(h.filter((l) => l && l.op !== " ").length).toBe(2);
  });
});

describe("keys", () => {
  const ev = (o: Partial<KeyboardEvent>) =>
    ({ ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, key: "", code: "", ...o }) as KeyboardEvent;

  it("matches Ctrl+Shift+O by code", () => {
    const b = parseBinding("Ctrl+Shift+O");
    expect(matches(ev({ ctrlKey: true, shiftKey: true, key: "O", code: "KeyO" }), b, false)).toBe(true);
    expect(matches(ev({ ctrlKey: true, key: "o", code: "KeyO" }), b, false)).toBe(false);
  });

  it("accepts Cmd+Shift on macOS only", () => {
    const b = parseBinding("Ctrl+Shift+P");
    const e = ev({ metaKey: true, shiftKey: true, key: "P", code: "KeyP" });
    expect(matches(e, b, true)).toBe(true);
    expect(matches(e, b, false)).toBe(false);
  });
});

describe("sessionKeyOfRow", () => {
  it("drops the window part of a row key", () => {
    expect(sessionKeyOfRow("intra/0:@84")).toBe("intra/0");
    expect(sessionKeyOfRow("local/1795621712003")).toBe("local/1795621712003");
    expect(sessionKeyOfRow("local/~eph-1795564925000")).toBe("local/~eph-1795564925000");
  });
});

describe("diffInput", () => {
  it("sends only what a composing keyboard added", () => {
    // "tput" composed letter by letter must arrive once, not as "tputputt".
    let sent = "";
    let out = "";
    for (const v of ["t", "tp", "tpu", "tput", "tput ", "tput c", "tput co", "tput col", "tput cols"]) {
      out += diffInput(sent, v);
      sent = v;
    }
    expect(out).toBe("tput cols");
  });
  it("replaces an autocorrected word with backspaces", () => {
    expect(diffInput("teh", "the ")).toBe("\x7f\x7fhe ");
  });
  it("deletes", () => {
    expect(diffInput("abc", "ab")).toBe("\x7f");
  });
  it("turns a newline into Enter", () => {
    expect(diffInput("ls", "ls\n")).toBe("\r");
  });
  it("counts an emoji as one character to delete", () => {
    expect(diffInput("a🙂", "a")).toBe("\x7f");
  });
});

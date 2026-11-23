export interface Binding {
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  meta: boolean;
  key: string;
}

export function parseBinding(s: string): Binding {
  const parts = s.split("+").map((p) => p.trim().toLowerCase());
  const key = parts.pop() ?? "";
  return {
    ctrl: parts.includes("ctrl") || parts.includes("control"),
    shift: parts.includes("shift"),
    alt: parts.includes("alt") || parts.includes("option"),
    meta: parts.includes("cmd") || parts.includes("meta") || parts.includes("super"),
    key,
  };
}

function eventKey(e: KeyboardEvent): string {
  if (/^Key[A-Z]$/.test(e.code)) return e.code.slice(3).toLowerCase();
  if (/^Digit[0-9]$/.test(e.code)) return e.code.slice(5);
  return e.key.toLowerCase();
}

/** On macOS, Cmd may stand in for Ctrl in a Ctrl+Shift binding. */
export function matches(e: KeyboardEvent, b: Binding, mac: boolean): boolean {
  if (eventKey(e) !== b.key || e.shiftKey !== b.shift || e.altKey !== b.alt) return false;
  if (e.ctrlKey === b.ctrl && e.metaKey === b.meta) return true;
  return mac && b.ctrl && !b.meta && e.metaKey && !e.ctrlKey;
}

export function label(s: string, mac: boolean): string {
  if (!mac) return s;
  return s.replace(/Ctrl\+Shift/i, "⌘⇧").replace(/Ctrl\+/i, "⌃").replace(/Shift\+/i, "⇧").replace(/Alt\+/i, "⌥");
}

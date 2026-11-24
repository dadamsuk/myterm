import type { HostInfo } from "./api";

/** CSS class for a host's status dot (`.hdot.<class>` in app.css). */
export function hostColor(h: HostInfo): string {
  if (h.dormant) return "dormant";
  if (h.connecting) return "connecting";
  if (h.kind === "tmux" && h.connected) return "ok";
  if (h.kind === "ephemeral") return "eph";
  if (h.needs_login) return "login";
  return "down";
}

/** Short status words for a host. */
export function hostStatus(h: HostInfo): string {
  if (h.dormant) return "not connected: click to connect";
  if (h.needs_login) return "needs interactive login";
  return h.status;
}

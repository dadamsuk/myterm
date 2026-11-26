/**
 * Keyboard input on Android (spec 14.4).
 *
 * Soft keyboards compose words in xterm's hidden textarea, and xterm forwards
 * each update of the word as new input, so "tput" arrived as "tputputt". On
 * Android the textarea's text input is handled here instead: every change is
 * sent as the difference from what was already sent, as backspaces then new
 * text. That also covers autocorrect replacing a word. Keys the keyboard
 * sends as real key presses (Enter, Backspace on an empty field, arrows,
 * Ctrl combinations from a hardware keyboard) still go through xterm.
 */

export const DEL = "\x7f";

/** The bytes that turn the terminal's copy of `before` into `after`. */
export function diffInput(before: string, after: string): string {
  let p = 0;
  while (p < before.length && p < after.length && before[p] === after[p]) p++;
  return DEL.repeat([...before.slice(p)].length) + after.slice(p).replace(/\r?\n/g, "\r");
}

export function attachMobileInput(el: HTMLElement, ta: HTMLTextAreaElement, send: (data: string) => void) {
  ta.setAttribute("autocomplete", "off");
  ta.setAttribute("autocorrect", "off");
  ta.setAttribute("autocapitalize", "none");
  ta.setAttribute("spellcheck", "false");
  ta.setAttribute("enterkeyhint", "send");

  let sent = "";
  let composing = false;
  const reset = () => {
    if (composing) return;
    ta.value = "";
    sent = "";
  };
  // Capture on the terminal's element runs before xterm's own listeners on
  // the textarea; stopping there hides the event from xterm.
  const on = (type: string, fn: (ev: Event) => void) =>
    el.addEventListener(type, (ev) => ev.target === ta && fn(ev), true);

  on("compositionstart", (ev) => {
    composing = true;
    ev.stopPropagation();
  });
  on("compositionupdate", (ev) => ev.stopPropagation());
  on("compositionend", (ev) => {
    composing = false;
    ev.stopPropagation();
  });
  // Hardware keyboards fire keypress for printable keys; their text arrives
  // through "input" below, so xterm must not send it a second time.
  on("keypress", (ev) => ev.stopPropagation());
  on("input", (ev) => {
    ev.stopPropagation();
    const v = ta.value;
    const data = diffInput(sent, v);
    sent = v;
    if (data) send(data);
    if (v.includes("\n") || v.length > 256) {
      composing = false;
      reset();
    }
  });
  on("keydown", (ev) => {
    const k = ev as KeyboardEvent;
    if (k.keyCode === 229 || k.key === "Unidentified" || k.isComposing) {
      // The keyboard is editing its text; "input" will report the result.
      k.stopPropagation();
      return;
    }
    // A real key press that xterm sends itself. Start the field afresh, so
    // later differences don't count text that key changed on the remote side.
    if (k.key.length > 1 || k.ctrlKey || k.altKey || k.metaKey) queueMicrotask(reset);
  });
}

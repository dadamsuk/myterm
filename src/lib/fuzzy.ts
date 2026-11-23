/**
 * Small fuzzy matcher shared by the switcher, palette and folder picker.
 * Returns a score (higher is better) or null when `query` is not a
 * subsequence of `text`. Each whitespace-separated query word must match.
 */
export function fuzzyScore(query: string, text: string): number | null {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return 0;
  const t = text.toLowerCase();
  let total = 0;
  for (const w of words) {
    const s = scoreWord(w, t, text);
    if (s === null) return null;
    total += s;
  }
  return total;
}

function isBoundary(text: string, i: number): boolean {
  if (i === 0) return true;
  const prev = text[i - 1];
  const cur = text[i];
  if (/[\s/\\_\-.:]/.test(prev)) return true;
  return prev === prev.toLowerCase() && cur !== cur.toLowerCase();
}

function scoreWord(w: string, lower: string, orig: string): number | null {
  // Exact substring is best, with a bonus at a word boundary.
  const idx = lower.indexOf(w);
  if (idx >= 0) {
    return 100 + w.length * 10 + (isBoundary(orig, idx) ? 50 : 0) - idx * 0.1;
  }
  let score = 0;
  let ti = 0;
  let prev = -2;
  for (let qi = 0; qi < w.length; qi++) {
    const c = w[qi];
    let found = -1;
    // Prefer a boundary match ahead, else the next occurrence.
    for (let j = ti; j < lower.length; j++) {
      if (lower[j] === c && isBoundary(orig, j)) {
        found = j;
        break;
      }
    }
    const next = lower.indexOf(c, ti);
    if (next < 0) return null;
    if (found < 0 || found - next > 8) found = next;
    score += 1;
    if (found === prev + 1) score += 5;
    if (isBoundary(orig, found)) score += 8;
    score -= (found - ti) * 0.2;
    prev = found;
    ti = found + 1;
  }
  return score;
}

export function fuzzyFilter<T>(items: T[], query: string, text: (t: T) => string): T[] {
  if (!query.trim()) return items.slice();
  const scored: { item: T; score: number; i: number }[] = [];
  items.forEach((item, i) => {
    const s = fuzzyScore(query, text(item));
    if (s !== null) scored.push({ item, score: s, i });
  });
  scored.sort((a, b) => b.score - a.score || a.i - b.i);
  return scored.map((s) => s.item);
}

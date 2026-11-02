/**
 * Parse JSON with comments and trailing commas, the dialect VS Code uses for
 * `launch.json`, `tasks.json` and `settings.json`.
 */
export function parseJsonc(text: string): unknown {
  return JSON.parse(stripJsonc(text));
}

/** Replace comments with spaces and drop trailing commas, outside strings. */
export function stripJsonc(text: string): string {
  let out = '';
  let i = 0;
  const n = text.length;
  while (i < n) {
    const c = text[i];
    if (c === '"') {
      const start = i;
      i++;
      while (i < n && text[i] !== '"') {
        i += text[i] === '\\' ? 2 : 1;
      }
      i++;
      out += text.slice(start, i);
    } else if (c === '/' && text[i + 1] === '/') {
      while (i < n && text[i] !== '\n') {
        i++;
      }
    } else if (c === '/' && text[i + 1] === '*') {
      const end = text.indexOf('*/', i + 2);
      i = end < 0 ? n : end + 2;
      out += ' ';
    } else if (c === ',') {
      let j = i + 1;
      // Skip whitespace and comments to see what follows the comma.
      for (;;) {
        while (j < n && /\s/.test(text[j])) {
          j++;
        }
        if (text[j] === '/' && text[j + 1] === '/') {
          while (j < n && text[j] !== '\n') {
            j++;
          }
        } else if (text[j] === '/' && text[j + 1] === '*') {
          const end = text.indexOf('*/', j + 2);
          j = end < 0 ? n : end + 2;
        } else {
          break;
        }
      }
      if (text[j] !== '}' && text[j] !== ']') {
        out += c;
      }
      i++;
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

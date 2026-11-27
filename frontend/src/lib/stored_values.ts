/** Stored variable values as Settings shows and edits them: plain text plus a kind. */

export type ValueKind = 'text' | 'number' | 'yes/no' | 'list' | 'dictionary' | 'empty';

export function kindOf(v: unknown): ValueKind {
  if (v === null || v === undefined) return 'empty';
  if (typeof v === 'number') return 'number';
  if (typeof v === 'boolean') return 'yes/no';
  if (Array.isArray(v)) return 'list';
  if (typeof v === 'object') return 'dictionary';
  return 'text';
}

/** What goes in the edit box: text without quotes, lists and dictionaries as JSON. */
export function display(v: unknown): string {
  switch (kindOf(v)) {
    case 'empty':
      return '';
    case 'text':
      return String(v);
    case 'list':
    case 'dictionary':
      return JSON.stringify(v);
    default:
      return String(v);
  }
}

/** Reads the edit box back as a value of `kind`; throws with a message when it doesn't fit. */
export function parse(text: string, kind: ValueKind): unknown {
  const t = text.trim();
  switch (kind) {
    case 'text':
      return text;
    case 'empty':
      return null;
    case 'number': {
      const n = Number(t.replace(',', '.'));
      if (t === '' || !Number.isFinite(n)) throw new Error(`“${text}” isn't a number`);
      return n;
    }
    case 'yes/no': {
      if (/^(true|yes|on|1)$/i.test(t)) return true;
      if (/^(false|no|off|0)$/i.test(t)) return false;
      throw new Error('type yes or no');
    }
    case 'list':
    case 'dictionary': {
      let v: unknown;
      try {
        v = JSON.parse(t);
      } catch {
        throw new Error(kind === 'list' ? 'write a list like ["a", "b"]' : 'write a dictionary like {"key": "value"}');
      }
      if (kindOf(v) !== kind) throw new Error(kind === 'list' ? 'that is not a list' : 'that is not a dictionary');
      return v;
    }
  }
}

/** For new variables: a number or yes/no when it plainly is one, else text. */
export function guessKind(text: string): ValueKind {
  const t = text.trim();
  if (t !== '' && Number.isFinite(Number(t))) return 'number';
  if (/^(true|false)$/i.test(t)) return 'yes/no';
  if (t.startsWith('[')) return 'list';
  if (t.startsWith('{')) return 'dictionary';
  return 'text';
}

//! Block text (`block.toString()`, used by Problems, collapsed summaries, quick add and the
//! "triggered by" menu) built only from what the block shows: hidden fields and inputs, like
//! the numeric trigger's template while it watches the value, are left out.

import * as Blockly from 'blockly/core';

const COLLAPSED_INPUT = '_TEMP_COLLAPSED_INPUT';

function needsParens(c: Blockly.Connection): boolean {
  const checks = c.getCheck() ?? c.targetConnection?.getCheck() ?? null;
  return !!checks && (checks.includes('Boolean') || checks.includes('Number'));
}

/** Buttons and menus rather than content: the "+" of "if" (an image whose alt text is "Add
 * else if") and "＋ option" menus (dropdowns that save nothing). */
export function isControl(field: Blockly.Field): boolean {
  return field instanceof Blockly.FieldImage || (field instanceof Blockly.FieldDropdown && !field.isSerializable());
}

/** Blockly's own tokens, minus anything that isn't visible. */
export function visibleTokens(block: Blockly.Block, emptyToken = '?'): string[] {
  const tokens: string[] = [];
  for (const input of block.inputList) {
    if (input.name === COLLAPSED_INPUT || !input.isVisible()) continue;
    for (const field of input.fieldRow) {
      if (field.isVisible() && !isControl(field)) tokens.push(field.getText());
    }
    if (!input.connection) continue;
    const child = input.connection.targetBlock();
    if (!child) {
      tokens.push(emptyToken);
      continue;
    }
    const parens = needsParens(input.connection);
    if (parens) tokens.push('(');
    tokens.push(...visibleTokens(child, emptyToken));
    if (parens) tokens.push(')');
  }
  return tokens;
}

/** Same joining and shortening as Blockly's `toString`. */
export function blockText(block: Blockly.Block, maxLength?: number, emptyToken?: string): string {
  const tokens = visibleTokens(block, emptyToken);
  for (let i = 2; i < tokens.length; i++) {
    if (tokens[i - 2] === '(' && tokens[i] === ')') {
      tokens[i - 2] = tokens[i - 1];
      tokens.splice(i - 1, 2);
    }
  }
  let prev = '';
  let text = tokens.reduce((acc, cur) => {
    const joined = acc + (prev === '(' || cur === ')' ? '' : ' ') + cur;
    prev = cur[cur.length - 1];
    return joined;
  }, '');
  text = text.trim() || '???';
  if (maxLength && text.length > maxLength) text = text.substring(0, maxLength - 3) + '...';
  return text;
}

let installed = false;
/** Makes every `toString` use `blockText`; runs before anything wraps `toString`. */
export function installBlockText() {
  if (installed) return;
  installed = true;
  Blockly.Block.prototype.toString = function (this: Blockly.Block, maxLength?: number, emptyToken?: string) {
    return blockText(this, maxLength, emptyToken);
  };
}

installBlockText();

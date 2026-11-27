//! Number fields that take a decimal comma ("21,5"), as typed in most of Europe.

import * as Blockly from 'blockly/core';

/**
 * The text with one comma read as the decimal point. Null for text where a comma could be a
 * thousands separator as well ("1,234,5", "1.234,5"): guessing would set the wrong temperature.
 */
export function decimalComma(text: string): string | null {
  const commas = text.split(',').length - 1;
  if (!commas) return text;
  if (commas > 1 || text.includes('.')) return null;
  return text.replace(',', '.');
}

let installed = false;
export function installDecimalComma() {
  if (installed) return;
  installed = true;
  // Blockly drops every comma, which turns "1,5" into 15.
  const proto = Blockly.FieldNumber.prototype as unknown as { doClassValidation_: (v?: unknown) => number | null };
  const original = proto.doClassValidation_;
  proto.doClassValidation_ = function (this: Blockly.FieldNumber, v?: unknown) {
    if (typeof v === 'string') {
      const text = decimalComma(v);
      if (text === null) return null;
      v = text;
    }
    return original.call(this, v);
  };
}

import { describe, expect, it } from 'vitest';
import { display, guessKind, kindOf, parse } from './stored_values';

describe('stored values', () => {
  it('show text without quotes and keep their kind', () => {
    expect(display('on')).toBe('on');
    expect(kindOf('on')).toBe('text');
    expect(display(21.5)).toBe('21.5');
    expect(display(['a', 1])).toBe('["a",1]');
    expect(kindOf(null)).toBe('empty');
    expect(parse('on', 'text')).toBe('on');
    expect(parse('42', 'text')).toBe('42');
  });

  it('read numbers, yes/no and JSON back, or say what is wrong', () => {
    expect(parse('21,5', 'number')).toBe(21.5);
    expect(() => parse('warm', 'number')).toThrow("isn't a number");
    expect(parse('Yes', 'yes/no')).toBe(true);
    expect(parse('off', 'yes/no')).toBe(false);
    expect(parse('["a"]', 'list')).toEqual(['a']);
    expect(() => parse('{"a": 1}', 'list')).toThrow('not a list');
    expect(() => parse('a, b', 'list')).toThrow('like ["a", "b"]');
  });

  it('guess the kind of new values', () => {
    expect(guessKind('5')).toBe('number');
    expect(guessKind('true')).toBe('yes/no');
    expect(guessKind('on')).toBe('text');
    expect(guessKind('')).toBe('text');
  });
});

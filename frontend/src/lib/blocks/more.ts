//! Lists changed in place, running other programs and a slow fade for wake-up lights.
//! Compiled in crates/puzzlebox-ir/src/compile_more.rs, run by the engine.

import * as Blockly from 'blockly/core';
import { getJson } from '../api';
import { FieldSuggest, type Suggestion } from './field_entity';

type Json = Record<string, unknown>;
const value = (name: string, check?: string) => ({ type: 'input_value', name, ...(check ? { check } : {}) });
const variable = (name: string, v = 'list') => ({ type: 'field_variable', name, variable: v });

const stmt = (type: string, message0: string, args0: Json[], tooltip: string, style: string): Json => ({
  type,
  message0,
  args0,
  previousStatement: null,
  nextStatement: null,
  style,
  tooltip,
  inputsInline: true,
});

interface ProgramInfo {
  id: string;
  name: string;
  enabled: boolean;
  folder?: string;
}

let programs: ProgramInfo[] = [];
let fetchedAt = 0;
let inflight: Promise<void> | null = null;

/** The program list for program pickers, refreshed at most every few seconds. */
export function loadPrograms(force = false): Promise<void> {
  if (!force && Date.now() - fetchedAt < 5_000) return Promise.resolve();
  inflight ??= getJson<ProgramInfo[]>('api/programs')
    .then((list) => {
      programs = list.map(({ id, name, enabled, folder }) => ({ id, name, enabled, folder }));
      fetchedAt = Date.now();
    })
    .catch(() => {})
    .finally(() => (inflight = null));
  return inflight;
}

export function setProgramsForTest(list: ProgramInfo[]) {
  programs = list;
  fetchedAt = Date.now();
}

/** Picks a program by name; stores its id, so renaming the program doesn't break the block. */
export class FieldProgram extends FieldSuggest {
  static fromJson(options: Blockly.FieldTextInputFromJsonConfig) {
    return new FieldProgram(options.text ?? '');
  }

  protected override load() {
    return loadPrograms();
  }

  protected suggestions(q: string): Suggestion[] {
    return programs
      .filter((p) => !q || p.name.toLowerCase().includes(q))
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((p) => ({ value: p.id, label: p.name, meta: [p.folder, p.enabled ? '' : 'turned off'].filter(Boolean).join(' · ') }));
  }

  protected emptyText() {
    return 'No programs yet';
  }

  protected override doClassValidation_(v?: unknown): string | null {
    const text = String(v ?? '').trim();
    // A typed name becomes that program's id.
    const named = programs.find((p) => p.name.toLowerCase() === text.toLowerCase());
    return named ? named.id : text;
  }

  protected override getText_(): string | null {
    const v = String(this.getValue() ?? '');
    if (!v) return 'choose…';
    const p = programs.find((x) => x.id === v);
    if (p) return p.name;
    return fetchedAt ? 'a deleted program' : '…';
  }
}

const RUN_HELP =
  'Runs its "when run" blocks (or, without them, its first "when"), as its Run button does. The other program has to be turned on; its run mode decides what happens if it is already running. Programs can start each other at most 10 deep, so a program that starts itself can\'t loop forever.';

export const MORE_BLOCKS: Json[] = [
  stmt(
    'lists_add',
    'add %1 to list %2',
    [value('ITEM'), variable('VAR')],
    'Adds the item at the end of the list in the variable. An empty variable becomes a list with just this item.',
    'list_blocks',
  ),
  stmt(
    'lists_remove_value',
    'remove %1 from list %2',
    [value('ITEM'), variable('VAR')],
    'Takes every copy of the item out of the list in the variable (numbers and text that read the same count as equal, "5" and 5, like "find"). Nothing happens if it isn\'t in the list.',
    'list_blocks',
  ),
  {
    type: 'lists_contains',
    message0: 'list %1 contains %2',
    args0: [value('LIST'), value('ITEM')],
    output: 'Boolean',
    style: 'list_blocks',
    inputsInline: true,
    tooltip: 'True when the item is in the list (numbers and text that read the same count as equal, "5" and 5). An empty value counts as an empty list.',
  },
  stmt('program_run', 'run program %1', [{ type: 'field_program', name: 'PROGRAM', text: '' }], `Starts another program and carries on at once. ${RUN_HELP}`, 'flow_blocks'),
  stmt(
    'program_run_wait',
    'run program %1 and wait until it finishes',
    [{ type: 'field_program', name: 'PROGRAM', text: '' }],
    `Runs another program and waits for it. If it fails, this run fails too ("try" catches it); if it is already running and set to ignore new runs, this fails at once. ${RUN_HELP}`,
    'flow_blocks',
  ),
  stmt(
    'light_fade',
    'fade %1 from %2 % to %3 % over %4',
    [value('TARGET'), value('FROM', 'Number'), value('TO', 'Number'), value('SECONDS', 'Number')],
    'Changes the brightness a little at a time, for wake-up lights and slow sunsets: about every 1 % of the way, at most every few seconds, each step a smooth transition. Stops early when someone changes or turns off the light themselves, and when the run is stopped. The blocks below run after the fade.',
    'colour_blocks',
  ),
];

let registered = false;
export function registerMoreBlocks() {
  if (registered) return;
  registered = true;
  Blockly.fieldRegistry.register('field_program', FieldProgram);
  Blockly.common.defineBlocksWithJsonArray(MORE_BLOCKS);
}

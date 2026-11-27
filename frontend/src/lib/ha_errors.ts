//! Home Assistant's common errors in plain words; the original text stays available as details.

const KEY = String.raw`(?:dictionary value |value )?@ data\['([^'\]]+)'\]`;

const RULES: [RegExp, (m: RegExpMatchArray) => string][] = [
  [
    /at least one of \[[^\]]*'entity_id'[^\]]*\] is required/,
    () => 'Nothing to act on: choose an entity, area, device, floor or label for this action.',
  ],
  [
    /(?:Service|Action) ([\w.]+) not found|called (?:service|action) ([\w.]+) which was not found/,
    (m) => `Home Assistant has no action ${m[1] ?? m[2]}. Is its integration installed and working?`,
  ],
  [
    /unknown entity ([\w.]+)|Invalid entity id encountered: ([\w.]+)|[Ee]ntity ([\w.]+) (?:is unknown|not found|does not exist)|Referenced entities ([\w., ]+?) (?:are|is) missing/,
    (m) => `Home Assistant doesn't know ${m[1] ?? m[2] ?? m[3] ?? m[4]}. Was it renamed or removed?`,
  ],
  [
    new RegExp(String.raw`value must be at (most|least) (-?[\d.]+) for ${KEY}`),
    (m) => `"${m[3]}" can be at ${m[1]} ${m[2]}.`,
  ],
  [
    new RegExp(String.raw`(?:value must be one of|not a valid value) .*?for ${KEY}`),
    (m) => `"${m[1]}" has a value Home Assistant doesn't accept here.`,
  ],
  [
    new RegExp(String.raw`expected (?:float|int|a number|Number)\w* for ${KEY}`),
    (m) => `"${m[1]}" needs a number.`,
  ],
  [
    /The scene scene\.(\w+) already exists/,
    (m) => `There's already a scene called scene.${m[1]} in your configuration: choose another name to remember the state as.`,
  ],
  [/out of range|outside (?:of )?(?:the )?(?:valid )?range/i, () => 'A value is outside what this device accepts.'],
];

/** Plain words for a Home Assistant error, or null when it isn't a common one. */
export function plainError(message: string | null | undefined): string | null {
  if (!message) return null;
  for (const [re, say] of RULES) {
    const m = message.match(re);
    if (m) return say(m);
  }
  return null;
}

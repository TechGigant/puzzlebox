// Serve Blockly's sprites and cursors from the app instead of static.blockly.com,
// so the editor works offline and makes no third-party requests.
import { cpSync, mkdirSync } from 'node:fs';

mkdirSync('public', { recursive: true });
cpSync('node_modules/blockly/media', 'public/blockly-media', { recursive: true });

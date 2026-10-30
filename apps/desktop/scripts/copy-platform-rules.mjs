import { copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const src = join(here, '../../../config/platform_rules');
const dest = join(here, '../src/assets/platform-rules');
mkdirSync(dest, { recursive: true });
for (const name of ['twitch.txt', 'youtube.txt', 'kick.txt']) {
  copyFileSync(join(src, name), join(dest, name));
}

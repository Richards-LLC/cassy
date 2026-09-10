import { existsSync, readdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

export async function launchBrowser() {
  let playwright;
  try { playwright = await import('playwright'); }
  catch {
    const cache = join(homedir(), '.npm/_npx');
    const installed = existsSync(cache) ? readdirSync(cache).map(p => join(cache,p,'node_modules/playwright')).filter(p => existsSync(join(p,'index.mjs'))) : [];
    const path = process.env.PLAYWRIGHT_MODULE || installed.at(-1);
    if (!path) throw new Error('Install Playwright or set PLAYWRIGHT_MODULE to its package directory.');
    playwright = await import(pathToFileURL(join(path,'index.mjs')).href);
  }
  const executablePath = process.env.CHROMIUM_PATH || (existsSync('/usr/bin/google-chrome') ? '/usr/bin/google-chrome' : undefined);
  return playwright.chromium.launch({headless:true, executablePath});
}

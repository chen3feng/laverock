#!/usr/bin/env node
// Drives the real UI in headless Chromium to check that starting one clip
// stops whatever was already playing: the native players on the History
// page and the shared previewer on the takes page must not overlap.
//
//   npm install -g playwright         # once; Chromium comes with it
//   node scripts/check-web-audio.mjs [http://127.0.0.1:8787]
//
// The server must be running and have at least one history entry and one
// recorded take, otherwise the check is skipped.
//
// Exit codes: 0 pass, 1 failure, 2 skipped (no server or no data).

import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import path from 'node:path';

const require = createRequire(import.meta.url);

function loadPlaywright() {
  try {
    return require('playwright');
  } catch {
    const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm';
    const root = spawnSync(npm, ['root', '-g'], { encoding: 'utf8' });
    if (root.status !== 0) {
      throw new Error('playwright is not installed; run: npm install -g playwright');
    }
    return require(path.join(root.stdout.trim(), 'playwright'));
  }
}

const base = (process.argv[2] || process.env.LAVEROCK_URL || 'http://127.0.0.1:8787').replace(/\/+$/, '');
const { chromium } = loadPlaywright();

const browser = await chromium.launch({
  args: ['--autoplay-policy=no-user-gesture-required', '--mute-audio'],
});
const page = await browser.newPage();
const pageErrors = [];
page.on('pageerror', (e) => pageErrors.push(String(e)));

const waitPlaying = (selector, timeout = 5000) => page.waitForFunction(
  (sel) => {
    const audio = document.querySelector(sel);
    return Boolean(audio) && !audio.paused;
  },
  selector,
  { timeout },
);

let exitCode = 1;
try {
  const health = await fetch(`${base}/api/health`).catch(() => null);
  if (!health?.ok) {
    console.log(`SKIP: no server at ${base}`);
    exitCode = 2;
  } else {
    await page.goto(`${base}/#/history`, { waitUntil: 'domcontentloaded' });
    const hasHistory = await page
      .waitForSelector('#historyList audio', { timeout: 10000 })
      .then(() => true)
      .catch(() => false);
    if (!hasHistory) {
      console.log('SKIP: the history page has no entries');
      exitCode = 2;
    } else {
      // 1. History playing, then a take preview: history must stop.
      const historyAudio = page.locator('#historyList audio').first();
      await historyAudio.evaluate((el) => el.play());
      await waitPlaying('#historyList audio');
      console.log('history playing:', await historyAudio.evaluate((el) => !el.paused));

      await page.click('nav a[data-page="captures"]');
      const hasTakes = await page
        .waitForSelector('.capture-card .play', { timeout: 10000 })
        .then(() => true)
        .catch(() => false);
      if (!hasTakes) {
        console.log('SKIP: the takes page has no recordings');
        exitCode = 2;
      } else {
        await page.locator('.capture-card .play').first().click();
        await waitPlaying('#previewer');
        const historyStopped = await historyAudio.evaluate((el) => el.paused);
        console.log('take preview started; history paused:', historyStopped);

        // 2. Preview playing, then history again: preview must stop.
        await page.click('nav a[data-page="history"]');
        await page.waitForSelector('#historyList audio', { timeout: 10000 });
        await page.locator('#historyList audio').first().evaluate((el) => el.play());
        await waitPlaying('#historyList audio');
        await page.waitForFunction(
          () => document.getElementById('previewer').paused,
          null,
          { timeout: 5000 },
        );
        const previewStopped = await page.evaluate(() => document.getElementById('previewer').paused);
        console.log('history started again; preview paused:', previewStopped);

        const passed = historyStopped && previewStopped;
        console.log('page errors:', pageErrors.length ? pageErrors.join('; ') : 'none');
        console.log(passed ? 'PASS: only one clip plays at a time' : 'FAIL: two clips played together');
        exitCode = passed ? 0 : 1;
      }
    }
  }
} catch (e) {
  console.log('FAIL:', e.message);
  exitCode = 1;
} finally {
  await browser.close();
}
process.exit(exitCode);

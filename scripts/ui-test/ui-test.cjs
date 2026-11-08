#!/usr/bin/env node
// Browser-level smoke test of the Laverock web UI.
//
// The API chain is covered by scripts/e2e-test.sh; this walks the same
// chain through the real single-page app in Chromium:
//
//   choose engine -> upload a reference -> transcribe -> synthesize
//   -> history entry and download
//   -> models grouped by task, installed model opens its workspace
//   -> capture upload -> transcribe -> save
//   -> voice upload -> transcribe -> save -> export -> import
//
// Requires a running server with ZipVoice and SenseVoice installed (the
// same models the CI smoke job downloads). Usage:
//
//   node scripts/ui-test.cjs [--base URL] [--ref reference.wav]
//
// Env: LAVEROCK_URL, LAVEROCK_REF, LAVEROCK_UI_ARTIFACTS (downloads and a
// failure screenshot land there).
'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

// CI installs playwright into scripts/ui-test/node_modules from
// package.json; a local run can use the global install instead.
const { chromium } = (() => {
  try {
    return require('playwright');
  } catch (_) {
    const { spawnSync } = require('node:child_process');
    const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm';
    const root = spawnSync(npm, ['root', '-g'], { encoding: 'utf8' });
    if (root.status === 0) {
      try {
        return require(path.join(root.stdout.trim(), 'playwright'));
      } catch (_) { /* fall through */ }
    }
    throw new Error('playwright is not installed; run: npm install -g playwright');
  }
})();

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  return i !== -1 && process.argv[i + 1] ? process.argv[i + 1] : fallback;
}

const base = (arg('base', process.env.LAVEROCK_URL || 'http://127.0.0.1:8787')).replace(/\/+$/, '');
const root = path.resolve(__dirname, '..', '..');
const zipvoice = 'sherpa-onnx-zipvoice-distill-int8-zh-en-emilia';
const refCandidates = [
  path.join(root, 'models', zipvoice, 'test_wavs', 'leijun-1.wav'),
  path.join(os.homedir(), 'Library', 'Application Support', 'sh.laverock.desktop', 'models', zipvoice, 'test_wavs', 'leijun-1.wav'),
  path.join(process.env.APPDATA || path.join(os.homedir(), 'AppData', 'Roaming'), 'sh.laverock.desktop', 'models', zipvoice, 'test_wavs', 'leijun-1.wav'),
  path.join(os.homedir(), '.local', 'share', 'sh.laverock.desktop', 'models', zipvoice, 'test_wavs', 'leijun-1.wav'),
];
const ref = path.resolve(
  arg('ref', process.env.LAVEROCK_REF || refCandidates.find((p) => fs.existsSync(p)) || refCandidates[0]),
);
const artifacts = fs.mkdtempSync(path.join(os.tmpdir(), 'laverock-ui-'));
const synthText = '浏览器自动化测试：检查合成、历史下载和声音导入导出。';
const voiceName = `ui-test-${Date.now()}`;

let step = 0;
function pass(message) {
  step += 1;
  console.log(`  ok   ${message}`);
}
function say(message) {
  console.log(`\n${message}`);
}

async function apiJson(request, url, init) {
  const res = await request.fetch(url, init);
  assert.ok(res.ok(), `${init?.method || 'GET'} ${url} -> HTTP ${res.status()}`);
  return res.json();
}

async function saveDownload(page, locator, dir) {
  const [download] = await Promise.all([
    page.waitForEvent('download', { timeout: 60_000 }),
    locator.click(),
  ]);
  const file = path.join(dir, download.suggestedFilename());
  await download.saveAs(file);
  return file;
}

async function transcribe(page, hostSelector) {
  const button = page.locator(`${hostSelector} .cap-transcribe`);
  await button.waitFor({ state: 'visible', timeout: 30_000 });
  const [response] = await Promise.all([
    page.waitForResponse(
      (r) => r.url().endsWith('/api/transcribe') && r.request().method() === 'POST',
      { timeout: 120_000 },
    ),
    button.click(),
  ]);
  assert.ok(response.ok(), `transcribe -> HTTP ${response.status()}`);
  return response.json().then((body) => body.text || '');
}

async function main() {
  if (!fs.existsSync(ref)) {
    throw new Error(`reference clip not found: ${ref}`);
  }
  const health = await fetch(`${base}/api/health`).then((r) => r.json());
  assert.equal(health.status, 'ok', 'server health');
  assert.equal(health.transcribe, true, 'SenseVoice must be installed for this test');

  const browser = await chromium.launch();
  const context = await browser.newContext({ acceptDownloads: true });
  // The page asks for a native save dialog when the File System Access API
  // exists; headless Chromium cannot answer it, so use the anchor fallback.
  await context.addInitScript(() => {
    try { delete window.showSaveFilePicker; } catch (_) { /* ignore */ }

    // Stand-in for Web Audio: streaming playback only needs to decode and
    // schedule chunks, so count them instead of producing sound in CI.
    window.__streamChunks = 0;
    class FakeAudioContext {
      constructor() { this.currentTime = 0; this.state = 'running'; this.destination = {}; }
      resume() { return Promise.resolve(); }
      decodeAudioData() {
        window.__streamChunks += 1;
        return Promise.resolve({ duration: 1 });
      }
      createBufferSource() {
        return { buffer: null, connect() {}, start() {}, stop() {}, onended: null };
      }
      close() { return Promise.resolve(); }
    }
    window.AudioContext = FakeAudioContext;
    window.webkitAudioContext = FakeAudioContext;
  });
  const page = await context.newPage();
  const pageErrors = [];
  page.on('pageerror', (err) => pageErrors.push(String(err)));

  try {
    // ------------------------------------------------------------ synthesize
    say('synthesize');
    await page.goto(`${base}/#/synthesize`, { waitUntil: 'domcontentloaded' });
    await page.waitForFunction(
      () => Array.from(document.querySelectorAll('#engine option'))
        .some((o) => o.value.includes('zipvoice')),
      null,
      { timeout: 60_000 },
    );
    const engineId = await page.$eval('#engine', (select) => {
      const option = Array.from(select.options).find((o) => o.value.includes('zipvoice'));
      return option ? option.value : '';
    });
    await page.selectOption('#engine', engineId);
    pass(`engine selected (${engineId})`);

    await page.setInputFiles('#synthCapture .cap-file', ref);
    const promptText = await transcribe(page, '#synthCapture');
    await page.waitForFunction(
      () => (document.getElementById('prompt_text')?.value || '').trim().length > 0,
      null,
      { timeout: 30_000 },
    );
    assert.ok(promptText.length >= 5, 'transcript is not empty');
    pass(`reference transcribed (${promptText.length} chars)`);

    await page.fill('#text', synthText);
    await page.click('#go');
    await page.waitForSelector('#status.ok', { timeout: 180_000 });
    await page.waitForSelector('#synthResults:not([hidden]) audio', { timeout: 30_000 });
    const results = await page.locator('#synthResults audio').count();
    assert.ok(results >= 1, 'a take was rendered');
    pass(`synthesis finished (${results} take)`);

    const synthWav = await saveDownload(page, page.locator('#synthResults a.hint').first(), artifacts);
    assert.ok(fs.statSync(synthWav).size > 44, 'downloaded WAV has a header');
    fs.accessSync(synthWav);
    pass(`result downloaded (${path.basename(synthWav)})`);

    // ------------------------------------------------- streamed synthesis
    say('streaming synthesis');
    const streamText =
      '你好，我是 Laverock 的流式合成演示。这是一段比较长的文字，用来验证第一句生成后就能开始播放，'
      + '而不是等整段完成。流式合成会按句子切分文本，每生成一段就立刻发送一个音频帧，浏览器收到后'
      + '按顺序排入播放队列。这样长文本的首字节延迟就缩短到了第一句的生成时间。';
    await page.fill('#text', streamText);
    await page.check('#streamAudio');
    // Clear the previous run's status so waiting for .ok cannot match it.
    await page.evaluate(() => {
      document.getElementById('status').className = '';
      document.getElementById('status').textContent = '';
    });
    await page.click('#go');
    await page.waitForSelector('#status.ok', { timeout: 300_000 });
    const chunks = await page.evaluate(() => window.__streamChunks);
    assert.ok(chunks >= 2, `expected streamed audio chunks, got ${chunks}`);
    pass(`streamed audio (${chunks} chunks)`);

    // --------------------------------------------------------------- history
    say('history');
    await page.click('nav a[data-page="history"]');
    await page.waitForSelector('#page-history.active');
    await page.waitForFunction(
      (text) => Array.from(document.querySelectorAll('#historyList .hist-text'))
        .some((el) => el.textContent === text),
      synthText,
      { timeout: 30_000 },
    );
    pass('history entry recorded');

    const histCard = page.locator('#historyList .card', { hasText: synthText }).first();
    const histWav = await saveDownload(page, histCard.locator('.hist-row a.hint').first(), artifacts);
    assert.ok(fs.statSync(histWav).size > 44, 'history WAV has a header');
    assert.ok(histWav.endsWith('.wav'), 'history download is a WAV');
    pass(`history download (${path.basename(histWav)})`);

    // ---------------------------------------------------------------- models
    say('models');
    await page.click('nav a[data-page="models"]');
    await page.waitForSelector('#page-models.active');
    await page.waitForSelector('#modelGroups .model-card');
    const groupTitles = await page.$$eval('#modelGroups .section-title', (els) =>
      els.map((el) => el.textContent.trim()));
    assert.ok(groupTitles.length >= 2, `models are grouped by task: ${groupTitles}`);
    pass(`model groups rendered (${groupTitles.join(', ')})`);

    const sttCard = page.locator('.model-card', { hasText: 'SenseVoice' }).first();
    assert.equal(
      await sttCard.locator('.model-actions button').count(), 1,
      'a transcription model has no workspace yet, so only Delete is offered',
    );

    const zipCard = page.locator('.model-card', { hasText: 'ZipVoice' }).first();
    const unloadBtn = zipCard.locator('.model-actions button.ghost.small:not(.danger)').first();
    assert.equal(await unloadBtn.count(), 1, 'a loaded model offers Unload');
    await unloadBtn.click();
    await unloadBtn.waitFor({ state: 'detached', timeout: 30_000 });
    const zipLoaded = await page.evaluate(async (id) => {
      const list = await (await fetch('/api/engines')).json();
      return list.find((e) => e.id === id)?.loaded;
    }, engineId);
    assert.equal(zipLoaded, false, 'engine is unloaded after the button click');
    pass('loaded model unloaded from the models page');

    await zipCard.locator('.model-actions button').first().click();
    await page.waitForFunction(() => location.hash === '#/synthesize');
    assert.equal(await page.$eval('#engine', (select) => select.value), engineId);
    pass('installed model opens its workspace with the model selected');

    // -------------------------------------------------------------- captures
    say('captures');
    await page.click('nav a[data-page="captures"]');
    await page.waitForSelector('#page-captures.active');
    // A library may already hold takes (a saved voice's clip is one too), so
    // pick the card this step creates rather than the first one on screen.
    const takesBefore = new Set(
      (await apiJson(context.request, `${base}/api/captures`)).map((c) => c.id),
    );
    await page.click('#newCaptureBtn');
    await page.setInputFiles('#libraryCapture .cap-file', ref);
    await transcribe(page, '#libraryCapture');
    await page.click('#newCaptureSave');
    await page.waitForFunction(
      (known) => [...document.querySelectorAll('.capture-card')]
        .some((card) => card.dataset.id && !known.includes(card.dataset.id)),
      [...takesBefore],
      { timeout: 60_000 },
    );
    const takeId = await page.evaluate(
      (known) => [...document.querySelectorAll('.capture-card')]
        .map((card) => card.dataset.id)
        .find((id) => id && !known.includes(id)),
      [...takesBefore],
    );
    const capture = page.locator(`.capture-card[data-id="${takeId}"]`);
    await capture.waitFor({ timeout: 30_000 });
    pass('capture saved with its transcript');

    // Re-transcribing stores timed cues; the card shows them and offers
    // SRT/VTT downloads.
    await capture.locator('.tr').click();
    await capture.locator('.voice-seg').first().waitFor({ timeout: 120_000 });
    const cues = await capture.locator('.voice-seg').count();
    assert.ok(cues >= 1, 'timed cues rendered');
    pass(`capture shows ${cues} timed cue(s)`);
    const srtFile = await saveDownload(page, capture.locator('.sub-srt'), artifacts);
    const srtBody = fs.readFileSync(srtFile, 'utf8');
    assert.ok(srtBody.includes(' --> '), 'SRT contains cue timings');
    pass(`subtitle export (${path.basename(srtFile)})`);

    // ----------------------------------------------------------------- tools
    say('audio tools');
    await page.click('nav a[data-page="tools"]');
    await page.waitForSelector('#page-tools.active');
    const tools = await apiJson(context.request, `${base}/api/tools`);
    if (!tools.denoise && !tools.separation) {
      // CI installs the small denoiser; a machine without any pack still
      // has to explain how to get one instead of failing the run.
      await page.waitForFunction(
        () => (document.getElementById('toolModelsHint')?.textContent || '').trim().length > 0,
        null,
        { timeout: 10_000 },
      );
      assert.equal(await page.locator('#toolRun').isDisabled(), true, 'run is disabled');
      pass('tools page explains the missing models');
    } else {
      const mode = tools.denoise ? 'denoise' : 'vocals';
      // loadToolsPage() fetches availability asynchronously; wait for the
      // chosen mode to be enabled before selecting it.
      await page.waitForFunction(
        (value) => {
          const option = document.querySelector(`#toolMode option[value="${value}"]`);
          return option && !option.disabled;
        },
        mode,
        { timeout: 10_000 },
      );
      await page.selectOption('#toolMode', mode);
      await page.setInputFiles('#toolsCapture .cap-file', ref);
      await page.click('#toolRun');
      await page.waitForSelector('#toolResult:not([hidden])', { timeout: 300_000 });
      const toolWav = await saveDownload(
        page,
        page.locator('#toolDownloads button', { hasText: 'WAV' }).first(),
        artifacts,
      );
      assert.ok(fs.statSync(toolWav).size > 44, 'tool output is not empty');
      assert.equal(fs.readFileSync(toolWav).subarray(0, 4).toString(), 'RIFF');
      pass(`tool output downloaded (${path.basename(toolWav)})`);

      if (tools.separation) {
        // `both` renders one player per stem and bundles them in a ZIP.
        await page.selectOption('#toolMode', 'both');
        await page.click('#toolRun');
        await page.waitForFunction(
          () => document.querySelectorAll('#toolAfterPlayers audio').length === 2,
          null,
          { timeout: 300_000 },
        );
        const stemsZip = await saveDownload(
          page,
          page.locator('#toolDownloads button', { hasText: 'ZIP' }).first(),
          artifacts,
        );
        assert.equal(fs.readFileSync(stemsZip).subarray(0, 2).toString(), 'PK');
        pass(`both stems downloaded (${path.basename(stemsZip)})`);
      }
    }

    // --------------------------------------------------------------- projects
    say('subtitle projects');
    await page.click('nav a[data-page="projects"]');
    await page.waitForSelector('#page-projects.active');
    const projectsBefore = new Set(
      (await apiJson(context.request, `${base}/api/projects`)).map((p) => p.id),
    );
    await page.click('#newProjectBtn');
    await page.setInputFiles('#projectCapture .cap-file', ref);
    await page.fill('#projectName', 'UI subtitle test');
    await page.click('#projectCreate');
    // Creating transcribes automatically when the model is installed; the
    // editor opens either way.
    await page.waitForSelector('#projectEditor:not([hidden])', { timeout: 240_000 });
    if (await page.locator('#projectSegments .seg-row').count() === 0) {
      await page.click('#projectAddSegment');
      await page.fill('#projectSegments .seg-row .seg-start', '0');
      await page.fill('#projectSegments .seg-row .seg-end', '1.5');
    }
    await page.fill('#projectSegments .seg-row .seg-text', 'edited cue');
    const [projectSaved] = await Promise.all([
      page.waitForResponse(
        (r) => /\/api\/projects\/[^/]+$/.test(r.url()) && r.request().method() === 'PATCH',
        { timeout: 60_000 },
      ),
      page.click('#projectSave'),
    ]);
    assert.ok(projectSaved.ok(), `save cues -> HTTP ${projectSaved.status()}`);
    pass('project cues saved');

    const projectSrt = await saveDownload(page, page.locator('#projectSrt'), artifacts);
    const projectSrtBody = fs.readFileSync(projectSrt, 'utf8');
    assert.ok(
      projectSrtBody.includes('edited cue') && projectSrtBody.includes(' --> '),
      'project SRT contains the edited cue',
    );
    pass(`project subtitle export (${path.basename(projectSrt)})`);

    const projectId = (await apiJson(context.request, `${base}/api/projects`))
      .map((p) => p.id)
      .find((id) => !projectsBefore.has(id));
    assert.ok(projectId, 'project created');
    await context.request.delete(`${base}/api/projects/${projectId}`);

    // ---------------------------------------------------------------- voices
    say('voices');
    await page.click('nav a[data-page="voices"]');
    await page.waitForSelector('#page-voices.active');
    await page.click('#newVoiceBtn');
    await page.setInputFiles('#voiceCapture .cap-file', ref);
    await transcribe(page, '#voiceCapture');
    await page.fill('#newName', voiceName);
    await page.waitForFunction(
      () => (document.getElementById('newPromptText')?.value || '').trim().length > 0,
      null,
      { timeout: 30_000 },
    );
    await page.click('#newSave');
    await page.waitForFunction(
      (name) => Array.from(document.querySelectorAll('#voicesGrid .voice-card'))
        .some((el) => el.textContent.includes(name)),
      voiceName,
      { timeout: 30_000 },
    );
    pass(`voice saved (${voiceName})`);

    // --------------------------------------------------------- export/import
    say('export -> import');
    await page.click('#exportVoicesBtn');
    await page.waitForSelector('#exportDialog.show');
    const exported = await saveDownload(page, page.locator('#exportOk'), artifacts);
    assert.ok(fs.statSync(exported).size > 22, 'voice package is not empty');
    assert.equal(fs.readFileSync(exported).subarray(0, 2).toString(), 'PK', 'voice package is a zip');
    pass(`voices exported (${path.basename(exported)})`);

    const before = await apiJson(context.request, `${base}/api/profiles`);
    await page.setInputFiles('#importVoiceFile', exported);
    await page.waitForSelector('#importDialog.show', { timeout: 30_000 });
    const [importResponse] = await Promise.all([
      page.waitForResponse(
        (r) => r.url().endsWith('/api/profiles/import') && r.request().method() === 'POST',
        { timeout: 60_000 },
      ),
      page.click('#importCopy'),
    ]);
    assert.ok(importResponse.ok(), `import -> HTTP ${importResponse.status()}`);
    const report = await importResponse.json();
    assert.ok(report.imported >= 1, `import reported ${JSON.stringify(report)}`);
    const after = await apiJson(context.request, `${base}/api/profiles`);
    assert.equal(after.length, before.length + report.imported);
    pass(`voices imported (${report.imported})`);

    assert.deepEqual(pageErrors, [], 'uncaught page errors');
    say(`all checks passed (${step})`);
  } catch (err) {
    await page.screenshot({ path: path.join(artifacts, 'failure.png'), fullPage: true }).catch(() => {});
    console.error(`\nFAILED: ${err.message}`);
    if (pageErrors.length) console.error(`page errors:\n${pageErrors.join('\n')}`);
    console.error(`artifacts: ${artifacts}`);
    process.exitCode = 1;
  } finally {
    await browser.close();
  }
}

main().catch((err) => {
  console.error(err.stack || err);
  process.exit(1);
});

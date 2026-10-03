// Optional browser acceptance test. Install Playwright outside the production runner.
// NODE_PATH=/path/to/node_modules WDESK_SESSION=default node tests/viewer-smoke.cjs
const { chromium } = require('playwright');
const { execFileSync } = require('node:child_process');
const assert = require('node:assert/strict');
const path = require('node:path');
const bin = process.env.WDESK_BIN || path.resolve('target/debug/wdesk');
const session = process.env.WDESK_SESSION || 'default';
function w(...args) { return JSON.parse(execFileSync(bin, ['--session', session, '--json', ...args], {encoding:'utf8'})); }
(async () => {
  w('import', path.resolve('tests/input-probe.ps1'), 'workspace/input-probe.ps1');
  w('launch', '--', 'powershell.exe', '-NoProfile', '-STA', '-ExecutionPolicy', 'Bypass', '-WindowStyle', 'Hidden', '-File', 'C:\\ProgramData\\wdesk\\workspace\\input-probe.ps1');
  const windowDeadline = Date.now() + 30000;
  let win;
  do {
    win = w('windows').windows.find(x => x.title === 'wdesk input probe');
    if (!win) await new Promise(resolve => setTimeout(resolve, 500));
  } while (!win && Date.now() < windowDeadline);
  assert.ok(win, 'Inbox input probe window exists');
  w('windows', '--focus', win.id);
  const url = w('view');
  const browser = await chromium.launch({headless:true, args:['--no-sandbox']});
  try {
    const context = await browser.newContext({permissions:['clipboard-read', 'clipboard-write']});
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    await page.goto(url);
    await page.waitForFunction(() => /input \d+/.test(document.querySelector('#status').textContent));
    assert.equal(new URL(page.url()).hash, '', 'viewer credential removed from history URL');
    const dimensions = await page.locator('canvas').evaluate(c => ({width:c.width, height:c.height}));
    assert.ok(dimensions.width >= 640 && dimensions.height >= 480);
    await page.locator('canvas').focus();
    await page.keyboard.press('Control+Home');
    await page.keyboard.press('Control+Shift+End');
    await page.keyboard.type('wdesk browser');
    await page.evaluate(() => navigator.clipboard.writeText(' — café 日本語 🐈'));
    await page.keyboard.press('Control+V');
    w('clipboard', 'set', '');
    await page.keyboard.press('Control+Home');
    await page.keyboard.press('Control+Shift+End');
    await page.keyboard.press('Control+C');
    const deadline = Date.now() + 30000;
    let text;
    do {
      await page.waitForTimeout(500);
      try { text = w('clipboard', 'get').text; }
      catch (e) {
        // Another Windows application can briefly hold the shared clipboard.
        // This read is safe to retry; do not replay input or hide other failures.
        if (!String(e.stderr || e.message).includes('Requested Clipboard operation did not succeed.')) throw e;
      }
    } while (text !== 'wdesk browser — café 日本語 🐈' && Date.now() < deadline);
    assert.equal(text, 'wdesk browser — café 日本語 🐈', 'browser and agent share the real Windows desktop');
    assert.deepEqual(errors, []);
    await page.waitForTimeout(500); // Allow the next observed framebuffer to reach the canvas.
    const output = process.env.WDESK_VIEWER_SCREENSHOT || '/tmp/wdesk-viewer-smoke.png';
    await page.screenshot({path:output});
    console.log(`Viewer smoke passed (${dimensions.width}x${dimensions.height}); screenshot: ${output}`);
  } finally { await browser.close(); }
})().catch(e => { console.error(e.message); process.exitCode=1; });

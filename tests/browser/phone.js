// What the page does, in a real browser at a real phone's size: the end-to-end tests of the
// browser UI, one behaviour each, each leaving a video of itself behind.
//
// Everything here is a thing that has been broken at least once by a change that looked right
// in the stylesheet. A menu clipped by its own parent's `overflow`, a header that stopped
// sticking, a form that opened between two lines of code with nowhere to put its buttons: none
// of them fail a test that reads CSS, and all of them fail this one.
//
// It runs inside the container, with the page served on the host. The arguments are the URL
// that signs a browser in, and a JSON object saying which commit of codereview is under test.
//
// Every test starts from that URL on a page of its own, so each one stands alone and its
// video shows all of it. What a test leaves, in /tmp/e2e/<its name as a slug>/, is the video
// and `test.json`: whether it passed, and every step and check with the moment of the video
// it happened at and the line of this file that made it. The harness copies those out of the
// container; codereview shows them beside the test.
//
// Hit testing rather than measuring, wherever there is a choice: `getBoundingClientRect` is
// unmoved by a parent that clips its child, so a menu cut off at its header's edge measures
// exactly as it would if it were on screen. `elementFromPoint` is what a finger does.

const fs = require('fs');
const { execFileSync } = require('child_process');
const puppeteer = require('puppeteer');

// Found rather than indexed: `node script.js <url>` and `node -e <script> <url>` number their
// arguments differently, and the harness uses the second so that nothing has to be mounted.
const url = process.argv.find(a => a.startsWith('http'));
const under = JSON.parse(process.argv.find(a => a.startsWith('{')) || '{}');
if (!url) {
  console.error('usage: phone.js <url with token> [{"commit": ..., "dirty": ...}]');
  process.exit(2);
}
const OUT = '/tmp/e2e';
const FILE = 'tests/browser/phone.js';

/// The line of this file that called `fn`. Run with `node -e`, the script's frames are
/// `[eval]`; run as a file, they are its name.
function callerLine(fn) {
  const holder = {};
  Error.captureStackTrace(holder, fn);
  const m = /(?:\[eval\]|phone\.js):(\d+):\d+/.exec(holder.stack);
  return m ? Number(m[1]) : 0;
}

const slug = name => name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');

const tests = [];
/// A test: a sentence saying what the page does, and the steps that show it. `docs` names
/// the documentation it demonstrates, as `path#heading`.
function test(name, body, { docs = [] } = {}) {
  tests.push({ name, body, docs, line: callerLine(test) });
}

/// Whether a tap in the middle of `selector` would land on it, rather than on whatever is
/// drawn over it or on nothing at all because it is clipped away.
const hittable = sel => {
  const el = document.querySelector(sel);
  if (!el) return 'no such element';
  const r = el.getBoundingClientRect();
  if (r.width === 0 || r.height === 0) return 'no size';
  const x = r.left + r.width / 2, y = r.top + r.height / 2;
  if (x < 0 || y < 0 || x > innerWidth || y > innerHeight) return 'off screen';
  const at = document.elementFromPoint(x, y);
  if (!at) return 'nothing there';
  return el.contains(at) || at.contains(el) ? true : 'covered by ' + at.tagName.toLowerCase() + '.' + at.className;
};

const pause = ms => new Promise(r => setTimeout(r, ms));

// ---------------------------------------------------------------------------------------------

test('a phone gets the phone layout', async t => {
  await t.open();
  t.ok('the phone stylesheet applies', await t.page.evaluate(() => matchMedia('(max-width: 700px), (max-height: 500px)').matches));
  t.ok('a finger, not a pointer', await t.page.evaluate(() => matchMedia('(hover: none)').matches));
});

test('a repository opens on its end-to-end tests, each with its recording', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('.e2e .e2e-test');
  t.ok('it opens on the end-to-end tests', await page.evaluate(() =>
    document.querySelector('header .tabs [data-view=e2e]').classList.contains('active') && location.hash === '#/'), await page.evaluate(() => location.hash));
  t.ok('which say how many there are, and how many failed', await page.evaluate(() => {
    const head = document.querySelector('.e2e .e2e-head').textContent.replace(/\s+/g, ' ').trim();
    return /^2 end-to-end tests of \d+ tests · 1 failed/.test(head) ? true : head;
  }));
  t.ok('each a card a finger can reach', await t.inView('.e2e-test.passed .head'));
  // The terminal recording, played as text: its first screen worth seeing.
  await page.waitForFunction(() => /reviews/.test(document.querySelector('.e2e-test.passed pre.term')?.textContent || ''), { timeout: 10000 });
  t.ok('a terminal recording shows its screen', await page.evaluate(() =>
    !document.querySelector('.e2e-test.passed pre.term').textContent.includes('why is this pub')));
  t.step('play the terminal recording');
  await page.click('.e2e-test.passed [data-act=play]');
  await page.waitForFunction(() => /why is this pub/.test(document.querySelector('.e2e-test.passed pre.term').textContent), { timeout: 10000 });
  t.ok('and plays on to the next', true);
  t.ok('fitted to the card', await page.evaluate(() => {
    const pre = document.querySelector('.e2e-test.passed pre.term'), card = pre.closest('.e2e-test');
    return pre.getBoundingClientRect().right <= card.getBoundingClientRect().right + 1 ? true : 'wider than its card';
  }));
  // The video of the one that failed, and what it failed on.
  await t.inView('.e2e-test.failed video');
  await page.waitForFunction(() => {
    const v = document.querySelector('.e2e-test.failed video');
    return v && v.readyState >= 1 && isFinite(v.duration) && v.duration > 1;
  }, { timeout: 15000 });
  t.ok('a browser recording is a video that knows its length', true);
  t.ok('a failure says what it failed on', await page.evaluate(() =>
    /REVIEW.md has it[\s\S]*REVIEW.md was empty/.test(document.querySelector('.e2e-test.failed .failures').textContent)));
  t.step('open the steps and tap the failed check');
  await page.click('.e2e-test.failed details.steps > summary');
  await page.click('.e2e-test.failed details.steps li.failed');
  await page.waitForFunction(() => Math.abs(document.querySelector('.e2e-test.failed video').currentTime - 1.5) < 0.3, { timeout: 5000 });
  t.ok('a step goes to its moment of the video', await page.evaluate(() =>
    document.querySelector('.e2e-test.failed details.steps li.failed').classList.contains('now')));
  t.step('tap the test\'s file');
  await page.click('.e2e-test.failed [data-act=source]');
  await page.waitForSelector('table.code td.n');
  t.ok('the test opens at its line', await page.evaluate(() => location.hash.includes('/file/tests/cli.rs')), await page.evaluate(() => location.hash));
}, { docs: ['SPECS.md#What a repository opens on'] });

test('a repository without end-to-end tests opens on saying so', async t => {
  const page = t.page;
  await t.open('#/r1/');
  await page.waitForSelector('.e2e .finding');
  t.ok('the first thing it says is that there are none', await page.evaluate(() =>
    document.querySelector('.e2e .finding h2').textContent === 'No end-to-end tests'));
  t.ok('and the agent is offered the job', await t.inView('.e2e .finding [data-act=ask]'));
  t.ok('the tests it does have are a tap away', await page.evaluate(hittable, '.e2e .finding [data-act=spec]'));
  t.step('see the tests it does have');
  await page.click('.e2e .finding [data-act=spec]');
  await page.waitForSelector('.spec details.level');
  t.ok('they open', await page.evaluate(() => location.hash.startsWith('#/r1/spec')), await page.evaluate(() => location.hash));
}, { docs: ['SPECS.md#What a repository opens on'] });

test('the summary says what a repository is, where to start and how it is doing', async t => {
  const page = t.page;
  await t.open('#/summary');
  t.ok('the summary has an address of its own', page.url().endsWith('#/summary'), page.url());
  t.ok('with what it says it is', await page.evaluate(() => {
    const el = document.querySelector('.overview .readme');
    return !!el && el.textContent.length > 40;
  }));
  t.ok('where to start reading', await t.inView('.overview .entries .item'));
  t.ok('and how it is doing', await page.evaluate(() => {
    const figs = [...document.querySelectorAll('.overview .fig')].map(f => f.textContent);
    return figs.length >= 4 ? true : 'only ' + JSON.stringify(figs);
  }));
  // The slow half arrives on its own, after the symbol index is built.
  t.step('wait for the symbols to be read');
  await page.waitForFunction(() => {
    const el = document.querySelector('.overview .leaning');
    return el && !el.textContent.includes('reading the symbols');
  }, { timeout: 30000 });
  t.ok('what the tree leans on, once the symbols are read', await t.inView('.overview .leaning .item'));
  // The overview leads with a summary: a card a question, each a way to the view behind it.
  t.ok('the overview leads with a summary', await page.evaluate(() => {
    const n = document.querySelectorAll('.overview .summary .card').length;
    return n === 3 ? true : n + ' cards';
  }));
});

test('a test command is found, and one tap runs it', async t => {
  const page = t.page;
  await t.open('#/summary');
  // What the repository says runs its tests, found rather than configured; running it saves it.
  await page.waitForSelector('.overview .checks .check');
  t.ok('a test command is found', await page.evaluate(() => {
    const row = document.querySelector('.overview .checks .check');
    return row.textContent.includes('make test') && row.textContent.includes('found') ? true : row.textContent;
  }));
  t.ok('and its Run button can be tapped', await t.inView('.overview .checks .check [data-act=run]'));
  t.step('tap Run');
  await page.click('.overview .checks .check [data-act=run]');
  await page.waitForFunction(() => {
    const row = document.querySelector('.overview .checks .check');
    return row && row.classList.contains('passed');
  }, { timeout: 15000 });
  t.ok('it ran, and counted the tests it printed', await page.evaluate(() => {
    const row = document.querySelector('.overview .checks .check');
    return /\b1 test\b/.test(row.textContent) && !row.querySelector('.badge') ? true : row.textContent;
  }));
  t.ok('and says the result matches the tree', await page.evaluate(() => !!document.querySelector('.overview .checks .stale.fresh')));
});

test('the tests read as sentences, folded by level and by file', async t => {
  const page = t.page;
  await t.open('#/summary');
  await page.waitForSelector('.overview [data-card=areas] tr.area');
  t.ok('the tests by area, a row a finger tall', await page.evaluate(() => {
    const h = document.querySelector('[data-card=areas] tr.area').getBoundingClientRect().height;
    return h >= 44 ? true : 'height ' + h;
  }));
  // Drawn from OVERVIEW.md while the index is built, and redrawn every two seconds until it
  // is: a tap aimed at the one being replaced lands on nothing. Wait for the last one.
  await page.waitForFunction(() => document.querySelector('.overview .suite .item')
    && !document.querySelector('.overview .suite .asof'), { timeout: 30000 });
  await t.inView('.overview .suite .item');
  t.step('tap a test file in the overview');
  await page.click('.overview .suite .item');
  await page.waitForSelector('.spec details.level');
  t.ok('the overview leads to the tests', await page.evaluate(() => location.hash.includes('/spec')), await page.evaluate(() => location.hash));
  // The unit tests, the last level: twenty-one, too many to show at once, so the level starts
  // folded, a row a finger tall, and opens to its files, the long one folded and the short one
  // open.
  const UNIT = '.spec details.level:last-of-type';
  t.ok('a long level starts folded', await page.evaluate(sel => {
    const d = document.querySelector(sel);
    return d && !d.open && !d.querySelector('.card-test') ? true : 'open: ' + (d && d.open);
  }, UNIT));
  t.ok('and its row is a finger tall', await page.evaluate(sel => {
    const h = document.querySelector(sel + ' > summary').getBoundingClientRect().height;
    return h >= 44 ? true : 'height ' + h;
  }, UNIT));
  await t.inView(UNIT + ' > summary');
  t.step('open the level');
  await page.click(UNIT + ' > summary');
  await page.waitForSelector(UNIT + ' details.fold');
  t.ok('it opens to its files, each folded', await page.evaluate(sel => {
    const files = [...document.querySelectorAll(sel + ' details.fold')].map(d => {
      const head = d.querySelector('summary');
      return head.querySelector('.grow').textContent + ' ' + head.querySelector('.count').textContent + (d.open ? ' open' : ' folded');
    });
    return files.join(' | ') === 'src/core.rs 1 folded | src/many.rs 20 folded' ? true : files.join(' | ');
  }, UNIT));
  t.step('open the first file');
  await page.click(UNIT + ' details.fold > summary');
  await page.waitForSelector(UNIT + ' .card-test');
  t.ok('a test reads as words', await page.evaluate(sel => {
    const t = document.querySelector(sel + ' .card-test .label').textContent.trim();
    return t === 'a review round trips' ? true : t;
  }, UNIT));
  t.ok('and is a finger tall', await page.evaluate(sel => {
    const h = document.querySelector(sel + ' .card-test').getBoundingClientRect().height;
    return h >= 44 ? true : 'height ' + h;
  }, UNIT));
  // A search goes through every test, folded or not, as it is typed.
  await t.inView('.spec input.find');
  t.step('search for "number 1"');
  await page.type('.spec input.find', 'number 1');
  t.ok('a search finds tests in folds', await page.evaluate(() => {
    // Only what is shown: the folds behind the results are hidden, not gone.
    const n = [...document.querySelectorAll('.spec .card-test')].filter(c => c.offsetParent).length;
    const said = document.querySelector('.spec .caveat').textContent;
    return n === 11 && said.startsWith('11 tests match') ? true : n + ' cards, ' + said;
  }));
  await page.evaluate(() => { const i = document.querySelector('.spec input.find'); i.value = ''; i.dispatchEvent(new Event('input')); });
  // A page on a phone's home screen has no browser buttons: the way back is the page's own.
  t.ok('the back button is there', await page.evaluate(hittable, '#back'));
  t.step('tap back');
  await page.click('#back');
  await page.waitForSelector('.overview .summary');
  t.ok('and goes back', await page.evaluate(() => location.hash === '#/summary'), await page.evaluate(() => location.hash));
});

test('the tabs lead to the files, the changes and the log', async t => {
  const page = t.page;
  await t.open('#/summary');
  // The overview is a tab of its own, lit while it is up; Code with nothing open is the
  // tree, and the Summary tab brings the overview back.
  t.ok('the Summary tab is lit', await page.evaluate(() => document.querySelector('header .tabs [data-view=overview]').classList.contains('active')));
  t.ok('the Summary tab can be tapped', await page.evaluate(hittable, 'header .tabs [data-view=overview]'));
  t.step('tap Code');
  await page.click('header .tabs [data-view=explore]');
  await page.waitForSelector('#tree .row');
  t.ok('Code with nothing open is the tree', await page.evaluate(() => location.hash.endsWith('/tree')), await page.evaluate(() => location.hash));
  t.ok('Code holds the files, the changes and the log', await page.evaluate(() => {
    const subs = [...document.querySelectorAll('#subnav button')].map(b => b.dataset.sub).join(' ');
    return subs === 'explore changes log' ? true : subs;
  }));
  t.ok('and each can be tapped', await page.evaluate(hittable, '#subnav [data-sub=log]'));
  t.step('tap Summary');
  await page.click('header .tabs [data-view=overview]');
  await page.waitForSelector('.overview');
  t.ok('Summary brings the overview back', await page.evaluate(() => !!document.querySelector('.overview')));
});

test('the header menu refreshes, and Files opens the tree', async t => {
  const page = t.page;
  await t.open('#/summary');
  // The header's own menu: search, colour scheme, refresh, sign out, help. It hangs below the
  // header, which is why an `overflow: hidden` on the header once made it untappable.
  t.step('open the header menu');
  await page.click('#more');
  t.ok('the header menu opens', await page.evaluate(hittable, '#refresh'));
  t.step('tap Refresh');
  await page.click('#refresh');
  // Refreshing redraws the overview, and a tap aimed at the one being replaced lands on
  // nothing: wait for the redraw to start, then for the new one.
  await page.waitForFunction(() => document.querySelector('#msg').textContent.includes('refreshed'));
  await page.waitForSelector('#content .panelhead [data-act=files]');
  // Files is one tap from the overview, and the tree is what it opens.
  t.step('tap Files');
  await page.click('#content .panelhead [data-act=files]');
  await page.waitForSelector('#tree .row');
  t.ok('Files opens the tree', await page.evaluate(() => location.hash.endsWith('/tree')), await page.evaluate(() => location.hash));
});

test('a picture is shown, fitted to the phone', async t => {
  const page = t.page;
  // Fetched with the token like any request.
  await t.open('#/file/logo.png');
  await page.waitForFunction(() => {
    const img = document.querySelector('.picture img');
    return img && img.complete && img.naturalWidth > 0;
  }, { timeout: 15000 });
  t.ok('a picture is shown, not called binary', await page.evaluate(() => {
    const img = document.querySelector('.picture img');
    const fits = img.getBoundingClientRect().width <= innerWidth;
    return fits && !document.querySelector('#content').textContent.includes('binary file') ? true : 'width ' + img.getBoundingClientRect().width;
  }));
  t.ok('with its size under it', await page.evaluate(() => /\d+ × \d+/.test(document.querySelector('.picture .caption').textContent)));
}, { docs: ['SPECS.md#Browser UI'] });

test('a file keeps its header and menu in reach while it scrolls', async t => {
  const page = t.page;
  await t.open('#/file/long.rs');
  await page.waitForSelector('table.code td.n');
  const head = await page.evaluate(() => document.querySelector('#content .panelhead').getBoundingClientRect().height);
  t.ok('the file header is one row', head < 60, head + 'px');
  t.ok('Comment is on it', await page.evaluate(hittable, '#content .panelhead [data-act=comment]'));
  t.ok('and the rest is behind one button', await page.evaluate(hittable, '#content .panelhead [data-act=menu]'));
  t.step('open the file menu');
  await page.click('#content .panelhead [data-act=menu]');
  t.ok('the file menu opens', await page.evaluate(hittable, '#content .acts [data-act=zen]'));
  // A header that scrolls away with the code takes the button that opens that menu with it.
  t.step('scroll down the file');
  await page.evaluate(() => { document.querySelector('#content .acts').classList.remove('open'); $('#content').scrollTop = 600; });
  await pause(200);
  const stuck = await page.evaluate(() => {
    const head = document.querySelector('#content .panelhead').getBoundingClientRect();
    const content = document.querySelector('#content').getBoundingClientRect();
    return Math.round(head.top - content.top);
  });
  t.ok('the file header sticks while the code scrolls', Math.abs(stuck) <= 2, stuck + 'px from the top');
  t.ok('and its menu button is still there', await page.evaluate(hittable, '#content .panelhead [data-act=menu]'));
});

test('writing a comment takes over the screen', async t => {
  const page = t.page;
  await t.open('#/file/long.rs');
  await page.waitForSelector('table.code td.n');
  // Rather than opening between two lines of code, with its buttons under the keyboard.
  t.step('tap line 40');
  await page.click('table.code tr[data-line="40"] td.n');
  await page.waitForSelector('#content tr.form .form');
  const form = await page.evaluate(() => {
    const r = document.querySelector('#content tr.form .form').getBoundingClientRect();
    return { w: Math.round(r.width), h: Math.round(r.height) };
  });
  t.ok('the comment form covers the screen', form.w >= 350 && form.h >= 400, JSON.stringify(form));
  t.ok('its save button is reachable', await page.evaluate(hittable, '#content tr.form [data-act=save]'));
  t.ok('and so is cancel, below the keyboard', await page.evaluate(hittable, '#content tr.form [data-act=cancel]'));
  t.step('tap Cancel');
  await page.click('#content tr.form [data-act=cancel]');
  await pause(200);
}, { docs: ['SPECS.md#Browser UI'] });

test('zen leaves the code and one way back', async t => {
  const page = t.page;
  await t.open('#/file/long.rs');
  await page.waitForSelector('table.code td.n');
  t.step('choose Zen from the file menu');
  await page.click('#content .panelhead [data-act=menu]');
  await page.click('#content .acts [data-act=zen]');
  const gone = await page.evaluate(() => ['header', 'footer', '#content .panelhead']
    .filter(s => { const el = document.querySelector(s); return el && el.getBoundingClientRect().height > 0; }));
  t.ok('zen takes the frame away', gone.length === 0, 'still showing: ' + gone.join(', '));
  t.ok('and leaves the way back', await page.evaluate(hittable, '#zen'));
  t.ok('the code is still there', await page.evaluate(() => !!document.querySelector('table.code td.c')));
  t.step('leave zen');
  await page.click('#zen');
  t.ok('the frame comes back', await page.evaluate(hittable, '#content .panelhead [data-act=menu]'));
});

test('long lines wrap where the screen ends', async t => {
  const page = t.page;
  await t.open('#/file/long.rs');
  await page.waitForSelector('table.code td.n');
  t.ok('a long file scrolls sideways to begin with', await page.evaluate(() => $('#content').scrollWidth > $('#content').clientWidth));
  t.step('choose Wrap from the file menu');
  await page.click('#content .panelhead [data-act=menu]');
  await page.click('#content .acts [data-act=wrap]');
  await pause(200);
  t.ok('and does not once lines wrap', await page.evaluate(() => $('#content').scrollWidth <= $('#content').clientWidth + 1));
});

test('a test opens on its own, with the code it ran and what documents it', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('.e2e-test.failed a.label');
  t.step('tap the failed test');
  await page.click('.e2e-test.failed a.label');
  await page.waitForSelector('.testview .ran .item');
  t.ok('it has an address of its own', await page.evaluate(() => location.hash.startsWith('#/test?')), await page.evaluate(() => location.hash));
  t.ok('each step says which line of the test took it', await page.evaluate(() => {
    const src = document.querySelector('.testview ol.steps li .src');
    return src && /^9: assert!\(true\);$/.test(src.textContent.trim()) ? true : src && src.textContent;
  }));
  t.ok('the code it ran comes first, with what only it ran', await page.evaluate(() => {
    const row = document.querySelector('.testview .ran .item');
    return /src\/core\.rs[\s\S]*2 · 2 only here/.test(row.textContent) ? true : row.textContent;
  }));
  t.ok('the documentation it names', await page.evaluate(() => {
    const row = document.querySelector('.testview .docs .item');
    return /Saving a review[\s\S]*README\.md:\d+[\s\S]*named by the test/.test(row.textContent) ? true : row && row.textContent;
  }));
  t.ok('and the smaller tests below it', await page.evaluate(() =>
    /a review round trips/.test(document.querySelector('.testview .smaller').textContent)));
  t.ok('the code it ran is a finger tall', await page.evaluate(() =>
    document.querySelector('.testview .ran .item').getBoundingClientRect().height >= 32));
  await t.inView('.testview .ran .item');
  t.step('open the code it ran');
  await page.click('.testview .ran .item');
  await page.waitForSelector('table.code tr.ran');
  t.ok('the file marks the lines the test ran', await page.evaluate(() =>
    [...document.querySelectorAll('table.code tr.ran')].map(r => r.dataset.line).join(',') === '1,2'));
  t.ok('and says whose they are', await page.evaluate(() => /a review is saved/.test(document.querySelector('.ranbanner').textContent)));
  t.step('go back to the test');
  await page.click('.ranbanner [data-act=test]');
  await page.waitForSelector('.testview .docs .item');
  t.step('open what documents it');
  await page.click('.testview .docs .item');
  await page.waitForSelector('table.code td.n');
  t.ok('the documentation opens at its heading', await page.evaluate(() => /README\.md\?line=\d+/.test(location.hash)), await page.evaluate(() => location.hash));
}, { docs: ['SPECS.md#What a repository opens on'] });

// ---------------------------------------------------------------------------------------------

/// Runs one test on a page of its own, recording it, and writes what it left.
async function run(browser, spec, server) {
  const dir = `${OUT}/${slug(spec.name)}`;
  fs.mkdirSync(dir, { recursive: true });
  const page = await browser.newPage();
  // A small phone, upright, with a finger rather than a pointer.
  await page.setViewport({ width: 360, height: 640, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });
  // The recording is sized from the page it starts on, and a blank page is laid out for a
  // desktop, 980 pixels wide: without this the video is the phone in one corner of a black
  // frame three times its size.
  await page.setContent('<meta name="viewport" content="width=device-width, initial-scale=1">');
  // What the page's script ran, from here to the end of the test: Chromium keeps the count
  // across the navigations a test makes.
  await page.coverage.startJSCoverage({ resetOnNavigation: false });
  // And the server's, when it is built to count: what it ran before this test is not this
  // test's.
  if (server) await server('/coverage');
  const recorder = await page.screencast({ path: `${dir}/raw.webm` });
  paced(page);
  const started = new Date();
  const steps = [];
  const at = () => Date.now() - started.getTime();
  const t = {
    page,
    /// Something the test does, said the way a person would.
    step(text) { steps.push({ at_ms: at(), line: callerLine(t.step), kind: 'action', text }); },
    /// Only `true` passes. `hittable` answers with a description of what went wrong instead
    /// of a bare false, and a non-empty string is truthy: taking `!!pass` here quietly turned
    /// every one of those descriptions into a pass, which is how this file spent its first
    /// half hour agreeing with a page that was broken.
    ok(text, pass, detail) {
      const step = { at_ms: at(), line: callerLine(t.ok), kind: 'check', text, passed: pass === true };
      if (pass !== true) step.detail = String(detail !== undefined ? detail : pass);
      steps.push(step);
    },
    /// Signs in and opens the page, at `hash` if given.
    async open(hash) {
      steps.push({ at_ms: at(), line: callerLine(t.open), kind: 'action', text: hash ? `open ${hash.slice(2)}` : 'open the repository' });
      await page.goto(hash ? url.split('#')[0] + hash : url, { waitUntil: 'networkidle0' });
    },
    /// Scrolled to before it is asked about: a section below the fold is not a broken section,
    /// and `hittable` rightly refuses anything off screen.
    async inView(sel) {
      await page.evaluate(s => document.querySelector(s)?.scrollIntoView({ block: 'center' }), sel);
      return page.evaluate(hittable, sel);
    },
  };
  page.on('pageerror', e => t.ok('no script errors', false, e.message));
  try {
    await spec.body(t);
  } catch (e) {
    steps.push({ at_ms: at(), line: errorLine(e) || spec.line, kind: 'check', text: 'the test runs to its end', passed: false, detail: String(e && e.message) });
  }
  // A moment of the end state, so the video does not stop on the last tap.
  await pause(400);
  await recorder.stop();
  fs.writeFileSync(`${dir}/page.lcov`, pageLcov(await page.coverage.stopJSCoverage()));
  if (server) await server('/coverage?test=' + slug(spec.name));
  await page.close();
  // Written as it was recorded, a stream, the video says neither how long it is nor where
  // its frames are, and a player can then neither show its length nor seek in it. Copying it
  // through ffmpeg once more writes both.
  try {
    execFileSync('ffmpeg', ['-v', 'error', '-y', '-i', `${dir}/raw.webm`, '-c', 'copy', `${dir}/video.webm`]);
    fs.unlinkSync(`${dir}/raw.webm`);
  } catch (e) {
    fs.renameSync(`${dir}/raw.webm`, `${dir}/video.webm`);
  }
  const passed = steps.every(s => s.kind !== 'check' || s.passed);
  const result = {
    test: spec.name,
    file: FILE,
    line: spec.line,
    passed,
    started: started.toISOString(),
    duration_ms: Date.now() - started.getTime(),
    commit: under.commit || null,
    dirty: !!under.dirty,
    recording: 'video.webm',
    run_by: under.run_by,
    docs: spec.docs,
    steps,
  };
  fs.writeFileSync(`${dir}/test.json`, JSON.stringify(result, null, 1) + '\n');
  return result;
}

/// The lines of `src/web/index.html` the page's own script ran, as lcov, from what Chromium
/// says ran: ranges of the script's text. The script starts on the line of its `<script>` tag,
/// which the harness looks up and passes in.
function pageLcov(entries) {
  const lines = new Set();
  for (const e of entries) {
    if (!e.text.includes('function renderFile')) continue;
    const starts = [0];
    for (let i = 0; i < e.text.length; i++) if (e.text[i] === '\n') starts.push(i + 1);
    const lineOf = offset => {
      let lo = 0, hi = starts.length - 1;
      while (lo < hi) {
        const mid = (lo + hi + 1) >> 1;
        if (starts[mid] <= offset) lo = mid; else hi = mid - 1;
      }
      return lo;
    };
    for (const r of e.ranges) {
      if (r.end <= r.start) continue;
      for (let l = lineOf(r.start); l <= lineOf(r.end - 1); l++) {
        if (e.text.slice(starts[l], starts[l + 1] ?? e.text.length).trim()) lines.add(l);
      }
    }
  }
  if (!lines.size || !under.script_line) return '';
  const da = [...lines].sort((a, b) => a - b).map(l => `DA:${under.script_line + l},1`).join('\n');
  return `SF:src/web/index.html\n${da}\nend_of_record\n`;
}

/// How long the video holds on what a tap, a keystroke or a page load led to. Run at machine
/// speed, a test is over before anyone watching has seen what it did.
const HOLD = 600;

/// Makes `page` keep a person's pace: every tap is marked where the finger landed, and every
/// action is held for a moment before the next. The mark is drawn after the tap and gone
/// before the test goes on, and nothing can land on it, so no check ever sees it.
function paced(page) {
  const click = page.click.bind(page);
  page.click = async (selector, options) => {
    const box = await page.$eval(selector, el => {
      const r = el.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    }).catch(() => null);
    await click(selector, options);
    if (box) {
      await page.evaluate(({ x, y }) => {
        const dot = document.createElement('div');
        dot.id = 'e2e-tap';
        dot.style.cssText = `position:fixed;left:${x - 18}px;top:${y - 18}px;width:36px;height:36px;`
          + 'border-radius:50%;background:rgba(255,64,64,.45);border:2px solid #f44;'
          + 'pointer-events:none;z-index:2147483647';
        document.documentElement.appendChild(dot);
      }, box).catch(() => {});
    }
    await pause(HOLD);
    await page.evaluate(() => document.getElementById('e2e-tap')?.remove()).catch(() => {});
  };
  for (const name of ['type', 'goto']) {
    const original = page[name].bind(page);
    page[name] = async (...args) => {
      const result = await original(...args);
      await pause(HOLD);
      return result;
    };
  }
}

/// The line of this file an error was thrown from, where it was thrown from this file at all.
function errorLine(e) {
  const m = /(?:\[eval\]|phone\.js):(\d+):\d+/.exec((e && e.stack) || '');
  return m ? Number(m[1]) : 0;
}

(async () => {
  const browser = await puppeteer.launch({ args: ['--no-sandbox'] });
  // When the server counts what it runs, a call between tests takes each test's share. The
  // API wants the page's own token, which a page that signs in has; that page is closed
  // before any test starts, so that nothing it asks for is counted.
  let server = null;
  if (under.coverage) {
    const signIn = await browser.newPage();
    await signIn.goto(url, { waitUntil: 'networkidle0' });
    const token = await signIn.evaluate(() => TOKEN);
    await signIn.close();
    const base = new URL(url).origin;
    server = path => fetch(base + '/api' + path, { method: 'POST', headers: { Authorization: 'Bearer ' + token } });
  }
  let failed = 0;
  for (const spec of tests) {
    const result = await run(browser, spec, server);
    if (!result.passed) failed++;
    console.log((result.passed ? 'ok   ' : 'FAIL ') + spec.name);
    for (const s of result.steps.filter(s => s.kind === 'check' && !s.passed)) {
      console.log(`       line ${s.line}: ${s.text}  (${s.detail})`);
    }
  }
  await browser.close();
  console.log(`${tests.length - failed} of ${tests.length} tests passed`);
  process.exit(failed ? 1 : 0);
})().catch(e => {
  console.error('the browser could not finish: ' + (e && e.message));
  process.exit(1);
});

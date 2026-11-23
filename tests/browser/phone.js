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
  await page.waitForSelector('#tests [data-test]');
  t.ok('it opens on the end-to-end tests', await page.evaluate(() =>
    document.querySelector('header .focus [data-focus=tests]').classList.contains('active') && location.hash === '#/'), await page.evaluate(() => location.hash));
  t.ok('the list starts with the overview, then each test, marked', await page.evaluate(() => {
    const rows = [...document.querySelectorAll('#tests .row')].map(r => r.textContent.replace(/\s+/g, ' ').trim());
    return rows[0] === 'Overview' && rows.some(r => r.startsWith('✓')) && rows.some(r => r.startsWith('✗')) ? true : rows.join(' | ');
  }));
  t.ok('each a row a finger can reach', await t.inView('#tests .row.passed'));
  t.step('tap the overview');
  await page.click('#tests .row.overview');
  await page.waitForSelector('.e2e .e2e-head');
  t.ok('which says how many there are, and how many failed', await page.evaluate(() => {
    const head = document.querySelector('.e2e .e2e-head').textContent.replace(/\s+/g, ' ').trim();
    return /^2 end-to-end tests of \d+ tests · 1 failed/.test(head) ? true : head;
  }));
  t.step('back to the list, and tap the test that passed');
  await page.click('#content [data-act=tests]');
  await page.click('#tests .row.passed');
  // The terminal recording, played as text: its first screen worth seeing.
  await page.waitForFunction(() => /reviews/.test(document.querySelector('.testview pre.term')?.textContent || ''), { timeout: 10000 });
  t.ok('a terminal recording shows its screen', await page.evaluate(() =>
    !document.querySelector('.testview pre.term').textContent.includes('why is this pub')));
  t.step('play the terminal recording');
  await page.click('.testview [data-act=play]');
  await page.waitForFunction(() => /why is this pub/.test(document.querySelector('.testview pre.term').textContent), { timeout: 10000 });
  t.ok('and plays on to the next', true);
  t.ok('fitted to the screen', await page.evaluate(() => {
    const pre = document.querySelector('.testview pre.term');
    return pre.getBoundingClientRect().right <= innerWidth + 1 ? true : 'wider than the screen';
  }));
  t.step('back to the list, and tap the test that failed');
  await page.click('#content [data-act=tests]');
  await page.click('#tests .row.failed');
  await page.waitForSelector('.testview video');
  await page.waitForFunction(() => {
    const v = document.querySelector('.testview video');
    return v && v.readyState >= 1 && isFinite(v.duration) && v.duration > 1;
  }, { timeout: 15000 });
  t.ok('a browser recording is a video that knows its length', true);
  t.ok('a failure says what it failed on', await page.evaluate(() =>
    /REVIEW.md has it[\s\S]*REVIEW.md was empty/.test(document.querySelector('.testview .failures').textContent)));
  t.step('tap the failed check');
  await t.inView('.testview ol.steps li.failed');
  await page.click('.testview ol.steps li.failed');
  await page.waitForFunction(() => Math.abs(document.querySelector('.testview video').currentTime - 1.5) < 0.3, { timeout: 5000 });
  t.ok('a step goes to its moment of the video', await page.evaluate(() =>
    document.querySelector('.testview ol.steps li.failed').classList.contains('now')));
  t.step('tap the test\'s source');
  await page.click('#content .panelhead [data-act=source]');
  await page.waitForSelector('table.code td.n');
  t.ok('the test opens at its line', await page.evaluate(() => location.hash.includes('/file/tests/cli.rs')), await page.evaluate(() => location.hash));
}, { docs: ['SPECS.md#What a repository opens on'] });

test('on a wide screen the tests sit beside what is selected in them', async t => {
  const page = t.page;
  // A desk, not a phone: the side panel and what is selected in it are both on screen.
  await page.setViewport({ width: 1280, height: 800, isMobile: false, hasTouch: false });
  await t.open();
  await page.waitForSelector('#tests [data-test]');
  await page.waitForSelector('.stretch-card h3');
  t.ok('the list is beside the overview', await page.evaluate(() => {
    const list = document.querySelector('#tests').getBoundingClientRect(), main = document.querySelector('#content').getBoundingClientRect();
    return list.width > 0 && main.width > 0 && list.right <= main.left + 1 ? true : `list ${list.left}-${list.right}, main ${main.left}-${main.right}`;
  }));
  t.ok('with the overview selected', await page.evaluate(() => document.querySelector('#tests .row.overview').classList.contains('selected')));
  await page.screenshot({ path: `${t.dir}/overview.png` });
  t.step('tap the test that failed');
  await page.click('#tests .row.failed');
  await page.waitForSelector('.testview .ran');
  t.ok('the test is beside the list, which stays', await page.evaluate(() =>
    document.querySelector('#tests .row.failed').classList.contains('selected') && document.querySelector('#tests').getBoundingClientRect().width > 0));
  await page.screenshot({ path: `${t.dir}/test.png` });
  t.step('press \\\\');
  await page.keyboard.press('Backslash');
  t.ok('the list hides, and the test takes the width', await page.evaluate(() =>
    document.querySelector('#tests').getBoundingClientRect().width === 0 && document.querySelector('#content').getBoundingClientRect().left < 2));
}, { docs: ['SPECS.md#Browser UI'] });

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
  await page.waitForSelector('.spec details.group');
  t.ok('they open', await page.evaluate(() => location.hash.startsWith('#/r1/spec')), await page.evaluate(() => location.hash));
}, { docs: ['SPECS.md#What a repository opens on'] });

test('the tests can be grouped by another of the questions', async t => {
  const page = t.page;
  await t.open('#/spec');
  await page.waitForSelector('.spec details.group');
  t.ok('they start grouped by how they interact', await page.evaluate(() =>
    [...document.querySelectorAll('.spec details.group > summary b, .spec .fold-empty b')].map(b => b.textContent).join(', ')
      === 'API, GUI/TUI, CLI, No interaction' || [...document.querySelectorAll('.spec details.group > summary b, .spec .fold-empty b')].map(b => b.textContent).join(', ')));
  t.ok('the question is in reach', await page.evaluate(hittable, '.spec select.question'));
  t.step('group them by what artifact they test');
  await page.select('.spec select.question', 'artifact');
  await page.waitForFunction(() => location.hash.includes('by=artifact'));
  await page.waitForFunction(() => [...document.querySelectorAll('.spec details.group > summary b, .spec .fold-empty b')]
    .some(b => b.textContent === 'Documentation'));
  t.ok('the groups are the artifacts', await page.evaluate(() =>
    [...document.querySelectorAll('.spec details.group > summary b, .spec .fold-empty b')].slice(0, 4).map(b => b.textContent).join(', ')
      === 'Code, Config, Data, Documentation' || [...document.querySelectorAll('.spec details.group > summary b, .spec .fold-empty b')].map(b => b.textContent).join(', ')));
}, { docs: ['SPECS.md#What kind of test each one is'] });

test('the tests can be grouped by whether they use malformed input', async t => {
  const page = t.page;
  await t.open('#/spec');
  await page.waitForSelector('.spec select.question');
  t.ok('the question is one of those offered', await page.evaluate(() =>
    [...document.querySelectorAll('.spec select.question option')].some(o => o.textContent === 'Is the test using malformed input?')));
  t.step('group them by whether they use malformed input');
  await page.select('.spec select.question', 'malformed_input');
  await page.waitForFunction(() => location.hash.includes('by=malformed_input'));
  await page.waitForFunction(() => [...document.querySelectorAll('.spec details.group > summary b, .spec .fold-empty b')]
    .some(b => b.textContent === 'Well-formed input'));
  t.ok('the groups are its options', await page.evaluate(() => {
    const groups = [...document.querySelectorAll('.spec details.group > summary b, .spec .fold-empty b')].map(b => b.textContent);
    return ['Targeted attack', 'Heuristic', 'Random', 'Well-formed input'].every(g => groups.includes(g)) ? true : groups.join(', ');
  }));
}, { docs: ['SPECS.md#What kind of test each one is'] });

test('a test opens to what kind of test it is and why', async t => {
  const page = t.page;
  await t.open('#/spec');
  await page.waitForSelector('.spec details.group');
  const GROUP = '.spec details.group:first-of-type';
  await page.waitForSelector(GROUP + ' .card-test details.kind');
  await t.inView(GROUP + ' .card-test details.kind > summary');
  t.ok('an end-to-end test says so', await page.evaluate(sel =>
    document.querySelector(sel + ' .card-test details.kind > summary').textContent === 'end to end'
      || document.querySelector(sel + ' .card-test details.kind > summary').textContent, GROUP));
  t.step('open what kind of test it is');
  await page.click(GROUP + ' .card-test details.kind > summary');
  t.ok('it stays on the list', await page.evaluate(() => location.hash.includes('/spec')), await page.evaluate(() => location.hash));
  t.ok('every question is answered', await page.evaluate(sel =>
    document.querySelectorAll(sel + ' .card-test details.kind[open] tr').length === 9, GROUP));
  t.ok('with why', await page.evaluate(sel => {
    const row = [...document.querySelectorAll(sel + ' .card-test details.kind[open] tr')].find(r => r.querySelector('th').textContent === 'interaction');
    return row && row.textContent.includes('CLI') && row.textContent.includes('it drives the built program') ? true : row && row.textContent;
  }, GROUP));
}, { docs: ['SPECS.md#What kind of test each one is'] });

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
  // The parts that need the symbols arrive later and move everything below them: a tap
  // aimed before then lands on whatever moved under the finger — once, the next check's Run.
  await page.waitForFunction(() => {
    const el = document.querySelector('.overview .leaning');
    return el && !el.textContent.includes('reading the symbols')
      && document.querySelector('.overview .suite .item') && !document.querySelector('.overview .suite .asof');
  }, { timeout: 30000 });
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

test('the tests read as sentences, folded by how they interact and by file', async t => {
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
  await page.waitForSelector('.spec details.group');
  t.ok('the overview leads to the tests', await page.evaluate(() => location.hash.includes('/spec')), await page.evaluate(() => location.hash));
  // The tests that interact with nothing, the last group: twenty-one, too many to show at once,
  // so the group starts folded, a row a finger tall, and opens to its files, each folded.
  const UNIT = '.spec details.group:last-of-type';
  t.ok('a long group starts folded', await page.evaluate(sel => {
    const d = document.querySelector(sel);
    return d && !d.open && !d.querySelector('.card-test') ? true : 'open: ' + (d && d.open);
  }, UNIT));
  t.ok('and its row is a finger tall', await page.evaluate(sel => {
    const h = document.querySelector(sel + ' > summary').getBoundingClientRect().height;
    return h >= 44 ? true : 'height ' + h;
  }, UNIT));
  await t.inView(UNIT + ' > summary');
  t.step('open the group');
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

test('the focuses are Tests, Code, Docs and Data, and the page opens on Tests', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('#tests [data-test]');
  t.ok('the focus picker is in that order', await page.evaluate(() => {
    const names = [...document.querySelectorAll('header .focus button')].map(b => b.textContent.trim()).join(', ');
    return names === 'Tests, Code, Docs, Data' ? true : names;
  }));
  t.ok('Tests is lit, at the root address', await page.evaluate(() =>
    document.querySelector('header .focus [data-focus=tests]').classList.contains('active') && location.hash === '#/'), await page.evaluate(() => location.hash));
  t.ok('and no other is', await page.evaluate(() => document.querySelectorAll('header .focus button.active').length === 1));
  // Four focuses in a phone's header: each must be where a finger lands, and say its name.
  for (const focus of ['tests', 'code', 'docs', 'data']) {
    t.ok(`the ${focus} focus can be tapped`, await page.evaluate(hittable, `header .focus [data-focus=${focus}]`));
  }
  t.ok('no focus name is cut short', await page.evaluate(() => {
    const cut = [...document.querySelectorAll('header .focus button')].filter(b => b.scrollWidth > b.clientWidth + 1).map(b => b.textContent.trim());
    return cut.length ? 'cut: ' + cut.join(', ') : true;
  }));
  t.ok('there is no tab bar along the bottom', await page.evaluate(() => {
    const at = document.elementFromPoint(innerWidth / 2, innerHeight - 10);
    return !at || !at.closest('header') ? true : 'the header is at the bottom edge';
  }));
  t.step('tap All tests');
  await page.click('#subnav [data-sub=spec]');
  await page.waitForSelector('.spec');
  t.ok('every test is a view the tests hold', await page.evaluate(() =>
    location.hash.startsWith('#/spec') && document.querySelector('header .focus [data-focus=tests]').classList.contains('active')), await page.evaluate(() => location.hash));
}, { docs: ['SPECS.md#Browser UI'] });

test('the breadcrumb names the branch and the range the tests are seen through', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('.e2e');
  t.ok('the branch is in the header', await page.evaluate(hittable, '#branch'));
  t.ok('and so is the range', await page.evaluate(() => document.querySelector('#range').textContent.trim() === 'last 7 days' ? true : document.querySelector('#range').textContent));
  t.ok('the header is two rows: the breadcrumb, Agent and the menu, then the focus picker', await page.evaluate(() => {
    const mid = el => { const r = document.querySelector(el).getBoundingClientRect(); return Math.round((r.top + r.bottom) / 2); };
    const row = [mid('#branch'), mid('#agentbtn'), mid('#more')];
    return Math.max(...row) - Math.min(...row) < 8 && mid('header .focus') > Math.max(...row) ? true : `rows at ${row}, focus at ${mid('header .focus')}`;
  }));
  t.ok('neither is under the focus picker', await page.evaluate(() => {
    const a = document.querySelector('header .crumbs').getBoundingClientRect(), b = document.querySelector('header .focus').getBoundingClientRect();
    const apart = a.right <= b.left + 1 || b.right <= a.left + 1 || a.bottom <= b.top + 1 || b.bottom <= a.top + 1;
    return apart ? true : 'the breadcrumb overlaps the focus picker';
  }));
}, { docs: ['SPECS.md#Browser UI'] });

test('the review range is picked from the breadcrumb', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('#tests [data-test]');
  t.step('tap the range in the breadcrumb');
  await page.click('#range');
  await page.waitForSelector('#ranges .choice');
  t.ok('the picker offers the stretches', await page.evaluate(() =>
    [...document.querySelectorAll('#ranges [data-stretch]')].map(c => c.textContent.trim()).join(', ') === 'last 7 days, last commit, uncommitted'));
  t.ok('each a finger can reach, the list scrolled to it', await t.inView('#ranges [data-stretch=uncommitted]'));
  t.step('pick the uncommitted work');
  await page.click('#ranges [data-stretch=uncommitted]');
  t.ok('the breadcrumb names it', await page.evaluate(() => document.querySelector('#range').textContent.trim() === 'uncommitted' ? true : document.querySelector('#range').textContent));
  t.ok('and the picker has gone', await page.evaluate(() => document.querySelector('#ranges').hidden));
}, { docs: ['SPECS.md#Browser UI'] });

test('o opens what to review, as the range in the breadcrumb does', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('#tests [data-test]');
  t.step('press o');
  await page.keyboard.press('o');
  await page.waitForSelector('#ranges [data-head]');
  t.ok('the range picker is open', await page.evaluate(() => !document.querySelector('#ranges').hidden));
}, { docs: ['SPECS.md#Browser UI'] });

/// A range just picked: it lands on the tests, on what it did to them, and its files are the
/// code's, a tap away.
async function landsOnTestsThenCode(t) {
  const page = t.page;
  await page.waitForFunction(() => location.hash.endsWith('/tests'));
  t.ok('it opens on what it did to the tests', await page.evaluate(() =>
    document.querySelector('header .focus [data-focus=tests]').classList.contains('active')));
  t.step('go to its code');
  await page.click('header .focus [data-focus=code]');
  await page.waitForSelector('#subnav [data-sub=range]');
  await page.click('#subnav [data-sub=range]');
  await page.waitForSelector('.range .files .item');
}

const filesOfRange = () => [...document.querySelectorAll('.range .files .item')].map(i => i.textContent.replace(/\s+/g, ' ').trim()).join(' | ');

test('a pull request from the forge opens with its files and comments', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('#tests [data-test]');
  t.step('open the range picker, which asks the forge for its pull requests');
  await page.click('#range');
  await page.waitForSelector('#ranges .pulls .choice');
  t.ok('the pull request is offered', await page.evaluate(() =>
    /#7\s*Say what the feature adds\s*ana/.test(document.querySelector('#ranges .pulls .choice').textContent)));
  t.step('pick it');
  await page.click('#ranges .pulls .choice');
  await landsOnTestsThenCode(t);
  t.ok('it is the review range, at an address of its own', await page.evaluate(() =>
    location.hash.includes('/range?pull=7') && document.querySelector('#range').textContent.trim() === '#7'), await page.evaluate(() => location.hash + ' ' + document.querySelector('#range').textContent));
  t.ok('it says what it is', await page.evaluate(() =>
    /open\s*· feature into \S+ by ana/.test(document.querySelector('.range').textContent)));
  t.ok('the files it changes, each with its size', await page.evaluate(filesOfRange) === 'Afeature.txt+1'
    || await page.evaluate(filesOfRange));
  t.ok('and the whole size, as omnidiff counts it', await page.evaluate(() =>
    /Files it changes\s*1 · \+1/.test(document.querySelector('.range h3').textContent)));
  t.ok('and its comments, read from the forge', await page.evaluate(() =>
    /bo on feature\.txt:1\s*Why this line\?/.test(document.querySelector('.range .comments').textContent)));
  t.step('tap the comment');
  await page.click('.range .comments .item');
  await page.waitForSelector('#content table.code');
  t.ok('it opens the diff at its line', await page.evaluate(() => {
    const row = document.querySelector('#content tr.sel[data-after="1"]');
    return row && /what the feature adds/.test(row.textContent) ? true : 'no line 1 picked out';
  }));
}, { docs: ['SPECS.md#Pull requests and comparisons'] });

test('a branch is compared with the one checked out as a pull request would be', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('#tests [data-test]');
  t.step('open the range picker and find the feature branch');
  await page.click('#range');
  await page.waitForSelector('#ranges [data-head="feature"]');
  await page.type('#ranges input.find', 'feat');
  t.ok('the filter leaves the branch, and not the stretches', await page.evaluate(() =>
    !document.querySelector('#ranges [data-head="feature"]').hidden && document.querySelector('#ranges [data-stretch=week]').hidden));
  t.step('pick it, then what to compare it with');
  await page.click('#ranges [data-head="feature"]');
  await page.waitForFunction(() => /Compare feature with/.test(document.querySelector('#ranges').textContent));
  t.ok('the default branch is offered first, and says why', await page.evaluate(() =>
    /the default branch/.test(document.querySelector('#ranges .choice').textContent)));
  await page.click('#ranges .choice');
  await landsOnTestsThenCode(t);
  t.ok('it opens as a comparison of the two', await page.evaluate(() =>
    /comparison\s*feature against/.test(document.querySelector('#content .panelhead').textContent)));
  t.ok('changing only what the branch did', await page.evaluate(() =>
    document.querySelectorAll('.range .files .item').length === 1));
}, { docs: ['SPECS.md#Pull requests and comparisons'] });

test('a worktree is reviewed as it is on disk, against its own last commit', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('#tests [data-test]');
  t.step('open the range picker and pick the agent\'s worktree');
  await page.click('#range');
  await page.waitForSelector('#ranges [data-head="worktree agent"]');
  await page.click('#ranges [data-head="worktree agent"]');
  await page.waitForFunction(() => /Compare worktree agent with/.test(document.querySelector('#ranges').textContent));
  t.ok('its own last commit is offered first, for its uncommitted work alone', await page.evaluate(() =>
    /its last commit, agent-work\s*its uncommitted work alone/.test(document.querySelector('#ranges .choice').textContent)));
  t.step('pick it');
  await page.click('#ranges .choice');
  await landsOnTestsThenCode(t);
  t.ok('the range is its uncommitted work, an untracked file included', await page.evaluate(filesOfRange) === 'Aagent-notes.txt+1'
    || await page.evaluate(filesOfRange));
  t.step('its documents and its data: it changes neither');
  await page.click('header .focus [data-focus=docs]');
  await page.click('#subnav [data-sub=range]');
  await page.waitForFunction(() => /It changes no document/.test(document.querySelector('.range')?.textContent || ''));
  t.ok('the documents say so', true);
  t.ok('and the breadcrumb names it', await page.evaluate(() =>
    /^worktree agent, uncommitted against [0-9a-f]{7}$/.test(document.querySelector('#range').textContent.trim()) ? true : document.querySelector('#range').textContent));
}, { docs: ['SPECS.md#Pull requests and comparisons'] });

test('an added file reads as code, with a way to its diff', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('#tests [data-test]');
  t.step('review the agent\'s worktree against its last commit, and open the file it added');
  await page.click('#range');
  await page.waitForSelector('#ranges [data-head="worktree agent"]');
  await page.click('#ranges [data-head="worktree agent"]');
  await page.waitForFunction(() => /Compare worktree agent with/.test(document.querySelector('#ranges').textContent));
  await page.click('#ranges .choice');
  await landsOnTestsThenCode(t);
  await page.click('.range .files .item');
  await page.waitForSelector('#content table.code');
  t.ok('it says the whole file is new', await page.evaluate(() =>
    document.querySelector('#content .panelhead .notice')?.textContent === 'Added: the whole file is new'));
  t.ok('and shows it as code, one column, nothing painted as inserted', await page.evaluate(() =>
    !document.querySelector('#content .sbs') && !document.querySelector('#content tr.op-insert')
    && /what the agent is doing/.test(document.querySelector('#content table.code').textContent)));
  t.ok('the breadcrumb ends in it', await page.evaluate(() => document.querySelector('#crumbfile').textContent === 'agent-notes.txt'
    && !document.querySelector('#crumbfile').hidden));
  t.step('switch to its diff');
  await page.click('#content [data-act=shown]');
  t.ok('the diff paints it inserted', await page.evaluate(() => !!document.querySelector('#content tr.op-insert')));
}, { docs: ['SPECS.md#Browser UI'] });

test('the breadcrumb names the file on screen, and nothing where there is none', async t => {
  const page = t.page;
  await t.open('#/file/long.rs');
  await page.waitForSelector('table.code td.n');
  t.ok('the file is the breadcrumb\'s last part', await page.evaluate(() =>
    document.querySelector('#crumbfile').textContent === 'long.rs' && !document.querySelector('#crumbfile').hidden));
  t.ok('and not a row of its own', await page.evaluate(() => !document.querySelector('#content .panelhead .path')));
  t.step('go to the tests');
  await page.click('header .focus [data-focus=tests]');
  await page.waitForSelector('#tests [data-test]');
  t.ok('the file has gone from it', await page.evaluate(() => document.querySelector('#crumbfile').hidden));
}, { docs: ['SPECS.md#Browser UI'] });

test('a request that takes a while puts up a loading bar, and takes it down', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('#tests [data-test]');
  t.step('a request still running after a moment');
  await page.evaluate(() => { window.__done = startLoading('/pull'); });
  t.ok('not at once', await page.evaluate(() => document.querySelector('#loading').hidden));
  await page.waitForFunction(() => !document.querySelector('#loading').hidden, { timeout: 2000 });
  t.ok('then in the middle of the screen, saying what it does', await page.evaluate(() => {
    const r = document.querySelector('#loading').getBoundingClientRect();
    const middle = Math.abs((r.left + r.right) / 2 - innerWidth / 2) < 4;
    return middle && document.querySelector('#loading .label').textContent === 'fetching the pull request…' ? true
      : document.querySelector('#loading .label').textContent;
  }));
  t.step('the answer comes');
  await page.evaluate(() => window.__done());
  t.ok('and the bar goes', await page.evaluate(() => document.querySelector('#loading').hidden));
}, { docs: ['SPECS.md#Browser UI'] });

test('the Docs and Data focuses hold the documents and the datasets', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('.e2e');
  t.step('tap Docs');
  await page.click('header .focus [data-focus=docs]');
  await page.waitForSelector('#tree .row');
  t.ok('Docs is a tree of the Markdown documents', await page.evaluate(() => {
    const files = [...document.querySelectorAll('#tree [data-file]')].map(r => r.dataset.file).join(' ');
    return files === 'README.md' ? true : files;
  }));
  t.ok('at an address of its own', await page.evaluate(() => location.hash.endsWith('/docs')), await page.evaluate(() => location.hash));
  t.step('tap Data');
  await page.click('header .focus [data-focus=data]');
  await page.waitForFunction(() => location.hash.endsWith('/data'));
  t.ok('Data is a tree of the datasets', await page.evaluate(() => {
    const files = [...document.querySelectorAll('#tree [data-file]')].map(r => r.dataset.file).join(' ');
    return files === 'data/scores.csv' ? true : files;
  }));
  t.step('open the dataset');
  await page.click('#tree [data-file="data/scores.csv"]');
  await page.waitForSelector('table.code td.n');
  t.ok('it opens in the Data focus', await page.evaluate(() => location.hash.includes('in=data')
    && document.querySelector('header .focus [data-focus=data]').classList.contains('active')), await page.evaluate(() => location.hash));
}, { docs: ['SPECS.md#Browser UI'] });

test('the Code focus lands on the summary, with the files, changes, log and review beside it', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('.e2e');
  t.step('tap Code');
  await page.click('header .focus [data-focus=code]');
  await page.waitForSelector('.overview .summary');
  t.ok('Code opens on the summary', await page.evaluate(() => location.hash === '#/summary'), await page.evaluate(() => location.hash));
  t.ok('Code holds the summary, the files, the changes, the log, the comments and the notes', await page.evaluate(() => {
    const subs = [...document.querySelectorAll('#subnav button')].map(b => b.dataset.sub).join(' ');
    return subs === 'overview explore changes log review notes' ? true : subs;
  }));
  t.ok('and each can be tapped', await page.evaluate(hittable, '#subnav [data-sub=log]'));
  t.step('tap Files');
  await page.click('#subnav [data-sub=explore]');
  await page.waitForSelector('#tree .row');
  t.ok('Files with nothing open is the tree', await page.evaluate(() => location.hash.endsWith('/tree')), await page.evaluate(() => location.hash));
  t.step('tap Tests, then Code again');
  await page.click('header .focus [data-focus=tests]');
  await page.waitForSelector('.e2e');
  await page.click('header .focus [data-focus=code]');
  await page.waitForSelector('#tree .row');
  t.ok('Code goes back to the view it was last on', await page.evaluate(() => location.hash.endsWith('/tree')), await page.evaluate(() => location.hash));
}, { docs: ['SPECS.md#Browser UI'] });

test('the agent opens from the header, over what was on screen', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('.e2e');
  t.step('tap Agent');
  await page.click('#agentbtn');
  await page.waitForSelector('#agent');
  t.ok('the agent opens at its own address', await page.evaluate(() => location.hash === '#/agent'), await page.evaluate(() => location.hash));
  t.ok('the tests are still the focus', await page.evaluate(() =>
    document.querySelector('header .focus [data-focus=tests]').classList.contains('active')));
}, { docs: ['SPECS.md#Browser UI'] });

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
  t.ok('the file menu opens', await page.evaluate(hittable, '#content .acts [data-act=wrap]'));
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

test('a backslash hides the side panel and brings it back', async t => {
  const page = t.page;
  await t.open('#/tree');
  await page.waitForSelector('#tree .row');
  t.ok('the tree is there', await page.evaluate(hittable, '#tree .row'));
  t.step('press \\');
  await page.keyboard.press('Backslash');
  t.ok('the side panel is gone', await page.evaluate(() => document.querySelector('#tree').getBoundingClientRect().width === 0));
  t.step('press \\ again');
  await page.keyboard.press('Backslash');
  t.ok('and back', await page.evaluate(hittable, '#tree .row'));
}, { docs: ['SPECS.md#Browser UI'] });

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
  await page.waitForSelector('#tests .row.failed');
  t.step('tap the failed test');
  await page.click('#tests .row.failed');
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

test('a stretch says what no end-to-end test ran, and how each test ran before it', async t => {
  const page = t.page;
  await t.open();
  await page.waitForSelector('#tests .row.failed');
  t.step('open the overview, and look at the uncommitted work');
  await page.click('#tests .row.overview');
  await page.waitForSelector('.stretch-card h3');
  await page.click('.stretch-card [data-stretch=uncommitted]');
  // The card of the last 7 days is redrawn as the uncommitted work's: wait for that one.
  await page.waitForFunction(() => /uncommitted/.test(document.querySelector('.stretch-card h3')?.textContent || '')
    && document.querySelector('.stretch-card .unran .item'), { timeout: 30000 });
  t.ok('it says how much changed code no end-to-end test ran', await page.evaluate(() => {
    const head = document.querySelector('.stretch-card .unran-head').textContent.replace(/\s+/g, ' ').trim();
    const row = document.querySelector('.stretch-card .unran .item').textContent.replace(/\s+/g, ' ').trim();
    return /^1 of 2 changed lines no end-to-end test ran/.test(head) && /src\/core\.rs\s*1 of 2 lines/.test(row) ? true : head + ' | ' + row;
  }));
  t.ok('and which test ran the rest', await page.evaluate(() =>
    /Δ1/.test(document.querySelector('#tests .row.failed .badges').textContent)));
  t.ok('recording the tests before it is a tap', await t.inView('.stretch-card .base [data-act=record]'));
  t.ok('which says what it will run', await page.evaluate(() =>
    /runs make e2e in a worktree of [0-9a-f]{7}/.test(document.querySelector('.stretch-card .base').textContent.replace(/\s+/g, ' '))));
  t.step('record the tests at the base');
  await page.click('.stretch-card .base [data-act=record]');
  await page.waitForFunction(() => /Before: the tests as they ran at/.test(document.querySelector('.stretch-card .base')?.textContent || ''), { timeout: 30000 });
  t.ok('each test says how it ran before and how it runs now', await page.evaluate(() => {
    const badges = sel => document.querySelector(sel + ' .badges').textContent.replace(/\s+/g, ' ');
    const listed = badges('#tests .row.passed'), saved = badges('#tests .row.failed');
    return /✓→✓ screen/.test(listed) && /✓→✗/.test(saved) ? true : listed + ' | ' + saved;
  }));
  t.step('back to the list, and open the test whose screen changed');
  await page.click('#content [data-act=tests]');
  await page.click('#tests .row.passed');
  await page.waitForSelector('.testview .compare ol.pairs li');
  t.ok('its steps pair up, before and now', await page.evaluate(() => {
    const rows = [...document.querySelectorAll('.testview .compare ol.pairs li')].map(li => li.querySelector('.what').textContent);
    return rows.join(',') === 'same screen,screen changed' ? true : rows.join(',');
  }));
  await page.waitForFunction(() => document.querySelectorAll('.testview .compare pre.term').length === 2
    && /none yet/.test(document.querySelector('.testview .compare .media.before pre.term').textContent || '') === false, { timeout: 10000 });
  t.step('go to the step whose screen changed');
  await page.click('.testview .compare ol.pairs li.changed');
  await page.waitForFunction(() => /none yet/.test(document.querySelector('.testview .compare .media.before pre.term').textContent)
    && /why is this pub/.test(document.querySelector('.testview .compare .media.after pre.term').textContent), { timeout: 5000 });
  t.ok('both recordings go to it', true);
  // Reading down the test put the header away, and the back button with it: a tap where it
  // was lands on the test's own title.
  t.step('scroll back up, so that the header comes back');
  await page.evaluate(() => { $('#content').scrollTop = 0; });
  await page.waitForFunction(`(${hittable})('#back') === true`, { timeout: 5000 }).catch(() => {});
  t.ok('the back button is there', await page.evaluate(hittable, '#back'));
  t.step('go back to the list, and from its overview open the code nothing ran');
  await page.click('#back');
  await page.waitForSelector('#tests .row.overview');
  await page.click('#tests .row.overview');
  await page.waitForSelector('.stretch-card .unran .item', { timeout: 30000 });
  await t.inView('.stretch-card .unran .item');
  await page.click('.stretch-card .unran .item');
  await page.waitForSelector('table.code tr.unran');
  t.ok('the file marks the changed lines no test ran', await page.evaluate(() =>
    [...document.querySelectorAll('table.code tr.unran')].map(r => r.dataset.line).join(',') === '4'
    && /no end-to-end test ran them/.test(document.querySelector('.ranbanner').textContent)));
}, { docs: ['SPECS.md#What a stretch of history did'] });

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
    /// Where this test's recording goes, for a picture of the page worth keeping beside it.
    dir,
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
      // Scrolling down puts the header away, which moves everything under it up by its
      // height over a fraction of a second: a finger taps once the page has stopped moving.
      await pause(300);
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
    // A person scrolls to what they mean to tap, and taps once the page has stopped: the
    // header goes away as the page scrolls down, moving everything under it, and a tap made
    // in the same instant as the scroll lands on whatever moved under the finger.
    await page.$eval(selector, el => {
      const r = el.getBoundingClientRect();
      if (r.top < 0 || r.bottom > innerHeight) el.scrollIntoView({ block: 'center' });
    }).catch(() => {});
    // Until it has stopped moving: two looks a beat apart that agree, or a second at most.
    let last = null;
    for (let i = 0; i < 10; i++) {
      const at = await page.$eval(selector, el => {
        const r = el.getBoundingClientRect();
        return `${Math.round(r.top)},${Math.round(r.left)}`;
      }).catch(() => null);
      if (at === null || at === last) break;
      last = at;
      await pause(100);
    }
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

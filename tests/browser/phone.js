// What the phone layout has to do, checked in a real browser at a real phone's size.
//
// Everything here is a thing that has been broken at least once by a change that looked right
// in the stylesheet. A menu clipped by its own parent's `overflow`, a header that stopped
// sticking, a form that opened between two lines of code with nowhere to put its buttons: none
// of them fail a test that reads CSS, and all of them fail this one.
//
// It runs inside the container, with the page served on the host. The one argument is the URL
// that signs a browser in.
//
// Hit testing rather than measuring, wherever there is a choice: `getBoundingClientRect` is
// unmoved by a parent that clips its child, so a menu cut off at its header's edge measures
// exactly as it would if it were on screen. `elementFromPoint` is what a finger does.

const puppeteer = require('puppeteer');

// Found rather than indexed: `node script.js <url>` and `node -e <script> <url>` number their
// arguments differently, and the harness uses the second so that nothing has to be mounted.
const url = process.argv.find(a => a.startsWith('http'));
if (!url) {
  console.error('usage: phone.js <url with token>');
  process.exit(2);
}

const results = [];
// Only `true` passes. `hittable` answers with a description of what went wrong instead of a
// bare false, and a non-empty string is truthy: taking `!!pass` here quietly turned every one
// of those descriptions into a pass, which is how this file spent its first half hour agreeing
// with a page that was broken.
function ok(name, pass, detail) {
  results.push({ name, pass: pass === true, detail: detail !== undefined ? detail : pass });
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

(async () => {
  const browser = await puppeteer.launch({ args: ['--no-sandbox'] });
  const page = await browser.newPage();
  // A small phone, upright, with a finger rather than a pointer.
  await page.setViewport({ width: 360, height: 640, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });
  page.on('pageerror', e => ok('no script errors', false, e.message));
  await page.goto(url, { waitUntil: 'networkidle0' });

  ok('the phone stylesheet applies', await page.evaluate(() => matchMedia('(max-width: 700px), (max-height: 500px)').matches));
  ok('a finger, not a pointer', await page.evaluate(() => matchMedia('(hover: none)').matches));
  ok('a repository opens on its overview', page.url().endsWith('#/'), page.url());
  ok('with what it says it is', await page.evaluate(() => {
    const el = document.querySelector('.overview .readme');
    return !!el && el.textContent.length > 40;
  }));
  // Scrolled to before it is asked about: a section below the fold is not a broken section,
  // and `hittable` rightly refuses anything off screen.
  const inView = async sel => {
    await page.evaluate(s => document.querySelector(s)?.scrollIntoView({ block: 'center' }), sel);
    return page.evaluate(hittable, sel);
  };
  ok('where to start reading', await inView('.overview .entries .item'));
  ok('and how it is doing', await page.evaluate(() => {
    const figs = [...document.querySelectorAll('.overview .fig')].map(f => f.textContent);
    return figs.length >= 4 ? true : 'only ' + JSON.stringify(figs);
  }));
  // The slow half arrives on its own, after the symbol index is built.
  await page.waitForFunction(() => {
    const el = document.querySelector('.overview .leaning');
    return el && !el.textContent.includes('reading the symbols');
  }, { timeout: 30000 });
  ok('what the tree leans on, once the symbols are read', await inView('.overview .leaning .item'));

  // The status view is still every repository at once, one tap away.
  await page.click('header .tabs [data-view=status]');
  await page.waitForSelector('.list.status .item.repo');
  ok('every repository is listed', await page.evaluate(() => !!document.querySelector('.list.status .item.repo')));
  await page.click('header .tabs [data-view=explore]');
  await page.waitForSelector('.overview');

  // The header's own menu: search, colour scheme, refresh, sign out, help. It hangs below the
  // header, which is why an `overflow: hidden` on the header once made it untappable.
  await page.click('#more');
  ok('the header menu opens', await page.evaluate(hittable, '#refresh'));
  await page.click('#refresh');

  // Files is one tap from the overview, and the tree is what it opens.
  await page.click('#content .panelhead [data-act=files]');
  await page.waitForSelector('#tree .row');
  ok('Files opens the tree', await page.evaluate(() => location.hash.endsWith('/tree')), await page.evaluate(() => location.hash));

  // The file, by its address, and its own header.
  await page.goto(url.split('#')[0] + '#/file/long.rs', { waitUntil: 'networkidle0' });
  await page.waitForSelector('table.code td.n');
  const head = await page.evaluate(() => document.querySelector('#content .panelhead').getBoundingClientRect().height);
  ok('the file header is one row', head < 60, head + 'px');
  ok('Comment is on it', await page.evaluate(hittable, '#content .panelhead [data-act=comment]'));
  ok('and the rest is behind one button', await page.evaluate(hittable, '#content .panelhead [data-act=menu]'));

  await page.click('#content .panelhead [data-act=menu]');
  ok('the file menu opens', await page.evaluate(hittable, '#content .acts [data-act=zen]'));

  // A header that scrolls away with the code takes the button that opens that menu with it.
  await page.evaluate(() => { document.querySelector('#content .acts').classList.remove('open'); $('#content').scrollTop = 600; });
  await new Promise(r => setTimeout(r, 200));
  const stuck = await page.evaluate(() => {
    const head = document.querySelector('#content .panelhead').getBoundingClientRect();
    const content = document.querySelector('#content').getBoundingClientRect();
    return Math.round(head.top - content.top);
  });
  ok('the file header sticks while the code scrolls', Math.abs(stuck) <= 2, stuck + 'px from the top');
  ok('and its menu button is still there', await page.evaluate(hittable, '#content .panelhead [data-act=menu]'));

  // Writing a comment takes over the screen rather than opening between two lines of code.
  await page.click('table.code tr[data-line="40"] td.n');
  await page.waitForSelector('#content tr.form .form');
  const form = await page.evaluate(() => {
    const r = document.querySelector('#content tr.form .form').getBoundingClientRect();
    return { w: Math.round(r.width), h: Math.round(r.height) };
  });
  ok('the comment form covers the screen', form.w >= 350 && form.h >= 400, JSON.stringify(form));
  ok('its save button is reachable', await page.evaluate(hittable, '#content tr.form [data-act=save]'));
  ok('and so is cancel, below the keyboard', await page.evaluate(hittable, '#content tr.form [data-act=cancel]'));
  await page.click('#content tr.form [data-act=cancel]');

  // Zen leaves the file and one way back.
  await page.click('#content .panelhead [data-act=menu]');
  await page.click('#content .acts [data-act=zen]');
  const gone = await page.evaluate(() => ['header', 'footer', '#content .panelhead']
    .filter(s => { const el = document.querySelector(s); return el && el.getBoundingClientRect().height > 0; }));
  ok('zen takes the frame away', gone.length === 0, 'still showing: ' + gone.join(', '));
  ok('and leaves the way back', await page.evaluate(hittable, '#zen'));
  ok('the code is still there', await page.evaluate(() => !!document.querySelector('table.code td.c')));
  await page.click('#zen');

  // Long lines wrap where the screen ends instead of running off it.
  const before = await page.evaluate(() => $('#content').scrollWidth > $('#content').clientWidth);
  ok('a long file scrolls sideways to begin with', before);
  await page.click('#content .panelhead [data-act=menu]');
  await page.click('#content .acts [data-act=wrap]');
  await new Promise(r => setTimeout(r, 200));
  const after = await page.evaluate(() => $('#content').scrollWidth <= $('#content').clientWidth + 1);
  ok('and does not once lines wrap', after);

  await browser.close();

  let failed = 0;
  for (const r of results) {
    if (!r.pass) failed++;
    console.log((r.pass ? 'ok   ' : 'FAIL ') + r.name + (r.pass ? '' : '  (' + r.detail + ')'));
  }
  console.log(`${results.length - failed} of ${results.length} checks passed`);
  process.exit(failed ? 1 : 0);
})().catch(e => {
  console.error('the browser could not finish: ' + (e && e.message));
  process.exit(1);
});

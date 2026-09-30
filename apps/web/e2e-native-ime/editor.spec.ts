import { chromium, expect, test } from '@playwright/test';
import { join } from 'node:path';
import { writeFileSync } from 'node:fs';
import { admin, createDoc, editorOf, expectBlocks, openDoc, savedBody, workspaceId } from '../e2e/workspace-wiki-vue-editor';
import { clickNative, evidence, focusNative, keys, observe, snapshot } from './native';

const cases = process.env.FVOCI_NATIVE_IME_CASES ?? 'first-jamo,composition-enter,backspace,plain-commit';
test(`OS IBus Hangul [${cases}]: save and persisted reload`, async ({ baseURL }) => {
  test.setTimeout(60_000);
  const profile = join(evidence,'chrome-profile');
  const context = await chromium.launchPersistentContext(profile, {
    baseURL, headless:false, viewport:null, env:{...process.env,TMPDIR:'.'},
    args:['--ozone-platform=x11','--no-first-run','--no-default-browser-check'],
  });
  const db=new URL(process.env.DATABASE_APP_URL!);
  writeFileSync(join(evidence,'group-ownership.json'),JSON.stringify({
    baseURL,display:process.env.DISPLAY,profile,browserVersion:context.browser()?.version(),
    postgresContainer:process.env.FVOCI_TEST_PG_CONTAINER,
    meiliContainer:process.env.FVOCI_TEST_MEILI_CONTAINER,
    db:{role:db.username,host:db.hostname,port:db.port,database:db.pathname},
    storage:process.env.FVOCI_STORAGE_DIR,static:process.env.FVOCI_STATIC_DIR,
    group:process.env.FVOCI_E2E_RESULT_DIR,server:process.env.FVOCI_E2E_SERVER_BIN,
    collaboration:process.env.FVOCI_COLLAB_ENGINE,cases,
  },null,2));
  try {
    // HTTP fixture setup, real Rust session cookie shared by this context.
    // No CDP/DOM/key input generates test text or composition.
    const setup = await context.request.post('/api/v1/setup',{data:admin});
    expect(setup.status(),await setup.text()).toBe(201);
    const page = context.pages()[0];
    const ws = await workspaceId(context.request);
    let initial = true;
    const scenarios = cases.split(',');
    for (const scenario of scenarios) {
      if(!['first-jamo','composition-enter','backspace','plain-commit'].includes(scenario)) throw new Error('Unknown native IME scenario '+scenario);
      const doc = await createDoc(context.request,ws,'Native OS Hangul '+scenario,{markdown:''});
      await openDoc(page,doc.path);
      await observe(page);
      const editor = editorOf(page);
      await focusNative(page,editor,profile);
      if(initial) { keys('Shift+space'); initial=false; }
      keys('g'); await page.waitForTimeout(120);
      await snapshot(page,scenario+'-first-jamo');
      expect(await page.evaluate(() => (window as any).nativeImeEvents.some((e:any) => e.type==='compositionupdate' && e.data==='ㅎ'))).toBe(true);
      let expected: string[];
      if(scenario==='first-jamo') { keys('space'); expected=['ㅎ ']; }
      else {
        keys('k','s'); await page.waitForTimeout(120);
        await snapshot(page,scenario+'-preedit');
        if(scenario==='backspace') {
          keys('BackSpace'); await page.waitForTimeout(120); await snapshot(page,scenario+'-deleted');
          expect(await editor.textContent()).toBe('하');
          keys('s','space'); expected=['한 '];
        } else {
          keys('r','m','f'); await page.waitForTimeout(120); await snapshot(page,scenario+'-hangul');
          if(scenario==='composition-enter') {
            keys('Return'); await page.waitForTimeout(120);
            await snapshot(page,scenario+'-first-enter');
            // This isolated IBus/Chrome path ends preedit BEFORE delivering
            // non-composing Enter (keyCode 13). The plain contenteditable
            // baseline inserts a break on that same first key; pin that local
            // behavior without claiming a contract for other OS IMEs.
            await expectBlocks(page,['한글','']);
            keys('Return'); expected=['한글','',''];
          } else { keys('space'); expected=['한글 ']; }
        }
      }
      await expectBlocks(page,expected);
      await snapshot(page,scenario+'-committed');
      expect(await page.evaluate(() => (window as any).nativeImeEvents.some((e:any) => e.type==='compositionend'))).toBe(true);
      await clickNative(page,page.getByRole('button',{name:'저장',exact:true}));
      await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible({timeout:15_000});
      const stored = await savedBody(context.request,ws,doc.id);
      writeFileSync(join(evidence,scenario+'-persisted.json'),JSON.stringify({doc,expected,stored},null,2));
      expect(stored.content?.map(node => (node.content??[]).map(child => child.text??'').join(''))).toEqual(expected);
      await openDoc(page,doc.path);
      await expectBlocks(page,expected);
      await page.screenshot({path:join(evidence,scenario+'-reloaded.png')});
    }
  } finally {
    try { if(context.pages()[0]) await snapshot(context.pages()[0],'final-state'); }
    finally { await context.close(); }
  }
});

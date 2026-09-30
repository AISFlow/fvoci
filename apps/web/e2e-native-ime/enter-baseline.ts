import { chromium } from '@playwright/test';
import { join } from 'node:path';
import { writeFileSync } from 'node:fs';
import { evidence, focusNative, keys, observe, snapshot } from './native';
const profile = join(evidence,'chrome-profile');
const context = await chromium.launchPersistentContext(profile, {
  headless:false, viewport:null, env:{...process.env,TMPDIR:'.'},args:['--ozone-platform=x11'],
});
try {
  const page=context.pages()[0];
  await page.goto('data:text/html,<title>FVOCI native Enter baseline</title><style>div{font:32px sans-serif;width:800px;height:200px;margin:40px;border:1px solid black}</style><div contenteditable="true" aria-label="Owned plain contenteditable"></div>');
  const field=page.locator('[contenteditable]');
  await field.waitFor({state:'visible'});
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await observe(page); await focusNative(page,field,profile);
  keys('Shift+space'); keys('g','k','s','r','m','f');
  await page.waitForTimeout(120); await snapshot(page,'baseline-preedit');
  keys('Return'); await page.waitForTimeout(120); await snapshot(page,'baseline-first-enter');
  const first=await field.innerHTML();
  keys('Return'); await page.waitForTimeout(120); await snapshot(page,'baseline-second-enter');
  const second=await field.innerHTML();
  const result={first,second,events:await page.evaluate(()=>(window as any).nativeImeEvents)};
  writeFileSync(join(evidence,'baseline-enter-result.json'),JSON.stringify(result,null,2));
  if(!result.events.some((e:any)=>e.type==='compositionupdate' && e.data==='글'))throw new Error('No native Hangul preedit');
  console.log(JSON.stringify(result));
}finally{await context.close();}

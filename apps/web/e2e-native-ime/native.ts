import { execFileSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { Page, Locator } from '@playwright/test';

export const evidence = process.env.FVOCI_NATIVE_IME_SESSION!;
export function native(command: string, args: string[]) {
  if (args.includes('--window')) throw new Error('XSendEvent targeting is forbidden');
  const out = execFileSync(command, args, { encoding: 'utf8' });
  writeFileSync(join(evidence, 'native-commands.jsonl'), JSON.stringify({ time: new Date().toISOString(), command, args, out })+'\n', { flag: 'a' });
  return out.trim();
}
export function keys(...keys: string[]) { native('xdotool', ['key', '--delay', '180', ...keys]); }
export async function observe(page: Page) {
  await page.evaluate(() => {
    (window as any).nativeImeEvents = [];
    for (const type of ['keydown','keyup','compositionstart','compositionupdate','compositionend','beforeinput','input']) {
      document.addEventListener(type, (e: any) => {
        const record = {
        type, time: performance.now(), key: e.key, code: e.code, keyCode:e.keyCode, which:e.which, data: e.data,
        inputType: e.inputType, isComposing: e.isComposing, isTrusted: e.isTrusted,
        defaultPrevented: e.defaultPrevented, defaultPreventedAfterDispatch:e.defaultPrevented,
        value: e.target.value ?? e.target.textContent,
        };
        (window as any).nativeImeEvents.push(record);
        // Native event dispatch can run microtasks between listeners; observe
        // cancellation in the next task, after all editor listeners ran.
        setTimeout(() => { record.defaultPreventedAfterDispatch=e.defaultPrevented; },0);
      }, true);
    }
  });
}
export async function snapshot(page: Page, name: string) {
  writeFileSync(join(evidence, name+'.json'), JSON.stringify(await page.evaluate(() => ({
    url:location.href,
    events: (window as any).nativeImeEvents,
    active: document.activeElement?.outerHTML,
    selection: window.getSelection()?.toString(),
  })), null, 2));
  await page.screenshot({ path: join(evidence, name+'.png') });
}
export async function focusNative(page: Page, field: Locator, profile: string) {
  const candidates = JSON.parse(execFileSync('python3',['-c', `
import pathlib,json,sys,shlex
matches=[]
for p in pathlib.Path('/proc').iterdir():
 if not p.name.isdigit():continue
 try:
  args=(p/'cmdline').read_bytes().decode().split('\\0')
  if len([a for a in args if a])==1:args=shlex.split(args[0])
  if '--user-data-dir='+sys.argv[1] in args and not any(x.startswith('--type=') for x in args):
   env=dict(x.split('=',1) for x in (p/'environ').read_bytes().decode().split('\\0') if '=' in x)
   matches.append({'pid':p.name,'args':args,'exe':str((p/'exe').resolve()),'display':env.get('DISPLAY')})
 except (OSError,UnicodeError):pass
print(json.dumps(matches))
`,profile],{encoding:'utf8'}));
  if (candidates.length !== 1) throw new Error('Ambiguous owned browser process: '+JSON.stringify(candidates));
  if(!candidates[0].exe.endsWith('/chrome-linux64/chrome')) throw new Error('Owned Chrome executable mismatch');
  // Chromium rewrites its process title and /proc/environ may be empty.
  // Verify the browser on this X connection by its XID and _NET_WM_PID.
  if(candidates[0].display && candidates[0].display!==process.env.DISPLAY) throw new Error('Owned Chrome display mismatch');
  const { pid, args } = candidates[0];
  const windows = native('xdotool', ['search','--onlyvisible','--pid',pid]).split('\n').filter(Boolean);
  const title = await page.title();
  const owned = windows.filter(id => native('xdotool',['getwindowname',id]).startsWith(title+' - '));
  if (owned.length !== 1) throw new Error('Ambiguous Chrome XID: '+JSON.stringify(windows));
  const xid = owned[0];
  const windowProperties=native('xprop',['-id',xid,'_NET_WM_PID','WM_CLASS','WM_NAME']);
  if(!windowProperties.includes(`_NET_WM_PID(CARDINAL) = ${pid}`)) throw new Error('XID PID does not match owned Chrome');
  writeFileSync(join(evidence, 'browser-ownership.json'), JSON.stringify({ ...candidates[0], displayFromProc:candidates[0].display, xid, windows, profile, display: process.env.DISPLAY, displayProof:'owned PID and XID queried through this private X connection', windowProperties }, null,2));
  native('xdotool',['windowfocus','--sync',xid]);
  await snapshot(page, 'before-focus');
  await clickNative(page,field);
  if (!await field.evaluate(el => el === document.activeElement)) throw new Error('Native click did not focus field');
}
export async function clickNative(page: Page, field: Locator) {
  const rect = await field.boundingBox();
  if (!rect) throw new Error('No visible field');
  const frame = await page.evaluate(() => ({x:window.screenX,y:window.screenY,top:window.outerHeight-window.innerHeight}));
  native('xdotool',['mousemove',String(Math.round(frame.x+rect.x+8)),String(Math.round(frame.y+frame.top+rect.y+12))]);
  native('xdotool',['click','1']);
}

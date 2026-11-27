import { readdirSync, statSync } from 'node:fs';
import path from 'node:path';

import { expect, test, type Page } from '@playwright/test';

/**
 * Which font files a first screen asks for, per language.
 *
 * The page under test is not the app (its shell shows two words) but a page
 * of ordinary text in one language, styled by the app's built style sheets
 * and nothing else: the main one, and the `@font-face` rules of every family,
 * which is what the app has loaded once the browser has been idle.
 *
 * Local fonts are excluded from this download probe so installed families
 * cannot bypass the files and byte counts it verifies.
 */
const STATIC = path.resolve(import.meta.dirname, '../dist/static');
const sheets = readdirSync(STATIC).filter((file) => /^(index|faces)-.*\.css$/.test(file));

const SAMPLES = {
  en: 'Board “Roadmap” — 12 cards, 3 done… Due 2026-03-05 (Thu) at 9:30 AM; it’s 40% complete.',
  ko: '보드 “로드맵” — 카드 12장 가운데 3장을 끝냈습니다… 마감은 2026-03-05(목) 오전 9:30이고, 진행률은 40%입니다. ※ 담당자: 김하늘 → 이도윤',
  ja: 'ボード「ロードマップ」— カード12枚のうち3枚が完了しました… 期限は2026-03-05（木）午前9:30、進捗は40%です。担当者：佐藤 → 鈴木',
  'zh-Hans':
    '看板“路线图” — 12张卡片中已完成3张… 截止日期为2026-03-05（周四）上午9:30，进度40%。负责人：王伟 → 李娜',
  'zh-Hant':
    '看板「路線圖」— 12張卡片中已完成3張… 截止日期為2026-03-05（週四）上午9:30，進度40%。負責人：王偉 → 李娜',
} as const;

interface FontRequests {
  files: string[];
  bytes: number;
}

async function fontsFor(page: Page, language: string, text: string): Promise<FontRequests> {
  const links = sheets.map((file) => `<link rel="stylesheet" href="./static/${file}">`).join('');
  await page.route('**/font-probe', (route) =>
    route.fulfill({
      contentType: 'text/html; charset=utf-8',
      body: `<!doctype html><html lang="${language}"><head><meta charset="utf-8">${links}</head>
        <body><p>${text}</p><p><strong>${text}</strong></p></body></html>`,
    }),
  );
  await page.route('**/static/*.css', async (route) => {
    const response = await route.fetch();
    const body = (await response.text()).replace(/local\([^)]*\),?/g, '');
    await route.fulfill({ response, body });
  });
  const requests: FontRequests = { files: [], bytes: 0 };
  page.on('response', (response) => {
    const file = new URL(response.url()).pathname.replace(/^.*\//, '');
    if (!file.endsWith('.woff2') || requests.files.includes(file)) return;
    // The second weight range of a family names the same file; it comes from the cache.
    requests.files.push(file);
    // A font file is sent as it is on disk: WOFF2 is compressed already.
    requests.bytes += statSync(path.join(STATIC, file)).size;
  });
  await page.goto('./font-probe');
  await page.evaluate(() => document.fonts.ready);
  await page.waitForLoadState('networkidle');
  requests.files = requests.files.map((file) => file.replace(/-[0-9a-f]{16}/, '')).sort();
  return requests;
}

const measured: Record<string, FontRequests> = {};

test.describe('font files of a first screen', () => {
  test.beforeEach(({ page: _page }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'the same files on every device; measured once');
  });

  test('English needs the common file of the Latin family', async ({ page }) => {
    measured.en = await fontsFor(page, 'en', SAMPLES.en);
    expect(measured.en.files).toEqual(['noto-sans-common.woff2']);
  });

  test('Korean needs one file: its own family draws the Latin text too', async ({ page }) => {
    measured.ko = await fontsFor(page, 'ko', SAMPLES.ko);
    expect(measured.ko.files).toEqual(['noto-sans-kr-common.woff2']);
  });

  test('a Korean syllable outside KS X 1001 adds the file of the other syllables, not the rest', async ({
    page,
  }) => {
    measured['ko + rare syllable'] = await fontsFor(page, 'ko', `${SAMPLES.ko} 똠얌꿍, 쌰오롱바오`);
    expect(measured['ko + rare syllable'].files).toEqual([
      'noto-sans-kr-common.woff2',
      'noto-sans-kr-syllables.woff2',
    ]);
  });

  test('Japanese needs one file as well', async ({ page }) => {
    measured.ja = await fontsFor(page, 'ja', SAMPLES.ja);
    expect(measured.ja.files).toEqual(['noto-sans-jp-common.woff2']);
  });

  test('Chinese keeps the Latin family first and needs two files', async ({ page }) => {
    measured['zh-Hans'] = await fontsFor(page, 'zh-Hans', SAMPLES['zh-Hans']);
    expect(measured['zh-Hans'].files).toEqual([
      'noto-sans-common.woff2',
      'noto-sans-sc-common.woff2',
    ]);
  });

  test('traditional Chinese takes its own family for the Han characters', async ({ page }) => {
    measured['zh-Hant'] = await fontsFor(page, 'zh-Hant', SAMPLES['zh-Hant']);
    expect(measured['zh-Hant'].files).toEqual([
      'noto-sans-common.woff2',
      'noto-sans-tc-common.woff2',
    ]);
  });

  test('Korean and Japanese put their own family first, the others the Latin one', async ({
    page,
  }) => {
    const familyOf = async (language: string): Promise<string> => {
      await fontsFor(page, language, '…');
      return page.evaluate(() =>
        (getComputedStyle(document.body).fontFamily.split(',')[0] ?? '').replace(
          /^['"]|['"]$/g,
          '',
        ),
      );
    };
    expect(await familyOf('ko')).toContain('Noto Sans KR');
    expect(await familyOf('ja')).toContain('Noto Sans JP');
    expect(await familyOf('en')).toBe('Noto Sans');
    expect(await familyOf('zh-Hans')).toBe('Noto Sans');
  });

  test.afterAll(() => {
    // The numbers for docs/notes.md: `pnpm --filter web run e2e` prints them.
    for (const [language, { files, bytes }] of Object.entries(measured)) {
      process.stdout.write(
        `fonts ${language}: ${files.length} request(s), ${bytes} bytes: ${files.join(', ')}\n`,
      );
    }
  });
});

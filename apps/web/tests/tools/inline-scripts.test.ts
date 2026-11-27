/** The hashes a content security policy allows the inline script of index.html by. */
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import {
  contentSecurityPolicy,
  inlineScripts,
  scriptHash,
} from '../../tools/build/inline-scripts.ts';

describe('the inline scripts of a page', () => {
  it('are the script elements without a src, with their text exactly as written', () => {
    const html = [
      '<script>one();</script>',
      '<script type="module" src="/static/app.js"></script>',
      '<script>\n  two();\n</script>',
    ].join('\n');
    expect(inlineScripts(html)).toEqual(['one();', '\n  two();\n']);
  });

  it('hash to the source expression a policy takes', () => {
    // The example of the Content Security Policy specification.
    expect(scriptHash("alert('Hello, world.');")).toBe(
      "'sha256-qznLcsROx4GACP2dm0UCKCzCG+HiZ1guq6ZZDob/Tng='",
    );
    expect(scriptHash('가')).toBe(
      `'sha256-${createHash('sha256').update(Buffer.from('가', 'utf8')).digest('base64')}'`,
    );
  });

  it('finds exactly one in index.html: the one that sets the theme before the first paint', () => {
    const html = readFileSync(path.resolve(import.meta.dirname, '../../index.html'), 'utf8');
    const scripts = inlineScripts(html);
    expect(scripts).toHaveLength(1);
    expect(scripts[0]).toContain("localStorage.getItem('fvoci:display')");
  });

  it('go into a policy that allows no other inline script and no eval', () => {
    const policy = contentSecurityPolicy(["'sha256-abc='"]);
    const scripts = policy.split('; ').find((part) => part.startsWith('script-src'));
    expect(scripts).toBe("script-src 'self' 'sha256-abc='");
    expect(policy).toContain("object-src 'none'");
    expect(policy).toContain("frame-ancestors 'none'");
  });
});

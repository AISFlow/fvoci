import { createHmac } from "node:crypto";

const STEP_SECONDS = 30;

export function base32Decode(input: string): Buffer {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  const clean = input.replace(/[\s=]/g, "").toUpperCase();
  let bits = 0;
  let value = 0;
  const out: number[] = [];
  for (const char of clean) {
    const index = alphabet.indexOf(char);
    if (index < 0) {
      throw new Error(`invalid base32 character: ${char}`);
    }
    value = (value << 5) | index;
    bits += 5;
    if (bits >= 8) {
      out.push((value >>> (bits - 8)) & 0xff);
      bits -= 8;
    }
  }
  return Buffer.from(out);
}

// RFC 6238 TOTP: HMAC-SHA1, 30 s step, 6 digits.
export function totp(secret: string, step: number): string {
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(step));
  const digest = createHmac("sha1", base32Decode(secret)).update(counter).digest();
  const offset = digest.readUInt8(digest.length - 1) & 0x0f;
  const binary =
    ((digest.readUInt8(offset) & 0x7f) << 24) |
    (digest.readUInt8(offset + 1) << 16) |
    (digest.readUInt8(offset + 2) << 8) |
    digest.readUInt8(offset + 3);
  return String(binary % 1000000).padStart(6, "0");
}

export function currentStep(): number {
  return Math.floor(Date.now() / 1000 / STEP_SECONDS);
}

// The server rejects a second use of a time step but accepts one step of
// clock drift either way, so the step after `usedStep` is valid right away.
export function freshCode(secret: string, usedStep: number): string {
  return totp(secret, Math.max(currentStep(), usedStep + 1));
}

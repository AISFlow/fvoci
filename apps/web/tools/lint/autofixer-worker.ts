import { readFileSync } from 'node:fs';
import path from 'node:path';
import { parentPort, workerData } from 'node:worker_threads';

import { checkSvelteCode } from './autofixer.ts';

interface WorkerInput {
  root: string;
  svelteVersion: string;
  files: string[];
}

const { root, svelteVersion, files } = workerData as WorkerInput;
const problems: string[] = [];

for (const file of files) {
  const code = readFileSync(path.join(root, file), 'utf8');
  problems.push(...(await checkSvelteCode(file, code, svelteVersion)));
}

parentPort?.postMessage({ problems });

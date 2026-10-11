// A lane-driver stand-in for the runner's interrupt tests. It logs
// "ready <pid>" once its SIGINT handler is installed. On SIGINT it takes a
// moment (a fixture wrapper's docker rm), records <marker> and exits 130; with
// "ignore" it ignores SIGINT and waits to be killed.
import { writeFileSync, writeSync } from "node:fs";
import process from "node:process";

const [marker, mode] = process.argv.slice(2) as [string, string | undefined];
setInterval(() => {
  // Alive until signalled.
}, 1 << 30);
process.on("SIGINT", () => {
  if (mode === "ignore") return;
  setTimeout(() => {
    writeFileSync(marker, "cleaned\n");
    process.exit(130);
  }, 300);
});
writeSync(1, "ready " + String(process.pid) + "\n");

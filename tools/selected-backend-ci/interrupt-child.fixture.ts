import { writeSync } from "node:fs";
import process from "node:process";
import { MessageChannel } from "node:worker_threads";

const channel = new MessageChannel();
channel.port1.on("message", () => {
  // Keep the readiness fixture alive without a timer or another child.
});
channel.port1.ref();
process.on("SIGINT", () => {
  // Deliberately require SIGKILL rather than graceful SIGINT handling.
});
writeSync(1, String(process.pid) + "\n");
if (process.argv[2] === "interrupt-parent") process.kill(process.ppid, "SIGINT");

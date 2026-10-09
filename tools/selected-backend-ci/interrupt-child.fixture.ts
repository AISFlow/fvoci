import { writeSync } from "node:fs";
import process from "node:process";
import { MessageChannel } from "node:worker_threads";

// Bun keeps the event loop alive only for a MessagePort the script actually
// retains. A ref on port1 alone lets this process exit 0 a few milliseconds
// after the ready write, so a parent abort under load observes no signal.
const channel = new MessageChannel();
for (const port of [channel.port1, channel.port2]) {
  port.on("message", () => {
    // Keep the readiness fixture alive without a timer or another child.
  });
  port.ref();
}
process.on("SIGINT", () => {
  // Deliberately require SIGKILL rather than graceful SIGINT handling.
});
writeSync(1, String(process.pid) + "\n");
if (process.argv[2] === "interrupt-parent") process.kill(process.ppid, "SIGINT");

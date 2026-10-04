import { StringDecoder } from "node:string_decoder";

/** Read only this fixture's complete Rust listening line. Pipes may divide any
 * byte boundary; a line on stdout must never finish a partial stderr address. */
export function createRustReadinessReader(onReady: (url: string) => void) {
  const streams = {
    stdout: { decoder: new StringDecoder("utf8"), pending: "", discarding: false },
    stderr: { decoder: new StringDecoder("utf8"), pending: "", discarding: false },
  };
  let ready = false;
  return (stream: "stdout" | "stderr", bytes: Buffer): void => {
    if (ready) return;
    const state = streams[stream];
    const parts = state.decoder.write(bytes).split("\n");
    for (let i = 0; i < parts.length; i++) {
      if (!state.discarding) state.pending += parts[i] ?? "";
      // Retain the previous startup reader's bounded 8192-character memory,
      // without turning the tail of an oversized line into a new log line.
      if (state.pending.length > 8192) {
        state.pending = "";
        state.discarding = true;
      }
      if (i === parts.length - 1) break;
      const match = state.discarding
        ? null
        : /^fvoci-server listening on (http:\/\/127\.0\.0\.1:([1-9]\d{0,4}))\r?$/.exec(
            state.pending,
          );
      state.pending = "";
      state.discarding = false;
      const url = match?.[1];
      const port = Number(match?.[2]);
      if (url && port <= 65535) {
        ready = true;
        onReady(url);
        return;
      }
    }
  };
}

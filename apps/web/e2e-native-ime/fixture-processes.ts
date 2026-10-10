// Records every process carrying this run's FVOCI_NATIVE_IME_SESSION into
// <session>/fixture-processes.json before the test command starts.
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { asciiJson, sessionProcesses } from "./proc";

const session = process.env.FVOCI_NATIVE_IME_SESSION;
if (!session) throw new Error("FVOCI_NATIVE_IME_SESSION is required");
writeFileSync(join(session, "fixture-processes.json"), asciiJson(sessionProcesses(session)));

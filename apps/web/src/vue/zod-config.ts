import { z } from "zod";

// The app CSP allows no eval. Zod 4 probes `new Function` when it builds its
// first object schema, and the browser reports that as a script-src violation
// even though Zod catches the throw. Without JIT, Zod parses the same way, so
// turn it off before any schema exists.
z.config({ jitless: true });

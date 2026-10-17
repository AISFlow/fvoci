export type NativeArchitecture = "x86_64" | "aarch64";

export function nativeArchitecture(value: string): NativeArchitecture {
  if (["x64", "x86_64", "amd64"].includes(value)) return "x86_64";
  if (["arm64", "aarch64"].includes(value)) return "aarch64";
  throw new Error("unsupported native architecture");
}

/** Linux native bytes only; a translated runtime or cross target is never a packet producer. */
export function nativeLinux(system: string, runtime: string, kernel: string, target?: string) {
  const machine = nativeArchitecture(kernel);
  const triple = machine + "-unknown-linux-gnu";
  if (
    system !== "linux" ||
    nativeArchitecture(runtime) !== machine ||
    (target !== undefined && target !== triple)
  )
    throw new Error("native Linux runtime, kernel and Rust target must match");
  const directory = "/lib/" + machine + "-linux-gnu";
  return {
    machine,
    target: triple,
    abi: [
      machine === "x86_64" ? "/lib64/ld-linux-x86-64.so.2" : "/lib/ld-linux-aarch64.so.1",
      ...["libc.so.6", "libm.so.6", "libgcc_s.so.1"].map((name) => directory + "/" + name),
    ],
  };
}

export interface Inputs {
  head: string;
  tree: string;
  status: string;
  tracked: Record<string, string>;
  external: Record<string, string>;
  untracked: Record<string, string>;
}
export interface Reference {
  path: string;
  sha256: string;
}
export interface Artifact {
  reason: string;
  executable: string | null;
  target: { name: string; [key: string]: unknown };
  profile: { test: boolean; [key: string]: unknown };
  features: string[];
  fresh: boolean;
  filenames: string[];
}
export interface Binary {
  sha256: string;
  bytes: number;
  mode: string;
  inode: number;
  compiledSource: string;
  targetTriple: string;
  target: Artifact["target"];
  features: string[];
  profile: Artifact["profile"];
  cargo_fresh: boolean;
}
export interface Bundle {
  source: string;
  tree: string;
  full_inputs_unchanged: boolean;
  binaries: Record<string, Binary>;
  compiler_artifacts: Artifact[];
}
export interface Web {
  source: string;
  tree: string;
  exit_code: number;
  full_inputs_unchanged: boolean;
  dist_files: Record<string, string>;
  servedDist: string;
}
export interface Abi {
  currentSource: string;
  currentServerSha256: string;
  currentELFDependenciesVerified: boolean;
  actualCurrentELFldd: Record<string, string>;
  host_runtime_files: Record<string, string>;
}
export interface Browser {
  bun: Reference;
  chromium: Reference;
  chromium_directory_files: Record<string, string>;
}
export interface Stage {
  source: string;
  tree: string;
  command: string[];
  exit_code: number;
  seconds: number;
  compilerMessages?: Reference;
}
export type Lane = "install" | "postgres" | "sqlite";
export type Flow = "on" | "off";
export interface DriverReceipt {
  source?: string;
  tree?: string;
  root_owner?: string;
  final_exit_code?: number;
  owned_container_absent?: boolean;
  actual_tests?: number;
  actual_owned_process_receipts?: number;
  selected_flow?: string;
  cleanup_errors?: unknown[];
  owned_loopback_port_closed?: boolean;
  recorded_process_identities_retired?: boolean;
  current_schema_server_restart?: { restartBrowserExit: number };
  actual_browser_tests?: number;
  retries?: number;
  all_owned_fixtures_closed?: boolean;
  original_driver_failure?: unknown;
  driver_error?: unknown;
  failed_phase?: string;
  [key: string]: unknown;
}
export interface Retirement {
  qualified: boolean;
  receiptPresent: boolean;
  refusalCodes: string[];
  receiptSha256: string | null;
  originalFailureSha256: string | null;
  failedPhase: string | null;
}
export interface RunResult {
  lane: Lane;
  flow: Flow;
  exit: number;
  actualSource: string;
  runRoot: string;
  retirement?: Retirement;
}
export interface Aggregate {
  source: string;
  tree: string;
  owner: string;
  runs: RunResult[];
  exit: number;
  allRequestedRunsExecuted: boolean;
  launcherFailure: unknown;
}
export interface LocalGrant {
  schema: number;
  status: string;
  executionMode: string;
  exclusiveLocalBatch: boolean;
  currentDispatchConfirmed: boolean;
  owner: string;
  runId: string;
  dispatchId: string;
  taskId: string;
  workerTerminal: string;
  rootTerminal: string;
  worktree: string;
  uid: number;
  gid: number;
  source: string;
  tree: string;
  allowedModes: string[];
  expiresUtc: string;
  outputRoot: string;
  registrationHashes: Record<string, string>;
  stageCommands: Record<string, string[]>;
  runtimeAllocation?: Reference;
  purpose?: "equal-work" | "auth-session" | "turso-ui" | "keycloak";
}

/** Controller-issued local consumer allocation; never an endpoint credential. */
export interface LocalConsumerGrant extends LocalGrant {
  [key: string]: unknown;
  purpose: "turso-ui" | "keycloak";
  heavy: Reference;
  inputSha256: string;
  build?: { manifest: Reference; physical: Reference };
  localLibsql?: {
    binary: Reference;
    directory: string;
    label: string;
    host: "127.0.0.1";
    port: 0;
    tls: "generated-private-san-ip-loopback";
    childOnlyTrust: true;
  };
  canonicalRuntime?: {
    imageId: string;
    networkAuthorized: true;
    shellProof: Reference;
  };
  keycloak?: {
    backends: ("postgres" | "sqlite" | "libsql-remote")[];
    sqld?: Reference;
    heavy: string;
    chromiumTmp?: string;
    sso?: Reference;
    processObserver?: {
      directory: string;
      bun: Reference;
      module: Reference;
      config: Reference;
    };
  };
}
export interface LocalRuntimeAccess {
  preparation_sha256: string;
  preparation_uid: number;
  preparation_gid: number;
  runtime_sha256: string;
  runId: string;
  dispatchId: string;
  taskId: string;
  workerTerminal: string;
  rootTerminal: string;
  expiresUtc: string;
}
export interface AccessReceipt {
  local_allocation?: LocalRuntimeAccess;
  runner_uid?: number;
  runner_gid?: number;
  source: string;
  tree: string;
  owner: string;
  runtime_uid: number;
  runtime_gid: number;
  groups: number[];
  preflight_exit: number;
  preflight: { missing: number };
}

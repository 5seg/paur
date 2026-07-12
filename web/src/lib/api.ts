// paur UI: thin HTTP client for the daemon's /api/v1 surface.
// All requests go to the same origin; the dev server proxies /api/*
// to the daemon, and the production deployment fronts the static
// bundle with the daemon (or Caddy) so /api/* resolves locally.

export type BuildStatus =
  | 'queued'
  | 'running'
  | 'success'
  | 'failed'
  | 'cancelled';

export interface LatestBuild {
  id: number;
  /** 1-based per-package sequence number; preferred for UI labels. */
  seq: number;
  status: string;
  pkg_version: string | null;
  finished_at: number | null;
  exit_code: number | null;
}

export interface PackageBuildFlags {
  low_memory: boolean;
  rust_codegen_units_1: boolean;
  no_ccache: boolean;
  /**
   * x86-64 microarchitecture level. `null`/missing means "use the
   * container's default makepkg.conf" (generic x86-64). Set to
   * `"v3"` / `"v4"` to opt the package into CachyOS-style
   * `-march=x86-64-vN` builds. Older daemons that pre-date this
   * field omit it; the UI renders it as "default" in that case.
   */
  march?: MarchLevel | null;
}

export type MarchLevel = 'v3' | 'v4';

export interface Package {
  id: number;
  name: string;
  aur_url: string;
  last_known_ref: string | null;
  auto_rebuild: boolean;
  latest_build: LatestBuild | null;
  /**
   * Per-package build tuning (memory/CPU OOM countermeasures).
   * Older daemon versions (<0.x with flags) omit this; the UI
   * falls back to all-false so toggles render in their "off" state.
   */
  build_flags?: PackageBuildFlags;
}

export interface Build {
  id: number;
  package_id: number;
  /** 1-based per-package sequence number. */
  seq: number;
  status: string;
  queued_at: number;
  started_at: number | null;
  finished_at: number | null;
  exit_code: number | null;
  pkg_file: string | null;
  pkg_version: string | null;
  worker_id: string | null;
  trigger: string;
}

export interface Queue {
  queued: Build[];
  running: Build[];
}

class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

async function getJson<T>(path: string): Promise<T> {
  const r = await fetch(path, { credentials: 'include' });
  if (!r.ok) {
    const text = await r.text();
    throw new ApiError(r.status, `${r.status} ${path}: ${text}`);
  }
  return (await r.json()) as T;
}

async function sendJson<T>(
  method: 'POST' | 'DELETE' | 'PATCH',
  path: string,
  body?: unknown
): Promise<T> {
  const r = await fetch(path, {
    method,
    credentials: 'include',
    headers: body ? { 'content-type': 'application/json' } : undefined,
    body: body ? JSON.stringify(body) : undefined
  });
  if (!r.ok) {
    const text = await r.text();
    throw new ApiError(r.status, `${method} ${path} -> ${r.status}: ${text}`);
  }
  // Some endpoints return 204; handle both.
  const text = await r.text();
  return (text ? JSON.parse(text) : (undefined as unknown)) as T;
}

async function sendJsonRaw(
  method: 'POST' | 'DELETE',
  path: string,
  body?: unknown
): Promise<Response> {
  return fetch(path, {
    method,
    credentials: 'include',
    headers: body ? { 'content-type': 'application/json' } : undefined,
    body: body ? JSON.stringify(body) : undefined
  });
}

export interface AuthStatus {
  authenticated: boolean;
  password_set: boolean;
}

export const auth = {
  status: () => getJson<AuthStatus>('/api/v1/auth/status'),
  login: (username: string, password: string) =>
    sendJsonRaw('POST', '/api/v1/auth/login', { username, password }),
  logout: () => sendJsonRaw('POST', '/api/v1/auth/logout')
};

export const api = {
  health: () => getJson<string>('/api/v1/health'),
  listPackages: () => getJson<Package[]>('/api/v1/packages'),
  getPackage: (name: string) => getJson<Package>(`/api/v1/packages/${name}`),
  addPackage: (name: string, auto_rebuild: boolean) =>
    sendJson<Package>('POST', '/api/v1/packages', { name, auto_rebuild }),
  removePackage: (name: string) =>
    sendJson<void>('DELETE', `/api/v1/packages/${name}`),
  rebuildPackage: (name: string) =>
    sendJson<{ build_id: number }>('POST', `/api/v1/packages/${name}/rebuild`),
  setAutoRebuild: (name: string, auto_rebuild: boolean) =>
    sendJson<Package>('PATCH', `/api/v1/packages/${name}/auto`, { auto_rebuild }),
  /**
   * Update per-package build flags. Only the fields set to `true`
   * in `patch` are merged into the existing flags; sending `false`
   * is a no-op. Use the admin endpoint to clear flags.
   */
  setBuildFlags: (name: string, patch: Partial<PackageBuildFlags>) =>
    sendJson<Package>('PATCH', `/api/v1/packages/${name}/flags`, patch),
  listBuilds: (opts: { pkg?: string; status?: string; limit?: number } = {}) => {
    const qs = new URLSearchParams();
    if (opts.pkg) qs.set('pkg', opts.pkg);
    if (opts.status) qs.set('status', opts.status);
    if (opts.limit) qs.set('limit', String(opts.limit));
    const q = qs.toString();
    return getJson<Build[]>(`/api/v1/builds${q ? `?${q}` : ''}`);
  },
  getBuild: (id: number) => getJson<Build>(`/api/v1/builds/${id}`),
  rawLogs: (id: number) =>
    fetch(`/api/v1/builds/${id}/logs.txt`, { credentials: 'include' }).then((r) => {
      if (!r.ok) throw new ApiError(r.status, `logs ${id}: ${r.status}`);
      return r.text();
    }),
  queue: () => getJson<Queue>('/api/v1/queue')
};

export { ApiError };

/// Subscribe to the SSE log stream for `buildId`. Returns an
/// `EventSource` which the caller should `.close()` when done.
export function streamLogs(buildId: number): EventSource {
  return new EventSource(`/api/v1/builds/${buildId}/logs`, {
    withCredentials: true
  });
}

/// Format a Unix epoch (seconds) as a short, human-readable local
/// time string. Returns "-" for null.
export function fmtTs(ts: number | null | undefined): string {
  if (ts == null) return '-';
  if (ts <= 0) return String(ts);
  try {
    return new Date(ts * 1000).toLocaleString();
  } catch {
    return String(ts);
  }
}

// paur UI: tiny auth state shared across the layout.
//
// We don't keep the password or token in this store — the token lives
// in an HttpOnly cookie managed by the browser. This module only
// tracks the boolean "am I logged in?" plus a hint about whether the
// admin password is configured at all (so we can show a setup hint).

import { writable, type Writable } from 'svelte/store';
import { auth, type AuthStatus } from './api';

export interface AuthState {
  ready: boolean;
  authenticated: boolean;
  passwordSet: boolean;
}

const initial: AuthState = {
  ready: false,
  authenticated: false,
  passwordSet: false
};

export const authState: Writable<AuthState> = writable(initial);

export async function refreshAuth(): Promise<void> {
  try {
    const s: AuthStatus = await auth.status();
    authState.set({
      ready: true,
      authenticated: s.authenticated,
      passwordSet: s.password_set
    });
  } catch {
    // Network error — keep whatever we had, just mark ready.
    authState.update((s) => ({ ...s, ready: true }));
  }
}

export async function login(username: string, password: string): Promise<void> {
  const r = await auth.login(username, password);
  if (!r.ok) {
    const text = await r.text();
    throw new Error(text || `login failed (${r.status})`);
  }
  // Cookie is set by the server; reflect new state.
  authState.update((s) => ({
    ...s,
    authenticated: true,
    passwordSet: true,
    ready: true
  }));
}

export async function logout(): Promise<void> {
  await auth.logout();
  authState.update((s) => ({ ...s, authenticated: false, ready: true }));
}

/**
 * PairingGate — browser pairing flow.
 *
 * Shown when the user opens the dashboard without a valid auth token.
 * Displays a 6-digit pairing code that the user sends via IM `/pair`
 * command. The page polls for verification and auto-reloads on success.
 *
 * Also keeps the legacy "paste token" fallback for advanced users.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

const STORAGE_KEY = "vibearound.auth.token";
const POLL_INTERVAL_MS = 2000;
const CODE_TTL_SECS = 60;

/**
 * Only allow same-origin redirects after pairing. Anything else (absolute
 * URL, protocol-relative, or scheme like `javascript:`) is rejected to
 * prevent open-redirect / phishing.
 */
function isSafeNext(next: string): boolean {
  return next.startsWith("/") && !next.startsWith("//");
}

type PairStatus = "loading" | "pending" | "expired" | "verified";

export function PairingGate() {
  const [code, setCode] = useState<string | null>(null);
  const [status, setStatus] = useState<PairStatus>("loading");
  const [remaining, setRemaining] = useState(CODE_TTL_SECS);
  const [showTokenInput, setShowTokenInput] = useState(false);
  const [pasted, setPasted] = useState("");
  const [tokenError, setTokenError] = useState<string | null>(null);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const startTimeRef = useRef<number>(0);

  const stopPolling = useCallback(() => {
    if (pollRef.current) {
      clearInterval(pollRef.current);
      pollRef.current = null;
    }
    if (timerRef.current) {
      clearInterval(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const startPairing = useCallback(async () => {
    stopPolling();
    setStatus("loading");
    setCode(null);
    setRemaining(CODE_TTL_SECS);

    try {
      const res = await fetch("/va/api/pair/start", { method: "POST" });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      setCode(data.code);
      setStatus("pending");
      startTimeRef.current = Date.now();

      // Start countdown timer
      timerRef.current = setInterval(() => {
        const elapsed = Math.floor((Date.now() - startTimeRef.current) / 1000);
        const left = Math.max(0, CODE_TTL_SECS - elapsed);
        setRemaining(left);
        if (left <= 0) {
          setStatus("expired");
          stopPolling();
        }
      }, 1000);

      // Start polling for verification
      pollRef.current = setInterval(async () => {
        try {
          const r = await fetch(`/va/api/pair/status?sid=${encodeURIComponent(data.sid)}`);
          if (!r.ok) return;
          const d = await r.json();
          if (d.status === "verified") {
            stopPolling();
            setStatus("verified");
            // Store the token in sessionStorage for API/WS auth.
            // The cookie is also set (by the status endpoint) for /u preview routes.
            if (d.token) {
              window.sessionStorage.setItem(STORAGE_KEY, d.token);
            }
            // Honor ?next=<url> from the gate URL so the user lands on
            // the page they originally tried to open. Default to dashboard.
            const params = new URLSearchParams(window.location.search);
            const nextRaw = params.get("next");
            const next = nextRaw && isSafeNext(nextRaw) ? nextRaw : "/va/";
            setTimeout(() => {
              window.location.replace(next);
            }, 500);
          } else if (d.status === "expired") {
            stopPolling();
            setStatus("expired");
          }
        } catch {
          // Ignore transient network errors during polling.
        }
      }, POLL_INTERVAL_MS);
    } catch {
      setStatus("expired");
    }
  }, [stopPolling]);

  useEffect(() => {
    startPairing();
    return stopPolling;
  }, [startPairing, stopPolling]);

  const submitToken = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = pasted.trim();
    if (!trimmed) {
      setTokenError("Token is required.");
      return;
    }
    if (!/^[0-9a-fA-F]{32,}$/.test(trimmed)) {
      setTokenError("That doesn't look like a VibeAround auth token.");
      return;
    }
    window.sessionStorage.setItem(STORAGE_KEY, trimmed);
    window.location.reload();
  };

  return (
    <div className="flex min-h-screen items-center justify-center bg-background px-6 py-12 text-foreground">
      <div className="w-full max-w-md space-y-6">
        {/* Header badge */}
        <div className="flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-primary/10 text-primary">
            <svg
              xmlns="http://www.w3.org/2000/svg"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth={2}
              strokeLinecap="round"
              strokeLinejoin="round"
              className="h-5 w-5"
              aria-hidden="true"
            >
              <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
              <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
            </svg>
          </div>
          <div>
            <h1 className="text-base font-semibold tracking-tight">
              Pair your browser
            </h1>
            <p className="text-xs text-muted-foreground">
              Connect this browser to your VibeAround instance.
            </p>
          </div>
        </div>

        {/* Pairing code display */}
        <div className="rounded-lg border border-border bg-card/50 p-6 text-center">
          {status === "loading" && (
            <p className="text-sm text-muted-foreground">Generating pairing code…</p>
          )}

          {(status === "pending" || status === "expired") && code && (
            <>
              <p className="mb-3 text-xs font-medium uppercase tracking-wide text-muted-foreground">
                Your pairing code
              </p>
              <p
                className={`font-mono text-4xl font-bold tracking-[0.3em] ${
                  status === "expired"
                    ? "text-muted-foreground/40 line-through"
                    : "text-foreground"
                }`}
              >
                {code}
              </p>
              <div className="mt-3 flex items-center justify-center gap-2 text-xs text-muted-foreground">
                {status === "pending" ? (
                  <>
                    <span className="inline-block h-2 w-2 animate-pulse rounded-full bg-primary" />
                    <span>
                      Waiting for <span className="font-mono">/pair {code}</span> · {remaining}s
                    </span>
                  </>
                ) : (
                  <span className="text-destructive">Code expired</span>
                )}
              </div>
            </>
          )}

          {status === "verified" && (
            <p className="text-sm font-medium text-primary">
              ✓ Paired! Loading dashboard…
            </p>
          )}
        </div>

        {/* Instructions */}
        {status !== "verified" && (
          <div className="space-y-2">
            <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
              How to pair
            </p>
            <ol className="list-decimal space-y-1 pl-5 text-sm text-foreground/90">
              <li>Open any IM channel connected to VibeAround.</li>
              <li>
                Send{" "}
                <span className="font-mono text-xs bg-muted px-1 py-0.5 rounded">
                  /pair {code ?? "···"}
                </span>
              </li>
              <li>This page will update automatically.</li>
            </ol>
          </div>
        )}

        {/* Refresh button when expired */}
        {status === "expired" && (
          <Button
            type="button"
            onClick={startPairing}
            className="w-full"
          >
            Generate new code
          </Button>
        )}

        {/* Token fallback */}
        {status !== "verified" && (
          <details
            className="group rounded-lg border border-border bg-card/50"
            open={showTokenInput}
            onToggle={(e) => setShowTokenInput((e.target as HTMLDetailsElement).open)}
          >
            <summary className="cursor-pointer select-none px-4 py-3 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground">
              I have a token — paste it
            </summary>
            <form onSubmit={submitToken} className="space-y-3 border-t border-border px-4 py-3">
              <label className="block space-y-1.5">
                <span className="text-xs text-muted-foreground">
                  Session auth token (from{" "}
                  <span className="font-mono">~/.vibearound/auth.json</span>)
                </span>
                <Input
                  type="text"
                  autoComplete="off"
                  spellCheck={false}
                  value={pasted}
                  onChange={(e) => {
                    setPasted(e.target.value);
                    if (tokenError) setTokenError(null);
                  }}
                  placeholder="hex token…"
                  className="font-mono text-xs"
                />
              </label>
              {tokenError && (
                <p className="text-xs text-destructive">{tokenError}</p>
              )}
              <Button
                type="submit"
                size="sm"
                className="w-full text-xs"
              >
                Unlock dashboard
              </Button>
            </form>
          </details>
        )}

        {/* Footer */}
        <p className="text-center text-[10px] text-muted-foreground/60">
          VibeAround · pairing codes expire after 1 minute
        </p>
      </div>
    </div>
  );
}

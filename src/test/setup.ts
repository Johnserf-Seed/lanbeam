// Shared test setup, loaded once per test file (vitest.config.ts setupFiles).
// globals are OFF (tests import describe/it/expect from vitest explicitly, so
// tsc needs no extra types entry) — which also means testing-library cannot
// auto-register its cleanup hook; do it here so components unmount between
// tests instead of leaking DOM and event listeners across cases.
import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => {
  cleanup();
});

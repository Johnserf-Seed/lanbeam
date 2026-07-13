// Shared render helper for component/page tests. Wraps the tree in a
// MemoryRouter (harmless for modals, required for pages that use useNavigate /
// <Outlet>) and ensures i18n is initialized (it auto-inits on import). Stores
// are NOT reset here — a test seeds the zustand stores it needs via
// useX.setState(...) before rendering, and setup.ts unmounts between tests.
import { render, type RenderOptions } from "@testing-library/react";
import type { ReactElement, ReactNode } from "react";
import { MemoryRouter } from "react-router-dom";
import "../i18n";

function Providers({ children }: { children: ReactNode }) {
  return <MemoryRouter>{children}</MemoryRouter>;
}

export function renderUI(
  ui: ReactElement,
  opts?: Omit<RenderOptions, "wrapper">,
) {
  return render(ui, { wrapper: Providers, ...opts });
}

export * from "@testing-library/react";

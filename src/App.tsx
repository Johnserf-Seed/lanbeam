import { HashRouter, Navigate, Routes, Route } from "react-router-dom";
import AppShell from "./components/AppShell";
import DevicesPage from "./pages/DevicesPage";
import TransfersPage from "./pages/TransfersPage";
import InboxPage from "./pages/InboxPage";
import TrustedPage from "./pages/TrustedPage";
import SettingsPage from "./pages/SettingsPage";

export default function App() {
  return (
    <HashRouter>
      <Routes>
        <Route element={<AppShell />}>
          <Route index element={<DevicesPage />} />
          <Route path="transfers" element={<TransfersPage />} />
          <Route path="inbox" element={<InboxPage />} />
          <Route path="trusted" element={<TrustedPage />} />
          <Route path="settings" element={<SettingsPage />} />
          {/* Any other path lands on Devices. It must exist, and it must sit
              INSIDE the layout route: with no match at all, <Routes> renders
              nothing — AppShell included, and with it the sidebar, the header
              and every backend event listener (an incoming request would then
              time out unseen). One stray `#/devices` was a blank app. */}
          <Route path="*" element={<Navigate to="/" replace />} />
        </Route>
      </Routes>
    </HashRouter>
  );
}

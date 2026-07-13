import { HashRouter, Routes, Route } from "react-router-dom";
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
        </Route>
      </Routes>
    </HashRouter>
  );
}

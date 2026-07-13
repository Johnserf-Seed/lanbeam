import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { isTauri } from "../bridge/api";

type Win = import("@tauri-apps/api/window").Window;

async function currentWindow(): Promise<Win> {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

/** Frontend-drawn window chrome (─ □ ×) for the undecorated Tauri window.
 *  Renders nothing in a plain browser. */
export default function WindowControls() {
  const { t } = useTranslation();
  const [maxed, setMaxed] = useState(false);

  useEffect(() => {
    if (!isTauri) return;
    let un: (() => void) | undefined;
    let disposed = false;
    void currentWindow().then((w) => {
      if (disposed) return;
      void w.isMaximized().then(setMaxed);
      void w
        .onResized(() => {
          void w.isMaximized().then(setMaxed);
        })
        .then((off) => {
          if (disposed) off();
          else un = off;
        });
    });
    return () => {
      disposed = true;
      un?.();
    };
  }, []);

  if (!isTauri) return null;

  return (
    <div style={{ display: "flex", gap: 2, marginLeft: 4, flex: "none" }}>
      <button
        type="button"
        className="win-btn"
        title={t("win.minimize")}
        onClick={() => void currentWindow().then((w) => w.minimize())}
      >
        <span
          style={{
            width: 10,
            height: 1.5,
            borderRadius: 1,
            background: "currentColor",
          }}
        />
      </button>
      <button
        type="button"
        className="win-btn"
        title={maxed ? t("win.restore") : t("win.maximize")}
        onClick={() => void currentWindow().then((w) => w.toggleMaximize())}
      >
        {maxed ? (
          <span style={{ position: "relative", width: 10, height: 10 }}>
            <span
              style={{
                position: "absolute",
                right: 0,
                top: 0,
                width: 7.5,
                height: 7.5,
                border: "1.5px solid currentColor",
                borderRadius: 2,
                boxSizing: "border-box",
                opacity: 0.6,
              }}
            />
            <span
              style={{
                position: "absolute",
                left: 0,
                bottom: 0,
                width: 7.5,
                height: 7.5,
                border: "1.5px solid currentColor",
                borderRadius: 2,
                boxSizing: "border-box",
                background: "var(--bg)",
              }}
            />
          </span>
        ) : (
          <span
            style={{
              width: 9,
              height: 9,
              border: "1.5px solid currentColor",
              borderRadius: 2,
              boxSizing: "border-box",
            }}
          />
        )}
      </button>
      <button
        type="button"
        className="win-btn close"
        title={t("win.close")}
        onClick={() => void currentWindow().then((w) => w.close())}
      >
        <span style={{ fontSize: 15, lineHeight: 1, marginTop: -1 }}>×</span>
      </button>
    </div>
  );
}

import { useTranslation } from "react-i18next";
import { useOverlays } from "../lib/store";

/** Full-window overlay while OS files are dragged over the app. */
export default function DragOverlay() {
  const { t } = useTranslation();
  const dragOver = useOverlays((s) => s.dragOver);
  if (!dragOver) return null;
  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 40,
        pointerEvents: "none",
        animation: "lbFade .15s ease",
      }}
    >
      <div
        style={{
          position: "absolute",
          inset: 10,
          border: "2px dashed var(--accent)",
          borderRadius: 18,
          background: "var(--accent-soft)",
        }}
      />
      <div
        style={{
          position: "absolute",
          left: 0,
          right: 0,
          bottom: 36,
          display: "flex",
          justifyContent: "center",
        }}
      >
        <div
          style={{
            background: "var(--toast-bg)",
            color: "var(--toast-fg)",
            fontSize: 12.5,
            fontWeight: 600,
            padding: "10px 18px",
            borderRadius: 99,
            boxShadow: "var(--shadow)",
          }}
        >
          {t("drag.hint")}
        </div>
      </div>
    </div>
  );
}

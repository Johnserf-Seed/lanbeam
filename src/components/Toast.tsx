import { useToast } from "../lib/store";

/** Bottom-centered toast pill (offset past the sidebar), optional action. */
export default function Toast() {
  const msg = useToast((s) => s.msg);
  const action = useToast((s) => s.action);
  if (!msg) return null;
  return (
    <div
      style={{
        position: "fixed",
        left: 214,
        right: 0,
        bottom: 112,
        zIndex: 70,
        display: "flex",
        justifyContent: "center",
        pointerEvents: "none",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 14,
          background: "var(--toast-bg)",
          color: "var(--toast-fg)",
          fontSize: 12.5,
          fontWeight: 600,
          padding: "9px 17px",
          borderRadius: 99,
          boxShadow: "var(--shadow)",
          animation: "lbUp .22s ease",
          pointerEvents: "auto",
          maxWidth: "calc(100% - 60px)",
        }}
      >
        <span>{msg}</span>
        {action && (
          // biome-ignore lint/a11y/useSemanticElements: styled span kept (a <button> would change the pill's visual styling); made keyboard-operable via role/tabIndex/onKeyDown
          <span
            role="button"
            tabIndex={0}
            onClick={action.fn}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                if (e.key === " ") e.preventDefault();
                action.fn();
              }
            }}
            style={{
              color: "var(--toast-accent)",
              cursor: "pointer",
              whiteSpace: "nowrap",
              fontWeight: 700,
            }}
          >
            {action.label}
          </span>
        )}
      </div>
    </div>
  );
}

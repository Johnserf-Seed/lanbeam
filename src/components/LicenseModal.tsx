import { useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { LICENSES } from "../lib/licenses";
import { useOverlays } from "../lib/store";
import { ModalHead } from "./ui";

/** 开源许可 (M9 polish): a read-only inventory of the third-party components
 *  LanBeam bundles — its direct frontend (npm) and Rust (crate) dependencies
 *  with the SPDX license each declares. Pure static data (src/lib/licenses.ts),
 *  so it renders identically in the desktop app and the browser demo; the footer
 *  notes LanBeam's own MIT license and that full texts ship with the source. */
export default function LicenseModal() {
  const { t } = useTranslation();
  const open = useOverlays((s) => s.licenseOpen);
  const setLicense = useOverlays((s) => s.setLicense);
  // True only when the pointer went down on the scrim itself; a drag that
  // starts inside the modal (selecting a version string) and ends on the scrim
  // dispatches click on the scrim, which must not dismiss the modal.
  const scrimDown = useRef(false);

  // Alphabetical by name (case-insensitive) so npm + crates interleave into one
  // scannable list; sorted once since LICENSES is static.
  const rows = useMemo(
    () =>
      [...LICENSES].sort((a, b) =>
        a.name.toLowerCase().localeCompare(b.name.toLowerCase()),
      ),
    [],
  );

  if (!open) return null;
  const close = () => setLicense(false);

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: click-away backdrop — keyboard users dismiss via the × button / Esc
    // biome-ignore lint/a11y/useKeyWithClickEvents: same
    <div
      className="scrim"
      style={{ zIndex: 56 }}
      onMouseDown={(e) => {
        scrimDown.current = e.target === e.currentTarget;
      }}
      onClick={() => {
        if (scrimDown.current) close();
      }}
    >
      {/* biome-ignore lint/a11y/noStaticElementInteractions: click-away guard — stops scrim dismissal, not an interactive control */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: same */}
      <div
        className="modal"
        style={{ width: 460, fontFamily: "var(--font)" }}
        onClick={(e) => e.stopPropagation()}
      >
        <ModalHead
          title={t("settings.licenseTitle")}
          sub={t("settings.licenseCount", { n: rows.length })}
          onClose={close}
        />
        <div
          className="scroll-y"
          style={{ maxHeight: 372, margin: "12px 0 2px" }}
        >
          {rows.map((e, i) => (
            <div
              key={e.name}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 10,
                padding: "9px 20px",
                borderTop: i ? "1px solid var(--border)" : "none",
              }}
            >
              <span
                style={{
                  fontSize: 12.5,
                  fontWeight: 600,
                  color: "var(--ink2)",
                  wordBreak: "break-all",
                }}
              >
                {e.name}
              </span>
              <span
                style={{
                  fontFamily: "var(--mono)",
                  fontSize: 11,
                  color: "var(--muted2)",
                  flex: "none",
                }}
              >
                {e.version}
              </span>
              <div style={{ flex: 1 }} />
              <span
                style={{
                  fontFamily: "var(--mono)",
                  fontSize: 10,
                  fontWeight: 600,
                  color: "var(--accent-ink)",
                  background: "var(--accent-soft)",
                  borderRadius: 99,
                  padding: "3px 9px",
                  whiteSpace: "nowrap",
                  flex: "none",
                }}
              >
                {e.license}
              </span>
            </div>
          ))}
        </div>
        <div className="modal-foot">{t("settings.licenseSelfNote")}</div>
      </div>
    </div>
  );
}
